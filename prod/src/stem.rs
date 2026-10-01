//! I²C-**SLAVE** transport — the Stem-bus surface the SoM (and other masters) read/write our register
//! interface on. This is the transport regmap.rs flags as the TODO: it dispatches a master's
//! command-pointer + read/write bytes onto [`regmap`] via [`RegFile`].
//!
//! **One transport, two peripherals.** The protocol / [`RegFile`] / [`Slave`] logic is chip-agnostic;
//! only the eUSCI_B instance + pins differ, isolated in the [`hw`] seam:
//!   • DUAL families (fr247x/fr215x/fr235x): eUSCI_B1 (`ucb1*`) — a dedicated slave bus, independent
//!     of the eUSCI_B0 sensor-master, so it never disturbs enumeration. The UCB1↔pin mux is **chip-
//!     specific** (verified per-datasheet, gated by `_stem_p3`/`_stem_p4`): FR247x = **P3.2/P3.6**
//!     (P3SEL=01); FR23xx/FR21xx (FR2355/FR2155) = **P4.6/P4.7** (P4SEL=01).
//!   • FR2433 (fr24xx, single-I²C slave-only): its ONE eUSCI_B0 (`ucb0*`, mode-split PAC field
//!     `usci_b0_i2c_mode`) on **P1.2/P1.3** — this bus IS the node's whole job (no sensor master).
//!
//! **Polled, no ISR** (matching prod's single-threaded model). eUSCI_B auto-clock-stretches SCL until
//! firmware services the TX/RX buffer, so [`poll`] just needs to be called often in the main loop;
//! latency shows up as clock-stretch on the master, not lost data. (A long CPU-bound stretch — e.g.
//! the console VL53L0X burst — will stall a concurrent master until it returns; a production build
//! would service this more aggressively. Documented, acceptable for bring-up.)
//!
//! **What's backed today:** the debug/identity window (0x30–0x3F) is REAL — served by
//! [`Status::read_reg`], so the SoM reads exactly what the bench UART dump shows (identity, build id,
//! status, dev-count, fault). The PCA9698 GPIO banks (IP/OP/IOC/PI/MSK) are a coherent **firmware
//! shadow** — writes persist and read back — but have **no physical pin backing yet** (that needs the
//! bank↔port pin map in `model::PinMap`; IP reads 0, OP/IOC/PI/MSK are shadow). Voltages (0x2C/0x2D)
//! read 0 until the rail ADC lands. This is the wire contract, live, minus the not-yet-defined pins.

use crate::regmap::{self, ShadowState};
use crate::status::Status;
use crate::pac::Peripherals;

/// I²C-slave address. PCA9698 parts answer a strap-selected address in 0x20–0x27; we default to
/// the group base. TODO(confirm-on-doc): pin this against I2C-API.md's canonical Stem-node address.
pub const STEM_ADDR: u16 = 0x20;

// ── CTLW0 / I2COA0 / IFG bit definitions (eUSCI_B, slau445 §24). Shared: the register *layout* is
// identical across instances; only the accessor name differs (handled in `hw`). ──
const UCSWRST: u16 = 0x0001;
const UCMODE_3: u16 = 0x0600; // I2C mode
const UCSYNC: u16 = 0x0100;
const UCOAEN: u16 = 0x0400; // I2COA0: own-address enable
const UCRXIFG0: u16 = 0x0001;
const UCTXIFG0: u16 = 0x0002;
const UCSTTIFG: u16 = 0x0004; // START matching our own address
const UCSTPIFG: u16 = 0x0008;
const UCNACKIFG: u16 = 0x0020;

