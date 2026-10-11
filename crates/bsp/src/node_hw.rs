//! `StemHw` — the MSP430 newtype that `impl`s the host-clean `node-hal` hardware-seam traits
//! ([`StemSlave`], [`GpioBanks`], [`LeafMaster`]) for the `deepsleep` node dispatch (DEEPSLEEP-EXTRACTION.md
//! Phase 6a). The orphan rule forbids impl'ing a `node-hal` trait directly for the foreign PAC
//! `Peripherals`, so we wrap it: `StemHw(&p)`. `deepsleep::stem` is generic over these traits and names
//! no concrete PAC; the bin constructs `bsp::StemHw(p)` and passes `&hw`.
//!
//! - [`StemSlave`] delegates to the `stem_i2c` transport free fns (which stay — the bin's LPM/ISR code
//!   calls `bsp::stem_i2c::irq_{enable,disable}` directly).
//! - [`GpioBanks`] holds the chip-specific PCA9698 bank→port backing (moved here from
//!   `deepsleep/src/stem.rs`'s `mod gpio` — the fr2433 port map belongs in `bsp`). On the dual families
//!   the banks are still a firmware shadow, so the impl is the no-op/0 stub, byte-identical to before.
//! - [`LeafMaster`] (dual/`_leaf` only) delegates to the Leaf-I²C master [`crate::i2c::read_reg_pumped`].

use crate::pac::Peripherals;
use node_hal::{Console, GpioBanks, SlaveEvent, StemSlave};
#[cfg(feature = "_leaf")]
use node_hal::{LeafBus, LeafMaster};

/// MSP430 hardware handle for the `node-hal` seams — a thin newtype over the PAC `Peripherals` so the
/// orphan rule is satisfied and `deepsleep` stays PAC-free. Zero-cost (one shared reference).
pub struct StemHw<'a>(pub &'a Peripherals);

impl StemSlave for StemHw<'_> {
    #[inline]
    fn init(&self, addr: u16) {
        crate::stem_i2c::init(self.0, addr);
    }
    #[inline]
    fn next_event(&self) -> Option<SlaveEvent> {
        crate::stem_i2c::next_event(self.0)
    }
    #[inline]
    fn provide_tx(&self, v: u8) {
        crate::stem_i2c::provide_tx(self.0, v);
    }
    #[inline]
    fn irq_enable(&self) {
        crate::stem_i2c::irq_enable(self.0);
    }
    #[inline]
    fn irq_disable(&self) {
        crate::stem_i2c::irq_disable(self.0);
    }
}

// ── Console: the backchannel text console (delegates to `bsp::fmt`, Phase 6b). Always available. ──

impl Console for StemHw<'_> {
    #[inline]
    fn putc(&self, c: u8) {
        crate::fmt::putc(self.0, c);
    }
    #[inline]
    fn puts(&self, s: &str) {
        crate::fmt::puts(self.0, s);
    }
    #[inline]
    fn hex8(&self, v: u8) {
        crate::fmt::hex8(self.0, v);
    }
    #[inline]
    fn hex16(&self, v: u16) {
        crate::fmt::hex16(self.0, v);
    }
    #[inline]
    fn dec(&self, v: u16) {
        crate::fmt::dec(self.0, v);
    }
    #[inline]
    fn dec_i16(&self, v: i16) {
        crate::fmt::dec_i16(self.0, v);
    }
    #[inline]
    fn fixed2(&self, centi: i32) {
        crate::fmt::fixed2(self.0, centi);
    }
}

// ── GpioBanks: the PCA9698 bank→port backing (moved from deepsleep/src/stem.rs `mod gpio`). ──

// bsp's dual marker is `_leaf` (the Leaf-master bus) — the dual families set it, fr2433 does not. So the
// two GpioBanks impls are mutually exclusive (`_leaf` XOR `fr2433`), never a duplicate.
#[cfg(feature = "_leaf")]
impl GpioBanks for StemHw<'_> {
    // DUAL node: the board's bank↔port map isn't defined yet, so the banks stay a firmware shadow —
    // `read_ip` returns 0, `drive_op`/`set_dir` are no-ops, exactly as the former inline dual stubs.
    #[inline]
    fn read_ip(&self, _bank: u8, _pol: u8) -> u8 {
        0
    }
    #[inline]
    fn drive_op(&self, _bank: u8, _val: u8) {}
    #[inline]
    fn set_dir(&self, _bank: u8, _ioc: u8) {}
}

