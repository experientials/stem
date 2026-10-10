//! COMBINED target (`fr2xxx`) — ONE board module covering the FR2476 LaunchPad **and** the
//! FR2355/FR2155 LaunchPad in a single image. It is the runtime-dispatching union of
//! `fr2476_launchpad.rs` + `fr2355_launchpad.rs`: for the handful of peripherals whose base/value/pins
//! diverge across the family it selects by `crate::model::detect()`; for everything else it uses the
//! typed `crate::pac` (the combined PAC is FR2476-shaped, and those peripherals sit at identical
//! addresses on both parts — verified per-datasheet, see `model::stem_i2c_base` / STEM-DIRECTION.md).
//!
//! The three divergences handled here (the full set prod touches):
//!   • **CSCTL1 clock trim** — FR2476 uses an explicit manual DCO trim; FR2355 uses the factory trim
//!     (the FR2476 values do NOT lock the FR2355 FLL — bench 2026-09-25, FR2355-SCOPE.md). Same
//!     register (CS @ 0x0180 on both), different value → runtime select. Lock-wait is bounded for both.
//!   • **Console UART** — eUSCI_A0 @ 0x0500 on P1.4/P1.5 (FR2476) vs eUSCI_A1 @ 0x0580 on P4.2/P4.3
//!     (FR2355; SLAU680 §2.2.4). eUSCI_A layout is identical, so we overlay the register block on the
//!     model-selected base + route the model's pins. (Dev-only; the dark prod image has `console` off.)
//!   • **Stem-I²C pins** — UCB1 on P3.2/P3.6 (FR2476) vs P4.6/P4.7 (FR2355/FR2155). The UCB1 *base*
//!     divergence is handled in `stem_i2c` (`_dynb1`); only the PIN mux is here.
//!
//! The µs/ms timers are deliberately ABSENT: prod deepsleep is event-driven (LPM + eUSCI_B1 wake) and
//! uses no `usec_*`, so the Timer_A(FR2476)/Timer_B(FR2355) instance+type divergence never arises on
//! the combined image. (The ms base in `board::mod` is `_leaf`-gated and prod-unused — dead-stripped.)

use crate::model::{detect, Model};
use crate::pac::Peripherals;

// ── Clock-system bits (shared across the family; CS @ 0x0180 on both parts). ──
const SELREF_REFOCLK: u16 = 0x0010;
const DCOFTRIMEN: u16 = 0x0080;
const DCOFTRIM0: u16 = 0x0010;
const DCOFTRIM1: u16 = 0x0020;
const DCORSEL_0: u16 = 0x0000;
const FLLD_0: u16 = 0x0000;
const SELMS_DCOCLKDIV: u16 = 0x0000;
const SELA_REFOCLK: u16 = 0x0100;
const FLLUNLOCK: u16 = 0x0300; // FLLUNLOCK0 | FLLUNLOCK1 (CSCTL7)

/// True when the detected silicon wants the FR2355 **factory** DCO trim (range-select only). FR247x
/// (and the fallback) use the explicit manual trim. This is the ONLY per-model clock delta.
#[inline]
fn factory_trim() -> bool {
    matches!(detect(), Model::Fr2355 | Model::Fr2155)
}