/// Chip-specific seam: the eUSCI_B **instance** + its slave pins. The two implementations are register-
/// for-register identical apart from the peripheral accessor (ucb1 vs ucb0) and the port/bits, so the
/// shared protocol below never mentions a concrete peripheral. (The fr24xx PAC is mode-split, so its
/// IFG is `ucb0ifg_i2c`; the dual PAC exposes a single `ucb1ifg`.)
#[cfg(all(feature = "_dual", not(any(feature = "_stem_p3", feature = "_stem_p4"))))]
compile_error!("a _dual family must select exactly one Stem pin-group: _stem_p3 (FR247x) or _stem_p4 (FR23xx/FR21xx)");

#[cfg(feature = "_dual")]
mod hw {
    use super::{Peripherals, STEM_ADDR, UCMODE_3, UCOAEN, UCSWRST, UCSYNC};

    // The UCB1 register programming below is identical across the dual family; ONLY the port the
    // UCB1SDA/UCB1SCL signals mux onto differs by chip (the UCB1↔pin map is NOT uniform — verified
    // against each datasheet). `route_pins` is the only chip-split; `init` and the accessors are shared.
    //   • _stem_p3 → FR247x (FR2476/FR2475): UCB1 on P3.2(SDA)/P3.6(SCL), P3SEL=01  [FR2476 ds p67]
    //   • _stem_p4 → FR23xx/FR21xx (FR2355/FR2155): UCB1 on P4.6(SDA)/P4.7(SCL), P4SEL=01 [FR2355 ds p24/p102]
    #[cfg(feature = "_stem_p3")]
    fn route_pins(p: &Peripherals) {
        const SDA_BIT: u8 = 0x04; // P3.2 = UCB1SDA
        const SCL_BIT: u8 = 0x40; // P3.6 = UCB1SCL
        // Route P3.2/P3.6 to the primary module function (SEL1=0, SEL0=1 = UCB1SDA/UCB1SCL).
        p.p3.p3sel1().modify(|r, w| unsafe { w.bits(r.bits() & !(SDA_BIT | SCL_BIT)) });
        p.p3.p3sel0().modify(|r, w| unsafe { w.bits(r.bits() | (SDA_BIT | SCL_BIT)) });
    }
    #[cfg(feature = "_stem_p4")]
    fn route_pins(p: &Peripherals) {
        const SDA_BIT: u8 = 0x40; // P4.6 = UCB1SDA (bit 6)
        const SCL_BIT: u8 = 0x80; // P4.7 = UCB1SCL (bit 7)
        // Route P4.6/P4.7 to the primary module function (SEL1=0, SEL0=1 = UCB1SDA/UCB1SCL). Distinct
        // bits from the FR2355 console UART (P4.2/P4.3), so the read-modify-write doesn't disturb it.
        p.p4.p4sel1().modify(|r, w| unsafe { w.bits(r.bits() & !(SDA_BIT | SCL_BIT)) });
        p.p4.p4sel0().modify(|r, w| unsafe { w.bits(r.bits() | (SDA_BIT | SCL_BIT)) });
    }

    pub fn init(p: &Peripherals) {
        route_pins(p); // chip-specific UCB1 pin mux (SEL=01); see route_pins above
        p.e_usci_b1.ucb1ctlw0().write(|w| unsafe { w.bits(UCSWRST) }); // hold in reset
        p.e_usci_b1
            .ucb1ctlw0()
            .modify(|r, w| unsafe { w.bits(r.bits() | UCMODE_3 | UCSYNC) }); // I2C SLAVE (no UCMST)
        p.e_usci_b1.ucb1i2coa0().write(|w| unsafe { w.bits(STEM_ADDR | UCOAEN) });
        p.e_usci_b1.ucb1ctlw0().modify(|r, w| unsafe { w.bits(r.bits() & !UCSWRST) }); // release
        p.e_usci_b1.ucb1ifg().write(|w| unsafe { w.bits(0) }); // clear stale flags
    }
    #[inline]
    pub fn ifg(p: &Peripherals) -> u16 {
        p.e_usci_b1.ucb1ifg().read().bits()
    }
    #[inline]
    pub fn clear(p: &Peripherals, bit: u16) {
        p.e_usci_b1.ucb1ifg().modify(|r, w| unsafe { w.bits(r.bits() & !bit) });
    }
    #[inline]
    pub fn rx(p: &Peripherals) -> u8 {
        p.e_usci_b1.ucb1rxbuf().read().bits() as u8 // read clears UCRXIFG
    }
    #[inline]
    pub fn tx(p: &Peripherals, v: u8) {
        p.e_usci_b1.ucb1txbuf().write(|w| unsafe { w.bits(v as u16) }); // write clears UCTXIFG
    }
}

