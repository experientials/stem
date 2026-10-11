//! FR2355 LaunchPad (MSP-EXP430FR2355) target.
//! Backchannel UART = eUSCI_A1 on P4.2(RXD)/P4.3(TXD) (SLAU680 §2.2.4 — NOT A0 like the FR2476
//! board); µs base = Timer_B (TB1, since FR2355 has no Timer_A); FACTORY DCO trim; 4 KB RAM.

use crate::pac::Peripherals;

// Clock-system bits.
const SELREF_REFOCLK: u16 = 0x0010;
const DCORSEL_0: u16 = 0x0000;
const FLLD_0: u16 = 0x0000;
const SELMS_DCOCLKDIV: u16 = 0x0000;
const SELA_REFOCLK: u16 = 0x0100;
const FLLUNLOCK: u16 = 0x0300; // FLLUNLOCK0 | FLLUNLOCK1 (CSCTL7)

/// MCLK = SMCLK = DCODIV = 1 MHz (FLL ref = REFO), ACLK = REFO.
///
/// FR2355 difference vs FR2476: use the **factory** DCO trim — select only the 1 MHz DCORSEL range,
/// do NOT set DCOFTRIMEN + manual DCOFTRIM bits. The FR2476 manual-trim values do not lock the
/// FR2355 FLL, so the lock loop would spin forever (bench 2026-09-25, FR2355-SCOPE.md). The lock
/// wait is also **bounded** so a never-locking FLL can't hang boot — we'd rather reach UART/bootrec
/// and report the fault than deadlock invisibly.
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
            break; // bounded — don't hang boot if lock never asserts (bootrec records the state)
        }
    }
    p.cs.csctl4().write(|w| unsafe { w.bits(SELMS_DCOCLKDIV | SELA_REFOCLK) });
    for _ in 0..8000u16 {
        msp430::asm::nop(); // let the DCO settle before clocking the UART
    }
}

// Console-UART pins: P4.2/P4.3 -> UCA1 (SEL1:SEL0 = 01). Leaf-I2C pins are routed in board::mod.
const P4_UART_PINS: u8 = 0x0C; // BIT2|BIT3
pub fn route_console_uart_pins(p: &Peripherals) {
    p.p4.p4sel1().modify(|r, w| unsafe { w.bits(r.bits() & !P4_UART_PINS) }); // SEL1=0
    p.p4.p4sel0().modify(|r, w| unsafe { w.bits(r.bits() | P4_UART_PINS) }); // SEL0=1
}

// Stem-I²C (slave uplink) pins: UCB1 on P4.6(SDA)/P4.7(SCL), SEL1:SEL0 = 01. Chip-specific — the
// UCB1↔pin mux differs from FR2476 (verified per-datasheet: FR2355 ds p24 signal + p102 select).
// Distinct bits from the console UART (P4.2/P4.3), so the read-modify-write doesn't disturb it.
// Serves FR2155 too (rides the FR2355 register superset). Called from `bsp::stem_i2c::init`.
const P4_STEM_I2C_PINS: u8 = 0xC0; // BIT6 (P4.6=UCB1SDA) | BIT7 (P4.7=UCB1SCL)
pub fn route_stem_i2c_pins(p: &Peripherals) {
    p.p4.p4sel1().modify(|r, w| unsafe { w.bits(r.bits() & !P4_STEM_I2C_PINS) }); // SEL1=0
    p.p4.p4sel0().modify(|r, w| unsafe { w.bits(r.bits() | P4_STEM_I2C_PINS) }); // SEL0=1
}

// Console UART = eUSCI_A1, 9600 8N1 @ 1 MHz SMCLK (the FR2355 LaunchPad backchannel; SLAU680 §2.2.4).
const UCSWRST: u16 = 0x0001;
const UCSSEL_SMCLK: u16 = 0x0080;
const UCOS16: u16 = 0x0001;
const UCTXIFG: u16 = 0x0002;

pub fn console_uart_init(p: &Peripherals) {
    p.e_usci_a1.uca1ctlw0().write(|w| unsafe { w.bits(UCSWRST) });
    p.e_usci_a1
        .uca1ctlw0()
        .modify(|r, w| unsafe { w.bits(r.bits() | UCSSEL_SMCLK) });
    p.e_usci_a1.uca1brw().write(|w| unsafe { w.bits(6) });
    p.e_usci_a1
        .uca1mctlw()
        .write(|w| unsafe { w.bits(0x2000 | (8 << 4) | UCOS16) });
    p.e_usci_a1
        .uca1ctlw0()
        .modify(|r, w| unsafe { w.bits(r.bits() & !UCSWRST) });
}

#[inline]
pub fn console_uart_tx(p: &Peripherals, c: u8) {
    while p.e_usci_a1.uca1ifg().read().bits() & UCTXIFG == 0 {}
    p.e_usci_a1.uca1txbuf().write(|w| unsafe { w.bits(c as u16) });
}

/// Snapshot the console UART's config regs (CTLW0, BRW, MCTLW) for the SFR dump — reads the
/// backchannel instance (UCA1 here) so the dump is correct per module, not hardcoded.
pub fn console_uart_regs(p: &Peripherals) -> (u16, u16, u16) {
    (
        p.e_usci_a1.uca1ctlw0().read().bits(),
        p.e_usci_a1.uca1brw().read().bits(),
        p.e_usci_a1.uca1mctlw().read().bits(),
    )
}

/// Console-UART pin-mux check for the SFR dump: the correct PORT's (SEL1, SEL0, DIR) + the per-pin
/// (name, bit, expected). FR2355 backchannel = UCA1 on **P4.2 (RXD) / P4.3 (TXD)** (SLAU680 §2.2.4) —
/// a DIFFERENT port from FR2476, which is the whole reason the dump must be board-aware.
pub fn console_uart_pin_report(
    p: &Peripherals,
) -> (u16, u16, u16, [(&'static str, u16, &'static str); 2]) {
    (
        u16::from(p.p4.p4sel1().read().bits()),
        u16::from(p.p4.p4sel0().read().bits()),
        u16::from(p.p4.p4dir().read().bits()),
        [("P4.2", 0x04, "UCA1RXD (01)"), ("P4.3", 0x08, "UCA1TXD (01)")],
    )
}

// RX side of the console backchannel (prod's on-demand command trigger; diag is TX-only → dead_code).
const UCRXIFG: u16 = 0x0001;
#[allow(dead_code)]
pub fn console_uart_rx_ready(p: &Peripherals) -> bool {
    p.e_usci_a1.uca1ifg().read().bits() & UCRXIFG != 0
}
#[allow(dead_code)]
pub fn console_uart_getc(p: &Peripherals) -> u8 {
    p.e_usci_a1.uca1rxbuf().read().bits() as u8
}

// µs time base: TB1 free-running off SMCLK. FR2355 has no Timer_A; TB0 is the ms base (clock.rs),
// so the µs base uses Timer_B instance 1.
const TBSSEL_SMCLK: u16 = 0x0200; // TBSSEL_2
const MC_CONTINUOUS: u16 = 0x0020; // MC_2
const TBCLR: u16 = 0x0004;
pub fn usec_start(p: &Peripherals) {
    p.tb1
        .tb1ctl()
        .write(|w| unsafe { w.bits(TBSSEL_SMCLK | MC_CONTINUOUS | TBCLR) });
}
#[inline]
pub fn usec_now(p: &Peripherals) -> u16 {
    p.tb1.tb1r().read().bits()
}