// FR2433 (24-pin RGE) bank→port backing — datasheet SLASE59F (Fig 4-1; per-pin SEL decode Tables 6-17 /
// 6-18). Bank 0 = P1, bank 1 = P2 (byte-wide ports). Bank 2 = P3 stays shadow: the PAC models P3 as
// per-bit registers and only P3.0–2 are bonded on VQFN-24. The USABLE masks exclude every pin with a
// reserved role so driving a bank NEVER disturbs a bus, the console, or the clock:
//   • P1.2/3 = UCB0 I²C slave (always); P1.4/5 = UCA0 console (Table 6-17). P1.6/7 = UCA0CLK/STE +
//     JTAG TDI/TDO — free as GPIO under the LaunchPad's 2-wire SBW (JTAG 4-wire not used).
//   • P2.0/1 = XOUT/XIN (LFXT crystal, Table 6-18) — EXCLUDED: driving a populated crystal's pins is a
//     hazard, and their GPIO use on the product is a BOM decision, not a default.
// PROVISIONAL pending an FR2433 connections.toml/BOM (which of the *remaining* bits are actually wired as
// GPIO on the product PCB); the masks below are the datasheet-safe superset.
#[cfg(feature = "fr2433")]
impl GpioBanks for StemHw<'_> {
    #[inline]
    fn read_ip(&self, bank: u8, pol: u8) -> u8 {
        let p = self.0;
        match bank {
            0 => (p.p1.p1in().read().bits() ^ pol) & P1_GPIO,
            1 => (p.p2.p2in().read().bits() ^ pol) & P2_GPIO,
            _ => 0, // bank 2 (P3) + unbacked → 0
        }
    }
    #[inline]
    fn drive_op(&self, bank: u8, val: u8) {
        let p = self.0;
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
    #[inline]
    fn set_dir(&self, bank: u8, ioc: u8) {
        // PCA9698 uses 1=input, 0=output → PxDIR = !IOC (backed bits only).
        let p = self.0;
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

#[cfg(feature = "fr2433")]
const P1_GPIO: u8 = 0xC3; // P1.0,1,6,7 — exclude UCB0 SDA/SCL (P1.2/3) + UCA0 TXD/RXD (P1.4/5)
#[cfg(feature = "fr2433")]
const P2_GPIO: u8 = 0xFC; // P2.2–7 — exclude XOUT/XIN (P2.0/1, LFXT crystal pins; Table 6-18)

// ── LeafMaster: the Leaf-I²C master register read (dual/`_leaf` only — fr2433 has no Leaf bus). ──

#[cfg(feature = "_leaf")]
impl LeafMaster for StemHw<'_> {
    #[inline]
    fn read_reg_pumped(&self, addr: u8, reg: u8, buf: &mut [u8], pump: &mut dyn FnMut()) -> bool {
        crate::i2c::read_reg_pumped(self.0, addr, reg, buf, pump)
    }
}

// ── LeafBus: the Leaf-I²C master bus seam enumerate/mode drive (dual/`_leaf` only — Phase 6b). The
// associated `Bus` is tied to the newtype's own lifetime `'a`, so `node-hal` needs no GAT: `self.0` is a
// `Copy` `&'a Peripherals`, so `EusciI2c::new(self.0)` yields `EusciI2c<'a>`. ──
#[cfg(feature = "_leaf")]
impl<'a> LeafBus for StemHw<'a> {
    type Bus = crate::hal::EusciI2c<'a>;
    #[inline]
    fn bus(&self) -> Self::Bus {
        crate::hal::EusciI2c::new(self.0)
    }
    #[inline]
    fn recover(&self) -> bool {
        crate::i2c::recover(self.0)
    }
    #[inline]
    fn bus_levels(&self) -> (bool, bool) {
        crate::i2c::bus_levels(self.0)
    }
    #[inline]
    fn acquire(&self) {
        crate::board::leaf_i2c_acquire(self.0);
    }
    #[inline]
    fn release(&self) {
        crate::board::leaf_i2c_release(self.0);
    }
    #[inline]
    fn leaf_debug_regs(&self) -> (u16, u16) {
        let p = self.0;
        (p.e_usci_b0.ucb0ctlw0().read().bits(), p.e_usci_b0.ucb0ifg().read().bits())
    }
}