#[cfg(feature = "fr24xx")]
mod hw {
    use super::{Peripherals, STEM_ADDR, UCMODE_3, UCOAEN, UCSWRST, UCSYNC};
    // FR2433's single eUSCI_B0 as the MCU-bus slave. UCB0SDA=P1.2, UCB0SCL=P1.3 at SEL=01 — CONFIRMED
    // against the MSP430FR2433 datasheet (SLASE59F Table 6-17, Port P1 Pin Functions): P1.2 P1SELx=01 →
    // UCB0SIMO/UCB0SDA, P1.3 P1SELx=01 → UCB0SOMI/UCB0SCL. SEL=01 = SEL1:0 = 0,1 (routed below).
    const SDA_BIT: u8 = 0x04; // P1.2 = UCB0SDA
    const SCL_BIT: u8 = 0x08; // P1.3 = UCB0SCL

    pub fn init(p: &Peripherals) {
        p.p1.p1sel1().modify(|r, w| unsafe { w.bits(r.bits() & !(SDA_BIT | SCL_BIT)) });
        p.p1.p1sel0().modify(|r, w| unsafe { w.bits(r.bits() | (SDA_BIT | SCL_BIT)) });
        let b = &p.usci_b0_i2c_mode;
        b.ucb0ctlw0().write(|w| unsafe { w.bits(UCSWRST) }); // hold in reset
        b.ucb0ctlw0()
            .modify(|r, w| unsafe { w.bits(r.bits() | UCMODE_3 | UCSYNC) }); // I2C SLAVE (no UCMST)
        b.ucb0i2coa0().write(|w| unsafe { w.bits(STEM_ADDR | UCOAEN) });
        b.ucb0ctlw0().modify(|r, w| unsafe { w.bits(r.bits() & !UCSWRST) }); // release
        b.ucb0ifg_i2c().write(|w| unsafe { w.bits(0) }); // clear stale flags
    }
    #[inline]
    pub fn ifg(p: &Peripherals) -> u16 {
        p.usci_b0_i2c_mode.ucb0ifg_i2c().read().bits()
    }
    #[inline]
    pub fn clear(p: &Peripherals, bit: u16) {
        p.usci_b0_i2c_mode
            .ucb0ifg_i2c()
            .modify(|r, w| unsafe { w.bits(r.bits() & !bit) });
    }
    #[inline]
    pub fn rx(p: &Peripherals) -> u8 {
        p.usci_b0_i2c_mode.ucb0rxbuf().read().bits() as u8 // read clears UCRXIFG
    }
    #[inline]
    pub fn tx(p: &Peripherals, v: u8) {
        p.usci_b0_i2c_mode.ucb0txbuf().write(|w| unsafe { w.bits(v as u16) }); // write clears UCTXIFG
    }
}

/// Configure the node's eUSCI_B as an I²C slave at [`STEM_ADDR`] and route its pins (see [`hw`]).
pub fn init(p: &Peripherals) {
    hw::init(p);
}

