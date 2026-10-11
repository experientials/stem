//! Single-button input for diag — the LP-MSP430FR2476 **S1** user button.
//!
//! Pin is authoritative from the LaunchPad user's guide (SLAU802, Fig. 19 "Pushbuttons"):
//! **S1 = P4.0** (net `P4.0_S1`), active-low with a 47 kΩ external pull-up to 3V3. We also enable
//! the internal pull-up (harmless in parallel). One button is all diag needs: it cycles
//! POST → Margin → Soak → POST (see `tasks::ButtonTask`). (S2 = P2.3 exists on the board but is
//! unused; S3 = RST.)
//!
//! Reading is a bare pin sample; debounce + edge detection live in `tasks::ButtonTask`.

use crate::pac::Peripherals;

const S1_BIT: u8 = 0x01; // P4.0

/// Configure S1 as a GPIO input with pull-up (active-low).
pub fn init(p: &Peripherals) {
    p.p4.p4dir().modify(|r, w| unsafe { w.bits(r.bits() & !S1_BIT) }); // input
    p.p4.p4out().modify(|r, w| unsafe { w.bits(r.bits() | S1_BIT) }); // OUT=1 selects pull-UP
    p.p4.p4ren().modify(|r, w| unsafe { w.bits(r.bits() | S1_BIT) }); // enable the resistor
}

/// True while S1 (P4.0) is held (active-low → pin reads 0).
pub fn s1(p: &Peripherals) -> bool {
    p.p4.p4in().read().bits() & S1_BIT == 0
}
