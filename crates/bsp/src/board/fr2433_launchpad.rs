//! FR2433 LaunchPad (MSP-EXP430FR2433) target — the single-I²C, slave-only part (no Leaf master).
//! Backchannel UART = eUSCI_A0 on P1.4/P1.5 (same pins as the FR2476 board), but the FR2433 PAC is
//! mode-split so the instance is `usci_a0_uart_mode` (methods are the standard `uca0*`). FACTORY DCO
//! trim (like the FR2355 family — the manual trim doesn't lock here). No `usec`/`ms` base: this part
//! is the slave surface with no cooperative scheduler, so the time bases live only on `_leaf` families.
//!
//! Previously these lived inline in prod (`prod/clock.rs` fr24xx path, `prod/uart.rs` fr24xx console),
//! because bsp had no fr2433 module — folding them here is the CONV-1 completion (ADR-0014).

use crate::pac::Peripherals;

// Clock-system bits.
const SELREF_REFOCLK: u16 = 0x0010;
const DCORSEL_0: u16 = 0x0000;
const FLLD_0: u16 = 0x0000;
const SELMS_DCOCLKDIV: u16 = 0x0000;
const SELA_REFOCLK: u16 = 0x0100;
const FLLUNLOCK: u16 = 0x0300; // FLLUNLOCK0 | FLLUNLOCK1 (CSCTL7)

/// MCLK = SMCLK = DCODIV = 1 MHz (FLL ref = REFO), ACLK = REFO. FACTORY DCO trim (DCORSEL range only,
/// no DCOFTRIMEN) — same as the FR2355 family; the FR2476 manual trim does not lock this FLL. The lock
/// wait is bounded so a never-locking FLL reports a garbled clock rather than hanging boot.
pub fn clock_init_1mhz(p: &Peripherals) {
    unsafe { core::arch::asm!("bis #0x40, r2", options(nomem, nostack)) }; // SCG0=1: FLL off
    p.cs.csctl3().modify(|r, w| unsafe { w.bits(r.bits() | SELREF_REFOCLK) });
    p.cs.csctl1().write(|w| unsafe { w.bits(DCORSEL_0) }); // factory trim: range select only
    p.cs.csctl2().write(|w| unsafe { w.bits(FLLD_0 + 30) }); // FLLN=30 -> 1 MHz
    msp430::asm::nop();
    msp430::asm::nop();
    msp430::asm::nop();
    unsafe { core::arch::asm!("bic #0x40, r2", options(nomem, nostack)) }; // SCG0=0: FLL on
    let mut spins = 0u16;
    while p.cs.csctl7().read().bits() & FLLUNLOCK != 0 {
        spins += 1;
        if spins > 20000 {
            break; // bounded — don't hang boot if lock never asserts
        }
    }
    p.cs.csctl4().write(|w| unsafe { w.bits(SELMS_DCOCLKDIV | SELA_REFOCLK) });
    for _ in 0..8000u16 {
        msp430::asm::nop(); // let the DCO settle before clocking the UART
    }
}

// Console-UART pins: P1.4/P1.5 -> UCA0 (SEL1:SEL0 = 01). Distinct from UCB0 (P1.2/P1.3) — no clash.
const P1_UART_PINS: u8 = 0x30; // BIT4|BIT5
pub fn route_console_uart_pins(p: &Peripherals) {
    p.p1.p1sel1().modify(|r, w| unsafe { w.bits(r.bits() & !P1_UART_PINS) }); // SEL1=0
    p.p1.p1sel0().modify(|r, w| unsafe { w.bits(r.bits() | P1_UART_PINS) }); // SEL0=1
}

// Stem-I²C (slave uplink) pins: the ONE eUSCI_B0 on P1.2(SDA)/P1.3(SCL), SEL1:SEL0 = 01 — CONFIRMED
// against the FR2433 datasheet (SLASE59F Table 6-17): P1.2 SELx=01 → UCB0SDA, P1.3 SELx=01 → UCB0SCL.
// This IS the node's whole bus (no Leaf master on fr2433). Called from `bsp::stem_i2c::init`.
const P1_STEM_I2C_PINS: u8 = 0x0C; // BIT2 (P1.2=UCB0SDA) | BIT3 (P1.3=UCB0SCL)
pub fn route_stem_i2c_pins(p: &Peripherals) {
    p.p1.p1sel1().modify(|r, w| unsafe { w.bits(r.bits() & !P1_STEM_I2C_PINS) }); // SEL1=0
    p.p1.p1sel0().modify(|r, w| unsafe { w.bits(r.bits() | P1_STEM_I2C_PINS) }); // SEL0=1
}

// Console UART = eUSCI_A0, 9600 8N1 @ 1 MHz SMCLK. FR2433 PAC is mode-split → `usci_a0_uart_mode`.
const UCSWRST: u16 = 0x0001;
const UCSSEL_SMCLK: u16 = 0x0080;
const UCOS16: u16 = 0x0001;
const UCTXIFG: u16 = 0x0002;
const UCRXIFG: u16 = 0x0001;

/// FR2433's eUSCI_A0 — PAC field `usci_a0_uart_mode`; register methods are the standard `uca0*`.
macro_rules! uca0 {
    ($p:expr) => {
        $p.usci_a0_uart_mode
    };
}

pub fn console_uart_init(p: &Peripherals) {
    uca0!(p).uca0ctlw0().write(|w| unsafe { w.bits(UCSWRST) });
    uca0!(p)
        .uca0ctlw0()
        .modify(|r, w| unsafe { w.bits(r.bits() | UCSSEL_SMCLK) });
    uca0!(p).uca0brw().write(|w| unsafe { w.bits(6) });
    // UCBRSx=0x20, UCBRFx=8, UCOS16=1 (TI baud table, 1 MHz / 9600).
    uca0!(p)
        .uca0mctlw()
        .write(|w| unsafe { w.bits(0x2000 | (8 << 4) | UCOS16) });
    uca0!(p)
        .uca0ctlw0()
        .modify(|r, w| unsafe { w.bits(r.bits() & !UCSWRST) });
}

#[inline]
pub fn console_uart_tx(p: &Peripherals, c: u8) {
    while uca0!(p).uca0ifg().read().bits() & UCTXIFG == 0 {}
    uca0!(p).uca0txbuf().write(|w| unsafe { w.bits(c as u16) });
}

#[allow(dead_code)]
pub fn console_uart_rx_ready(p: &Peripherals) -> bool {
    uca0!(p).uca0ifg().read().bits() & UCRXIFG != 0
}
#[allow(dead_code)]
pub fn console_uart_getc(p: &Peripherals) -> u8 {
    uca0!(p).uca0rxbuf().read().bits() as u8
}