/// Physical GPIO bank backing — the seam that makes the PCA9698 IP/OP/IOC banks touch real MSP430
/// port registers instead of a pure firmware shadow. Chip-specific (port register names + which bits
/// are safe to drive), so cfg-split like [`hw`]. On the DUAL node the board's bank↔port map isn't
/// defined yet, so its banks stay shadow — these are no-ops and IP reads 0, exactly as before.
#[cfg(feature = "_dual")]
mod gpio {
    use super::Peripherals;
    #[inline]
    pub fn read_ip(_p: &Peripherals, _bank: u8, _pol: u8) -> u8 {
        0 // dual: no physical input backing yet (unchanged shadow contract)
    }
    #[inline]
    pub fn drive_op(_p: &Peripherals, _bank: u8, _val: u8) {}
    #[inline]
    pub fn set_dir(_p: &Peripherals, _bank: u8, _ioc: u8) {}
}

#[cfg(feature = "fr24xx")]
mod gpio {
    //! FR2433 (24-pin RGE) bank→port backing — datasheet SLASE59F (Fig 4-1; per-pin SEL decode Tables
    //! 6-17 / 6-18). Bank 0 = P1, bank 1 = P2 (byte-wide ports). Bank 2 = P3 stays shadow: the PAC
    //! models P3 as per-bit registers and only P3.0–2 are bonded on VQFN-24. The USABLE masks exclude
    //! every pin with a reserved role so driving a bank NEVER disturbs a bus, the console, or the clock:
    //!   • P1.2/3 = UCB0 I²C slave (always); P1.4/5 = UCA0 console (Table 6-17). P1.6/7 = UCA0CLK/STE +
    //!     JTAG TDI/TDO — free as GPIO under the LaunchPad's 2-wire SBW (JTAG 4-wire not used).
    //!   • P2.0/1 = **XOUT/XIN** (LFXT crystal, Table 6-18) — EXCLUDED: driving a populated crystal's
    //!     pins is a hazard, and their GPIO use on the product is a BOM decision, not a default.
    //! PROVISIONAL pending an FR2433 connections.toml/BOM (which of the *remaining* bits are actually
    //! wired as GPIO on the product PCB); the masks below are the datasheet-safe superset.
    use super::Peripherals;
    const P1_GPIO: u8 = 0xC3; // P1.0,1,6,7 — exclude UCB0 SDA/SCL (P1.2/3) + UCA0 TXD/RXD (P1.4/5)
    const P2_GPIO: u8 = 0xFC; // P2.2–7 — exclude XOUT/XIN (P2.0/1, LFXT crystal pins; Table 6-18)

    /// IP (Input Port): live pin state XOR the polarity shadow, restricted to the backed bits.
    #[inline]
    pub fn read_ip(p: &Peripherals, bank: u8, pol: u8) -> u8 {
        match bank {
            0 => (p.p1.p1in().read().bits() ^ pol) & P1_GPIO,
            1 => (p.p2.p2in().read().bits() ^ pol) & P2_GPIO,
            _ => 0, // bank 2 (P3) + unbacked → 0
        }
    }
    /// OP (Output Port): drive PxOUT, backed bits only — reserved bus/console bits left untouched.
    #[inline]
    pub fn drive_op(p: &Peripherals, bank: u8, val: u8) {
        match bank {
            0 => {
                p.p1.p1out().modify(|r, w| unsafe { w.bits((r.bits() & !P1_GPIO) | (val & P1_GPIO)) });
            }
            1 => {
                p.p2.p2out().modify(|r, w| unsafe { w.bits((r.bits() & !P2_GPIO) | (val & P2_GPIO)) });
            }
            _ => {}
        }
    }
    /// IOC (I/O Config / direction): PCA9698 uses 1=input, 0=output → PxDIR = !IOC (backed bits only).
    #[inline]
    pub fn set_dir(p: &Peripherals, bank: u8, ioc: u8) {
        let dir = !ioc;
        match bank {
            0 => {
                p.p1.p1dir().modify(|r, w| unsafe { w.bits((r.bits() & !P1_GPIO) | (dir & P1_GPIO)) });
            }
            1 => {
                p.p2.p2dir().modify(|r, w| unsafe { w.bits((r.bits() & !P2_GPIO) | (dir & P2_GPIO)) });
            }
            _ => {}
        }
    }
}