/// MCLK = SMCLK = DCODIV = 1 MHz (FLL ref = REFO), ACLK = REFO. Same sequence as the per-family boards;
/// the CSCTL1 value is chosen at runtime (manual trim vs factory trim), and the lock-wait is bounded so
/// a never-locking FLL reaches UART/bootrec instead of deadlocking (FR2355-SCOPE.md).
pub fn clock_init_1mhz(p: &Peripherals) {
    unsafe { core::arch::asm!("bis #0x40, r2", options(nomem, nostack)) }; // SCG0=1: FLL off
    p.cs.csctl3().modify(|r, w| unsafe { w.bits(r.bits() | SELREF_REFOCLK) });
    let csctl1 = if factory_trim() {
        DCORSEL_0 // FR2355/FR2155: factory trim — range select only
    } else {
        DCOFTRIMEN | DCOFTRIM0 | DCOFTRIM1 | DCORSEL_0 // FR247x: explicit manual DCO trim
    };
    p.cs.csctl1().write(|w| unsafe { w.bits(csctl1) });
    p.cs.csctl2().write(|w| unsafe { w.bits(FLLD_0 + 30) }); // FLLN=30 -> 1 MHz
    msp430::asm::nop();
    msp430::asm::nop();
    msp430::asm::nop();
    unsafe { core::arch::asm!("bic #0x40, r2", options(nomem, nostack)) }; // SCG0=0: FLL on
    let mut spins = 0u16;
    while p.cs.csctl7().read().bits() & FLLUNLOCK != 0 {
        spins += 1;
        if spins > 20000 {
            break; // bounded — don't hang boot if lock never asserts (bootrec records the state)
        }
    }
    p.cs.csctl4().write(|w| unsafe { w.bits(SELMS_DCOCLKDIV | SELA_REFOCLK) });
    for _ in 0..8000u16 {
        msp430::asm::nop(); // let the DCO settle before clocking the UART
    }
}

// ── Stem-I²C (slave uplink) pins. UCB1 base is runtime-selected in `stem_i2c` (`_dynb1`); here we only
// route the model's pins: FR2476 = P3.2/P3.6, FR2355/FR2155 = P4.6/P4.7 (verified per-datasheet). ──
const P3_STEM_I2C_PINS: u8 = 0x44; // FR2476: BIT2 (P3.2=UCB1SDA) | BIT6 (P3.6=UCB1SCL)
const P4_STEM_I2C_PINS: u8 = 0xC0; // FR2355: BIT6 (P4.6=UCB1SDA) | BIT7 (P4.7=UCB1SCL)
pub fn route_stem_i2c_pins(p: &Peripherals) {
    match detect() {
        Model::Fr2355 | Model::Fr2155 => {
            p.p4.p4sel1().modify(|r, w| unsafe { w.bits(r.bits() & !P4_STEM_I2C_PINS) }); // SEL1=0
            p.p4.p4sel0().modify(|r, w| unsafe { w.bits(r.bits() | P4_STEM_I2C_PINS) }); // SEL0=1
        }
        // FR247x explicitly, and the fallback for an unexpected id on a dual board.
        _ => {
            p.p3.p3sel1().modify(|r, w| unsafe { w.bits(r.bits() & !P3_STEM_I2C_PINS) }); // SEL1=0
            p.p3.p3sel0().modify(|r, w| unsafe { w.bits(r.bits() | P3_STEM_I2C_PINS) }); // SEL0=1
        }
    }
}

// ── Console UART (dev-only). Base + pins diverge; eUSCI_A register layout is identical, so overlay the
// `e_usci_a0::RegisterBlock` on the model-selected base — the same overlay trick as `stem_i2c`. ──
use crate::pac::e_usci_a0::RegisterBlock as EUsciA;
const A_BASE_FR2476: usize = 0x0500; // eUSCI_A0 (both parts)
const A_BASE_FR2355: usize = 0x0580; // eUSCI_A1 on FR2355/FR2155 (where FR2476's eUSCI_B1 lives)

/// Overlay the typed eUSCI_A register block on the console instance for the detected model.
#[inline]
fn console_a() -> &'static EUsciA {
    let base = match detect() {
        Model::Fr2355 | Model::Fr2155 => A_BASE_FR2355,
        _ => A_BASE_FR2476,
    };
    // SAFETY: valid eUSCI_A base for the detected silicon; layout matches the PAC type; single-core.
    unsafe { &*(base as *const EUsciA) }
}

