//! Stem-bus I²C **slave** dispatch — the protocol/register-file layer ABOVE the transport. It maps a
//! master's PCA9698 command-pointer + read/write bytes onto [`regmap`] via [`RegFile`], consuming the
//! [`SlaveEvent`]s the `bsp::stem_i2c` transport decodes from the eUSCI_B IFG. The transport (eUSCI
//! programming, pins, IFG, and — Phase 1b — the wake ISR) is `bsp::stem_i2c`; this file is the
//! chip-agnostic dispatch, so prod and diag drive the same register surface (DEEPSLEEP-EXTRACTION.md §A).
//!
//! **What's backed today:** the debug/identity window (0x30–0x3F) is REAL — served by
//! [`Status::read_reg`], so the SoM reads exactly what the bench UART dump shows (identity, build id,
//! status, dev-count, fault). The PCA9698 GPIO banks (IP/OP/IOC/PI/MSK) are a coherent **firmware
//! shadow** — writes persist and read back; on fr2433 OP/IOC/IP also touch real P1/P2 pins (via the
//! [`GpioBanks`] impl in `bsp::node_hw`), on the dual node the banks are still shadow (no bank↔port map
//! yet). Voltages (0x2C/0x2D)
//! read 0 until the rail ADC lands. This is the wire contract, live, minus the not-yet-defined pins.

use crate::regmap::{self, ShadowState};
use crate::status::Status;
// The hardware seam is now the host-clean `node-hal` traits (DEEPSLEEP-EXTRACTION.md Phase 6a) — this file
// names NO concrete PAC. `bsp::StemHw` is the sole MSP430 impl; the bin constructs it and passes `&hw`.
use node_hal::{GpioBanks, SlaveEvent, StemSlave};
#[cfg(all(feature = "console", feature = "_dual"))]
use node_hal::LeafMaster;

/// Bring the node's slave transport up at the role-based `addr` (+ route its pins), generic over the
/// hardware seam. Thin wrapper over [`StemSlave::init`] so call sites keep `stem::init(&hw, addr)`.
pub fn init<H: StemSlave>(hw: &H, addr: u16) {
    hw.init(addr);
}

// The I²C-slave address is **ROLE-BASED** — `stembus::role::role_for_model(..).addr()`, threaded into
// `init`/`loopback_selftest` as `addr` (PCA9698 group 0x20–0x27). See I2C-API.md "Stem address is
// ROLE-BASED": FR2355/FR2155=0x20 (production supervisor), FR2476/FR2475=0x21 (dev stand-in),
// FR2433=0x22 (I/O-expander placeholder).

// The physical GPIO bank backing (PCA9698 IP/OP/IOC → real MSP430 port registers) moved DOWN into `bsp`
// (Phase 6a): it's the chip's port map, so it lives in `bsp::node_hw`'s [`node_hal::GpioBanks`] impl for
// `StemHw` (fr2433 drives P1/P2; dual stays a no-op/0 shadow). This file reaches it only through the trait.

/// The register file behind the slave: the debug/identity [`Status`] (real) plus the PCA9698 GPIO
/// shadow (coherent, not yet pin-backed on dual). One `read`/`write` maps a decoded [`regmap::RegSel`].
pub struct RegFile {
    /// Debug/identity/status snapshot (0x30–0x3F). Replace on each rescan so the SoM sees fresh state.
    pub status: Status,
    shadow: ShadowState,
    op: [u8; regmap::BANKS as usize],  // Output Port (0x08–0x0F)
    ioc: [u8; regmap::BANKS as usize], // I/O Config / direction (0x18–0x1F)
    /// Pending SoM-requested operating-mode switch (a `MODE_CTRL`/0x3E write). Applied by the main
    /// loop AFTER the transaction — the switch does sensor-bus acquire/release, which must not run
    /// inside dispatch (it'd re-enter the bus mid-transfer). The CURRENT mode is read back via
    /// `status.mode` (kept live by `Status::set_mode`), so reads need no special-casing here.
    mode_request: Option<u8>,
    /// Per-sector FRAM CRC16 (ADR-0010): `[sec0_hi, sec0_lo, …]`, computed once at boot by
    /// `post::fram_crcs` and set via [`set_crc`]. Served read-only over I²C at `DBG_CRC` (0x3F) through
    /// a windowed cursor (the MSP only reports; the M7 judges).
    crc: [u8; regmap::DBG_CRC_BYTES],
    /// Byte index selected by a master WRITE to `DBG_CRC` (0x3F); a READ returns `crc[crc_cursor]`.
    crc_cursor: u8,
}