/// The register file behind the slave: the debug/identity [`Status`] (real) plus the PCA9698 GPIO
/// shadow (coherent, not yet pin-backed). One `read`/`write` maps a decoded [`regmap::RegSel`] to it.
pub struct RegFile {
    /// Debug/identity/status snapshot (0x30–0x3F). Replace on each rescan so the SoM sees fresh state.
    pub status: Status,
    shadow: ShadowState,
    op: [u8; regmap::BANKS as usize],  // Output Port (0x08–0x0F)
    ioc: [u8; regmap::BANKS as usize], // I/O Config / direction (0x18–0x1F)
    /// Pending SoM-requested operating-mode switch (a `MODE_CTRL`/0x3E write). Applied by the main
    /// loop AFTER the transaction — the switch does sensor-bus acquire/release, which must not run
    /// inside `poll` (it'd re-enter the bus mid-transfer). The CURRENT mode is read back via
    /// `status.mode` (kept live by `Status::set_mode`), so reads need no special-casing here.
    mode_request: Option<u8>,
}

impl RegFile {
    pub fn new(status: Status) -> Self {
        Self {
            status,
            shadow: ShadowState::default(),
            op: [0; regmap::BANKS as usize],
            ioc: [0; regmap::BANKS as usize],
            mode_request: None,
        }
    }

    /// Take any pending SoM-requested mode code, clearing it. The main loop decodes it via
    /// `mode::Mode::from_code` and applies the switch through the mode machine.
    pub fn take_mode_request(&mut self) -> Option<u8> {
        self.mode_request.take()
    }

    /// Read the byte a master gets for command index `reg`. `p` drives the physical IP read on fr24xx
    /// (see [`gpio`]); it's unused on the dual node, whose banks are still shadow.
    fn read(&self, p: &Peripherals, reg: u8) -> u8 {
        #[cfg(feature = "_dual")]
        let _ = p;
        match regmap::decode(reg) {
            // Debug/identity window — the real state (identical to the bench UART dump).
            regmap::RegSel::Extended(_) => self.status.read_reg(reg),
            // Input Port — physical PxIN (XOR polarity) on fr24xx; 0 on the still-shadow dual banks.
            regmap::RegSel::InputPort(b) => self.read_ip(p, b),
            regmap::RegSel::OutputPort(b) => self.op[b as usize],
            regmap::RegSel::IoConfig(b) => self.ioc[b as usize],
            regmap::RegSel::PolarityInv(b) => self.shadow.polarity[b as usize],
            regmap::RegSel::IntMask(b) => self.shadow.int_mask[b as usize],
            regmap::RegSel::InitCode => self.shadow.init_code,
            // Voltages need the rail ADC (not wired); everything else reads 0 for now.
            _ => 0,
        }
    }

    // Input Port read, cfg-split so the dual path stays byte-for-byte its old `InputPort(_) => 0` (no
    // array access, no bounds-check path); fr24xx applies the polarity shadow to the live PxIN.
    #[cfg(feature = "fr24xx")]
    #[inline]
    fn read_ip(&self, p: &Peripherals, bank: u8) -> u8 {
        gpio::read_ip(p, bank, self.shadow.polarity[bank as usize])
    }
    #[cfg(feature = "_dual")]
    #[inline]
    fn read_ip(&self, _p: &Peripherals, _bank: u8) -> u8 {
        0
    }