const P1_UART_PINS: u8 = 0x30; // FR2476: BIT4|BIT5 (P1.4/P1.5 → UCA0)
const P4_UART_PINS: u8 = 0x0C; // FR2355: BIT2|BIT3 (P4.2/P4.3 → UCA1)
pub fn route_console_uart_pins(p: &Peripherals) {
    match detect() {
        Model::Fr2355 | Model::Fr2155 => {
            p.p4.p4sel1().modify(|r, w| unsafe { w.bits(r.bits() & !P4_UART_PINS) }); // SEL1=0
            p.p4.p4sel0().modify(|r, w| unsafe { w.bits(r.bits() | P4_UART_PINS) }); // SEL0=1
        }
        _ => {
            p.p1.p1sel1().modify(|r, w| unsafe { w.bits(r.bits() & !P1_UART_PINS) }); // SEL1=0
            p.p1.p1sel0().modify(|r, w| unsafe { w.bits(r.bits() | P1_UART_PINS) }); // SEL0=1
        }
    }
}

// Console UART = 9600 8N1 @ 1 MHz SMCLK (same config on both instances; only the base/pins differ).
const UCSWRST: u16 = 0x0001;
const UCSSEL_SMCLK: u16 = 0x0080;
const UCOS16: u16 = 0x0001;
const UCTXIFG: u16 = 0x0002;
const UCRXIFG: u16 = 0x0001;

pub fn console_uart_init(_p: &Peripherals) {
    let a = console_a();
    a.uca0ctlw0().write(|w| unsafe { w.bits(UCSWRST) });
    a.uca0ctlw0().modify(|r, w| unsafe { w.bits(r.bits() | UCSSEL_SMCLK) });
    a.uca0brw().write(|w| unsafe { w.bits(6) });
    // UCBRSx=0x20, UCBRFx=8, UCOS16=1 (TI baud table, 1 MHz / 9600).
    a.uca0mctlw().write(|w| unsafe { w.bits(0x2000 | (8 << 4) | UCOS16) });
    a.uca0ctlw0().modify(|r, w| unsafe { w.bits(r.bits() & !UCSWRST) });
}

#[inline]
pub fn console_uart_tx(_p: &Peripherals, c: u8) {
    let a = console_a();
    while a.uca0ifg().read().bits() & UCTXIFG == 0 {}
    a.uca0txbuf().write(|w| unsafe { w.bits(c as u16) });
}

/// Snapshot the console UART's config regs (CTLW0, BRW, MCTLW) for the SFR dump — reads the
/// model's backchannel instance so the dump is correct on whichever part the image booted.
pub fn console_uart_regs(_p: &Peripherals) -> (u16, u16, u16) {
    let a = console_a();
    (
        a.uca0ctlw0().read().bits(),
        a.uca0brw().read().bits(),
        a.uca0mctlw().read().bits(),
    )
}

/// Console-UART pin-mux check for the SFR dump: the model's PORT (SEL1, SEL0, DIR) + per-pin expectation.
/// FR2476 = UCA0 on P1.4/P1.5; FR2355/FR2155 = UCA1 on P4.2/P4.3 (SLAU680 §2.2.4).
pub fn console_uart_pin_report(
    p: &Peripherals,
) -> (u16, u16, u16, [(&'static str, u16, &'static str); 2]) {
    match detect() {
        Model::Fr2355 | Model::Fr2155 => (
            u16::from(p.p4.p4sel1().read().bits()),
            u16::from(p.p4.p4sel0().read().bits()),
            u16::from(p.p4.p4dir().read().bits()),
            [("P4.2", 0x04, "UCA1RXD (01)"), ("P4.3", 0x08, "UCA1TXD (01)")],
        ),
        _ => (
            u16::from(p.p1.p1sel1().read().bits()),
            u16::from(p.p1.p1sel0().read().bits()),
            u16::from(p.p1.p1dir().read().bits()),
            [("P1.4", 0x10, "UCA0TXD (01)"), ("P1.5", 0x20, "UCA0RXD (01)")],
        ),
    }
}

#[allow(dead_code)]
pub fn console_uart_rx_ready(_p: &Peripherals) -> bool {
    console_a().uca0ifg().read().bits() & UCRXIFG != 0
}
#[allow(dead_code)]
pub fn console_uart_getc(_p: &Peripherals) -> u8 {
    console_a().uca0rxbuf().read().bits() as u8
}
