//! RCWL-0516 microwave Doppler radar — presence/motion readout.
//!
//! One digital OUT line wired to P2.4 (see ../../crates/bsp/connections.toml `rcwl_out`). OUT idles
//! LOW and is driven HIGH (~3.3 V, push-pull) while motion is detected, held ~2 s per trigger and
//! retriggerable. VIN is 5 V; the 3V3 (regulator tap, an OUTPUT) and CDS pins are left open.
//!
//! P2.4 is a port-interrupt / wake-from-LPMx.5 pin (datasheet pin table), so the supervisor can
//! eventually wake on motion instead of polling — the reason the radar lives on Port 2 rather than
//! an ADC pin. Diag itself only *reads* it: this is a *presence* readout, not a pass/fail device
//! (nothing to ACK or ID), so each POST pass reports the instantaneous line level. A trigger holds
//! OUT high ~2 s and the POST cycle repeats every ~3 s, so a hand-wave shows on at least one pass.
//! `main` configures the pin as GPIO input with a pulldown, so an unplugged sensor reads idle
//! (LOW) rather than floating.

use crate::pac::Peripherals;

/// P2.4 — RCWL-0516 OUT. Kept here (not in `main`) so the pin mask lives next to its reader.
pub const RCWL_OUT: u8 = 0x10; // BIT4

/// True while the radar asserts motion (OUT high).
pub fn motion(p: &Peripherals) -> bool {
    p.p2.p2in().read().bits() & RCWL_OUT != 0
}