    /// Apply a master write of `val` to command index `reg`. Read-only regions are ignored. On fr24xx,
    /// OP/IOC also drive the physical port (PxOUT / PxDIR) via [`gpio`]; the shadow is still kept so a
    /// read-back returns the last written value regardless of pin backing.
    fn write(&mut self, p: &Peripherals, reg: u8, val: u8) {
        #[cfg(feature = "_dual")]
        let _ = p;
        // Operating-mode control (0x3E): record the request; the main loop applies it after the
        // transaction (decode(0x3E) is Extended, which is otherwise read-only → this must precede it).
        if reg == regmap::MODE_CTRL {
            self.mode_request = Some(val);
            return;
        }
        match regmap::decode(reg) {
            regmap::RegSel::OutputPort(b) => {
                self.op[b as usize] = val;
                gpio::drive_op(p, b, val); // no-op on dual
            }
            regmap::RegSel::IoConfig(b) => {
                self.ioc[b as usize] = val;
                gpio::set_dir(p, b, val); // no-op on dual
            }
            regmap::RegSel::PolarityInv(b) => self.shadow.polarity[b as usize] = val,
            regmap::RegSel::IntMask(b) => self.shadow.int_mask[b as usize] = val,
            regmap::RegSel::InitCode => self.shadow.init_code = val,
            // InputPort / debug window / mode regs: read-only or not-yet-backed → ignore.
            _ => {}
        }
    }
}

/// Per-transaction slave state: the current command pointer, the auto-increment flag, and whether the
/// next received byte is the command byte (first after a START) vs write data.
pub struct Slave {
    reg_ptr: u8,
    ai: bool,
    first_byte: bool,
}

impl Slave {
    pub const fn new() -> Self {
        Self {
            reg_ptr: 0,
            ai: false,
            first_byte: true,
        }
    }
}

/// Service any pending eUSCI_B1 slave events (non-blocking). Call frequently from the main loop.
///
/// Protocol (PCA9698, I2C-API.md §7.3): after our address, the master's FIRST written byte is the
/// command register — low 6 bits = pointer, [`regmap::CMD_AI`] = auto-increment. Subsequent written
/// bytes go to the pointed register (stepping the bank under AI); a master read returns the pointed
/// register (also stepping under AI).
pub fn poll(p: &Peripherals, s: &mut Slave, rf: &mut RegFile) {
    let ifg = hw::ifg(p);

    if ifg & UCSTTIFG != 0 {
        s.first_byte = true; // next RX byte is the command; a read keeps the existing pointer
        hw::clear(p, UCSTTIFG);
    }
    if ifg & UCNACKIFG != 0 {
        hw::clear(p, UCNACKIFG);
    }
    if ifg & UCRXIFG0 != 0 {
        let b = hw::rx(p);
        if s.first_byte {
            s.reg_ptr = b & regmap::CMD_REG_MASK;
            s.ai = b & regmap::CMD_AI != 0;
            s.first_byte = false;
        } else {
            rf.write(p, s.reg_ptr, b);
            if s.ai {
                s.reg_ptr = regmap::next_ai(s.reg_ptr);
            }
        }
    }
    if ifg & UCTXIFG0 != 0 {
        let v = rf.read(p, s.reg_ptr);
        hw::tx(p, v);
        if s.ai {
            s.reg_ptr = regmap::next_ai(s.reg_ptr);
        }
    }
    if ifg & UCSTPIFG != 0 {
        hw::clear(p, UCSTPIFG);
        s.first_byte = true;
    }
}

/// Bench self-test for the physical GPIO backing WITHOUT an I²C master (fr24xx has no way to be its
/// own master): drive **bank 0 / bit 0 (P1.0)** through the SAME [`RegFile`] path a SoM master would
/// use, and read it back on PxIN. Configure P1.0 as output (IOC bit0=0), drive high then low (OP),
/// sense IP each time. PASS = IP bit0 tracks the driven level — proving IOC→PxDIR, OP→PxOUT and
/// IP←PxIN all work end-to-end. Leaves P1.0 driven low. On the LaunchPad P1.0 is LED1, so a PASS also
/// blinks it. Console/bench only.
#[cfg(all(feature = "fr24xx", feature = "console"))]
pub fn gpio_selftest(p: &Peripherals, rf: &mut RegFile) -> bool {
    // IOC: 1=input, 0=output (PCA9698). 0xFE → bank-0 bit0 output, the rest input.
    rf.write(p, regmap::IOC_BASE, 0xFE);
    rf.write(p, regmap::OP_BASE, 0x01); // P1.0 high
    let hi = rf.read(p, regmap::IP_BASE) & 0x01;
    rf.write(p, regmap::OP_BASE, 0x00); // P1.0 low
    let lo = rf.read(p, regmap::IP_BASE) & 0x01;
    hi == 0x01 && lo == 0x00
}