impl RegFile {
    pub fn new(status: Status) -> Self {
        Self {
            status,
            shadow: ShadowState::default(),
            op: [0; regmap::BANKS as usize],
            ioc: [0; regmap::BANKS as usize],
            mode_request: None,
            crc: [0; regmap::DBG_CRC_BYTES],
            crc_cursor: 0,
        }
    }

    /// Store the boot-computed per-sector FRAM CRCs (from `post::fram_crcs`) for the M7 to read at 0x3F.
    pub fn set_crc(&mut self, crc: [u8; regmap::DBG_CRC_BYTES]) {
        self.crc = crc;
    }

    /// Take any pending SoM-requested mode code, clearing it. The main loop decodes it via
    /// `mode::Mode::from_code` and applies the switch through the mode machine.
    pub fn take_mode_request(&mut self) -> Option<u8> {
        self.mode_request.take()
    }

    /// Read the byte a master gets for command index `reg`. `hw` drives the physical IP read on fr2433
    /// (via [`GpioBanks`]); it's unused on the dual node, whose banks are still shadow.
    fn read<H: GpioBanks>(&self, hw: &H, reg: u8) -> u8 {
        #[cfg(feature = "_dual")]
        let _ = hw;
        // CRC readout window (ADR-0010): a prior WRITE to 0x3F set the byte cursor; return that CRC byte.
        // Intercept before the generic Extended→status path (0x3F would otherwise read as reserved 0).
        if reg == regmap::DBG_CRC {
            return self.crc[self.crc_cursor as usize % regmap::DBG_CRC_BYTES];
        }
        match regmap::decode(reg) {
            // Debug/identity window — the real state (identical to the bench UART dump).
            regmap::RegSel::Extended(_) => self.status.read_reg(reg),
            // Input Port — physical PxIN (XOR polarity) on fr2433; 0 on the still-shadow dual banks.
            regmap::RegSel::InputPort(b) => self.read_ip(hw, b),
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
    // array access, no bounds-check path); fr2433 applies the polarity shadow to the live PxIN.
    #[cfg(feature = "fr2433")]
    #[inline]
    fn read_ip<H: GpioBanks>(&self, hw: &H, bank: u8) -> u8 {
        hw.read_ip(bank, self.shadow.polarity[bank as usize])
    }
    #[cfg(feature = "_dual")]
    #[inline]
    fn read_ip<H: GpioBanks>(&self, _hw: &H, _bank: u8) -> u8 {
        0
    }

    /// Apply a master write of `val` to command index `reg`. Read-only regions are ignored. On fr2433,
    /// OP/IOC also drive the physical port (PxOUT / PxDIR) via [`GpioBanks`]; the shadow is still kept so a
    /// read-back returns the last written value regardless of pin backing.
    fn write<H: GpioBanks>(&mut self, hw: &H, reg: u8, val: u8) {
        #[cfg(feature = "_dual")]
        let _ = hw;
        // Operating-mode control (0x3E): record the request; the main loop applies it after the
        // transaction (decode(0x3E) is Extended, which is otherwise read-only → this must precede it).
        if reg == regmap::MODE_CTRL {
            self.mode_request = Some(val);
            return;
        }
        // CRC window cursor (0x3F, ADR-0010): a WRITE selects which CRC byte the next READ returns.
        // Data is read-only (boot-computed); the write only moves the cursor.
        if reg == regmap::DBG_CRC {
            self.crc_cursor = val;
            return;
        }
        match regmap::decode(reg) {
            regmap::RegSel::OutputPort(b) => {
                self.op[b as usize] = val;
                hw.drive_op(b, val); // no-op on dual
            }
            regmap::RegSel::IoConfig(b) => {
                self.ioc[b as usize] = val;
                hw.set_dir(b, val); // no-op on dual
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

    /// Apply one decoded [`SlaveEvent`] to the register file (PCA9698 command-pointer protocol,
    /// I2C-API.md §7.3). The single source of the dispatch logic — shared by the sync [`poll`] drain
    /// (dual) and the async [`serve`] task (fr2433), so the two can't drift.
    fn handle<H: StemSlave + GpioBanks>(&mut self, hw: &H, rf: &mut RegFile, ev: SlaveEvent) {
        match ev {
            // START matching our address: the next RX byte is the command; a read keeps the pointer.
            SlaveEvent::Start => self.first_byte = true,
            SlaveEvent::Nack => {}
            SlaveEvent::Rx(b) => {
                if self.first_byte {
                    self.reg_ptr = b & regmap::CMD_REG_MASK;
                    self.ai = b & regmap::CMD_AI != 0;
                    self.first_byte = false;
                } else {
                    rf.write(hw, self.reg_ptr, b);
                    if self.ai {
                        self.reg_ptr = regmap::next_ai(self.reg_ptr);
                    }
                }
            }
            SlaveEvent::TxReq => {
                let v = rf.read(hw, self.reg_ptr);
                hw.provide_tx(v);
                if self.ai {
                    self.reg_ptr = regmap::next_ai(self.reg_ptr);
                }
            }
            SlaveEvent::Stop => self.first_byte = true,
        }
    }
}

/// Service any pending slave events (non-blocking). Call frequently from the main loop (Phase 1a); from
/// Phase 1b it runs when the eUSCI_B ISR wakes the CPU from LPM. Drains the `bsp::stem_i2c` event seam.
///
/// Protocol (PCA9698, I2C-API.md §7.3): after our address, the master's FIRST written byte is the
/// command register — low 6 bits = pointer, [`regmap::CMD_AI`] = auto-increment. Subsequent written
/// bytes go to the pointed register (stepping the bank under AI); a master read returns the pointed
/// register (also stepping under AI).
pub fn poll<H: StemSlave + GpioBanks>(hw: &H, s: &mut Slave, rf: &mut RegFile) {
    while let Some(ev) = hw.next_event() {
        s.handle(hw, rf, ev);
    }
}

/// **Async** dispatch (Phase 1c, ADR-0013 §5 LPM pender) — the same register surface as [`poll`], driven
/// as a never-completing `sched` task. `.await`s one [`SlaveEvent`] at a time: when the bus is idle the
/// leaf returns `Pending`, so `sched::run_pended` runs the caller's LPM `idle` closure (sleep); the
/// eUSCI slave ISR wakes the CPU + `Pender::wake()`s, and the next poll drains the event. Functionally
/// identical to the `poll` drain — it only changes *when* the CPU runs (interrupt-driven sleep, not a
/// busy loop). Used by the fr2433 Passive node; dual still uses [`poll`] until its async reshape.
pub async fn serve<H: StemSlave + GpioBanks>(hw: &H, s: &mut Slave, rf: &mut RegFile) {
    loop {
        // Leaf: resolve to the next slave event, or `Pending` when the bus is idle (→ executor idles).
        let ev = core::future::poll_fn(|_cx| match hw.next_event() {
            Some(ev) => core::task::Poll::Ready(ev),
            None => core::task::Poll::Pending,
        })
        .await;
        s.handle(hw, rf, ev);
    }
}

/// Bench self-test for the physical GPIO backing WITHOUT an I²C master (fr2433 has no way to be its
/// own master): drive **bank 0 / bit 0 (P1.0)** through the SAME [`RegFile`] path a SoM master would
/// use, and read it back on PxIN. Configure P1.0 as output (IOC bit0=0), drive high then low (OP),
/// sense IP each time. PASS = IP bit0 tracks the driven level — proving IOC→PxDIR, OP→PxOUT and
/// IP←PxIN all work end-to-end. Leaves P1.0 driven low. On the LaunchPad P1.0 is LED1, so a PASS also
/// blinks it. Console/bench only.
#[cfg(all(feature = "fr2433", feature = "console"))]
pub fn gpio_selftest<H: GpioBanks>(hw: &H, rf: &mut RegFile) -> bool {
    // IOC: 1=input, 0=output (PCA9698). 0xFE → bank-0 bit0 output, the rest input.
    rf.write(hw, regmap::IOC_BASE, 0xFE);
    rf.write(hw, regmap::OP_BASE, 0x01); // P1.0 high
    let hi = rf.read(hw, regmap::IP_BASE) & 0x01;
    rf.write(hw, regmap::OP_BASE, 0x00); // P1.0 low
    let lo = rf.read(hw, regmap::IP_BASE) & 0x01;
    hi == 0x01 && lo == 0x00
}

/// Result of the B0↔B1 loopback self-test ([`loopback_selftest`]). DUAL-only: needs the eUSCI_B0
/// sensor-master to drive the eUSCI_B1 slave on the same MCU; fr2433 has only the one (slave) bus.
#[cfg(all(feature = "console", feature = "_dual"))]
pub struct Loopback {
    /// Both master reads completed (bus not wedged — i.e. the jumpers are in and the slave answered).
    pub ran: bool,
    /// Byte read back from `DBG_IFACE` (0x30) — expect [`regmap::DBG_IFACE_MAGIC`].
    pub iface: u8,
    /// Byte read back from `DBG_FW_VER_MAJOR` (0x3B) — expect [`crate::VER_MAJOR`].
    pub ver_major: u8,
}

#[cfg(all(feature = "console", feature = "_dual"))]
impl Loopback {
    /// PASS = both reads landed and returned the expected identity magic + version-major.
    pub fn passed(&self) -> bool {
        self.ran && self.iface == regmap::DBG_IFACE_MAGIC && self.ver_major == crate::VER_MAJOR
    }
}

/// Operator hint naming the no-SoM loopback jumpers — the chip's Stem (B1) ↔ Leaf (B0) pin pairing.
/// Chip-specific because the B1 pins differ (see `bsp::board::route_stem_i2c_pins`). Used by both the
/// boot self-test and the `l` command.
#[cfg(all(feature = "console", feature = "fr2476"))]
pub const LOOPBACK_JUMPER_HINT: &str = "P3.2->P1.2, P3.6->P1.3";
#[cfg(all(feature = "console", any(feature = "fr2355", feature = "fr2155")))]
pub const LOOPBACK_JUMPER_HINT: &str = "P4.6->P1.2, P4.7->P1.3";

/// Bench self-test with NO SoM: jumper the Stem (B1) pins to the Leaf (B0) pins so the eUSCI_B0 MASTER
/// shares a bus with this eUSCI_B1 SLAVE, then have the master READ our own slave surface. This proves
/// the slave answers a real master (not merely that it builds and boots). Jumpers are **chip-specific**
/// (the B1 pins differ): FR2476 = **P3.2→P1.2, P3.6→P1.3**; FR2355/FR2155 = **P4.6→P1.2, P4.7→P1.3**.
///
/// It's a single MCU, so a plain blocking master read would DEADLOCK — the slave clock-stretches SCL
/// waiting for firmware to service its RX/TX while that same firmware is blocked in the master read. We
/// break the deadlock by pumping [`poll`] inside every master spin-wait (`bsp::i2c::read_reg_pumped`),
/// so the slave side is serviced between the master's byte phases.
///
/// Requires **Sensing** (B0 must be up as master). With no jumpers the master reads just time out →
/// `ran == false` (harmless). Console/bench only.
#[cfg(all(feature = "console", feature = "_dual"))]
pub fn loopback_selftest<H: StemSlave + GpioBanks + LeafMaster>(
    hw: &H,
    s: &mut Slave,
    rf: &mut RegFile,
    addr: u16,
) -> Loopback {
    let addr = addr as u8;
    let mut iface = [0u8; 1];
    let r1 = hw.read_reg_pumped(addr, regmap::DBG_IFACE, &mut iface, &mut || poll(hw, s, rf));
    let mut ver = [0u8; 1];
    let r2 =
        hw.read_reg_pumped(addr, regmap::DBG_FW_VER_MAJOR, &mut ver, &mut || poll(hw, s, rf));
    Loopback {
        ran: r1 && r2,
        iface: iface[0],
        ver_major: ver[0],
    }
}