/// Result of the B0↔B1 loopback self-test ([`loopback_selftest`]). DUAL-only: needs the eUSCI_B0
/// sensor-master to drive the eUSCI_B1 slave on the same MCU; fr24xx has only the one (slave) bus.
#[cfg(all(feature = "console", feature = "_dual"))]
pub struct Loopback {
    /// Both master reads completed (bus not wedged — i.e. the jumpers are in and the slave answered).
    pub ran: bool,
    /// Byte read back from `DBG_IFACE` (0x30) — expect [`regmap::DBG_IFACE_MAGIC`].
    pub iface: u8,
    /// Byte read back from `DBG_FW_VER_MAJOR` (0x3B) — expect [`crate::FW_VER_MAJOR`].
    pub ver_major: u8,
}

#[cfg(all(feature = "console", feature = "_dual"))]
impl Loopback {
    /// PASS = both reads landed and returned the expected identity magic + version-major.
    pub fn passed(&self) -> bool {
        self.ran && self.iface == regmap::DBG_IFACE_MAGIC && self.ver_major == crate::FW_VER_MAJOR
    }
}

/// Operator hint naming the no-SoM loopback jumpers — the chip's Stem (B1) ↔ Leaf (B0) pin pairing.
/// Chip-specific because the B1 pins differ (see [`hw`]). Used by both the boot self-test and `l`.
#[cfg(all(feature = "console", feature = "_stem_p3"))]
pub const LOOPBACK_JUMPER_HINT: &str = "P3.2->P1.2, P3.6->P1.3";
#[cfg(all(feature = "console", feature = "_stem_p4"))]
pub const LOOPBACK_JUMPER_HINT: &str = "P4.6->P1.2, P4.7->P1.3";

/// Bench self-test with NO SoM: jumper the Stem (B1) pins to the Leaf (B0) pins so the eUSCI_B0 MASTER
/// shares a bus with this eUSCI_B1 SLAVE, then have the master READ our own slave surface. This proves
/// the slave answers a real master (not merely that it builds and boots). Jumpers are **chip-specific**
/// (the B1 pins differ): FR247x = **P3.2→P1.2, P3.6→P1.3**; FR23xx/FR21xx = **P4.6→P1.2, P4.7→P1.3**.
///
/// It's a single MCU, so a plain blocking master read would DEADLOCK — the slave clock-stretches SCL
/// waiting for firmware to service its RX/TX while that same firmware is blocked in the master read. We
/// break the deadlock by pumping [`poll`] inside every master spin-wait (`i2c::read_reg_pumped`), so
/// the slave side is serviced between the master's byte phases.
///
/// Requires **Sensing** (B0 must be up as master). With no jumpers the master reads just time out →
/// `ran == false` (harmless). Console/bench only.
#[cfg(all(feature = "console", feature = "_dual"))]
pub fn loopback_selftest(p: &Peripherals, s: &mut Slave, rf: &mut RegFile) -> Loopback {
    let addr = STEM_ADDR as u8;
    let mut iface = [0u8; 1];
    let r1 = crate::i2c::read_reg_pumped(p, addr, regmap::DBG_IFACE, &mut iface, || poll(p, s, rf));
    let mut ver = [0u8; 1];
    let r2 =
        crate::i2c::read_reg_pumped(p, addr, regmap::DBG_FW_VER_MAJOR, &mut ver, || poll(p, s, rf));
    Loopback {
        ran: r1 && r2,
        iface: iface[0],
        ver_major: ver[0],
    }
}
