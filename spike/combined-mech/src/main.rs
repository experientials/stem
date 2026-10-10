#![no_main]
#![no_std]
#![feature(abi_msp430_interrupt)] // `extern "msp430-interrupt"` for the handler fn type + the wake ISR.
#![feature(asm_experimental_arch)] // core::arch::asm!/naked_asm! on msp430 (the wake trampoline + LPM entry).

//! SPIKE: ONE image, flashed unchanged, runs an eUSCI_B1 I²C slave that wakes from LPM and answers a
//! fixed byte (0x5A) on a master read — on BOTH FR2476 and FR2355 — via:
//!   (a) a CUSTOM dual-slot `__INTERRUPTS` table (the EUSCI_B1 handler at word idx 45 AND 47, so each
//!       chip's hardware EUSCI_B1 vector — 0xFFDA on FR2476, 0xFFDE on FR2355 — lands on our handler), and
//!   (b) RUNTIME selection of the eUSCI_B1 base (0x0580 FR2476 / 0x05C0 FR2355), the Stem pins
//!       (P3.2/P3.6 FR2476 / P4.6/P4.7 FR2355) and the role address (0x21 / 0x20) keyed on the device-id.
//!
//! The PAC's `rt` feature is OFF, so the PAC emits NO `__INTERRUPTS` — WE supply the table below. That is
//! the crux this spike exists to de-risk. msp430-rt is used WITHOUT its `device` feature (generic-app
//! mode): its link.x omits `INCLUDE device.x` and provides only Reset + `.__RESET_VECTOR` (reset @ 0xFFFE).
//!
//! Build-only. The dual-chip hardware proof is the parent's; this file never claims runtime success.

extern crate panic_msp430; // infinite-loop panic handler (a hang trips the WDT → reset)

use msp430_rt::entry;
use msp430fr2xxx_dual as pac; // COMBINED PAC, rt ON — emits the dual-slot __INTERRUPTS + device.x; also gives Peripherals + the e_usci_b1 register-block TYPE (overlaid on a runtime base).

// ── eUSCI_B1 base addresses (VERIFIED per the task; register-block LAYOUT is identical, only base differs) ──
const B1_BASE_FR2476: usize = 0x0580;
const B1_BASE_FR2355: usize = 0x05C0;

// ── Device IDs (bsp::model: TLV word @ 0x1A04). 0x832A = FR2476, 0x830C = FR2355 (confirmed on HW). ──
const DEVICE_ID_ADDR: usize = 0x1A04;
const ID_FR2476: u16 = 0x832A;
const ID_FR2355: u16 = 0x830C;

// ── Role-based Stem I²C own-address (I2C-API.md / stembus::role): FR2476 = 0x21, FR2355 = 0x20. ──
const ADDR_FR2476: u16 = 0x21;
const ADDR_FR2355: u16 = 0x20;

// ── Stem pin masks (bsp::board::route_stem_i2c_pins): P3.2(SDA)|P3.6(SCL) on FR2476; P4.6|P4.7 on FR2355. ──
const P3_STEM_I2C_PINS: u8 = 0x44; // BIT2 | BIT6
const P4_STEM_I2C_PINS: u8 = 0xC0; // BIT6 | BIT7

// ── eUSCI_B CTLW0 / I2COA0 / IE bits (slau445i §24 — same as bsp::stem_i2c). ──
const UCSWRST: u16 = 0x0001;
const UCMODE_3: u16 = 0x0600; // I2C mode
const UCSYNC: u16 = 0x0100;
const UCOAEN: u16 = 0x0400; // own-address enable
const UCRXIFG0: u16 = 0x0001;
const UCTXIFG0: u16 = 0x0002;
const UCSTTIFG: u16 = 0x0004;
const UCSTPIFG: u16 = 0x0008;
const IE_SLAVE: u16 = UCRXIFG0 | UCTXIFG0 | UCSTTIFG | UCSTPIFG;

const FIXED_BYTE: u8 = 0x5A; // answer every master read with this.
const LOCKLPM5: u16 = 0x0001; // PM5CTL0

// WDT_A: password 0x5A00 | WDTHOLD 0x0080 (hold the watchdog during this spike).
const WDT_HOLD: u16 = 0x5A80;

/// The runtime eUSCI_B1 base, published by `main` BEFORE slave IE is enabled so the wake ISR can reach
/// the right instance. Single-core, written once before any interrupt can fire — plain static is sound here.
static mut B1_BASE: usize = 0;

// ─── VECTOR TABLE: now emitted by the COMBINED PAC (msp430fr2xxx-dual, rt) ───
//
// The combined PAC's `__INTERRUPTS` places the EUSCI_B1 handler at BOTH idx 45 (0xFFDA, FR2476) and idx 47
// (0xFFDE, FR2355) — idx 47 repointed A1→B1 in the PAC. Reset fills 0xFFFE; every other slot weak-aliases
// to msp430-rt's `DefaultHandler` via the PAC's `device.x` (26 PROVIDEs). So this bin defines ONLY the
// EUSCI_B1 handler below — no hand-rolled table. This is the mechanism the spike proved, now productized
// in the real PAC (the point of the S2b.1 re-point).

// ─────────────────────────────── the EUSCI_B1 wake ISR ───────────────────────────────
//
// Proven LPM-wake trampoline (prod/src/main.rs + docs/references/msp430-lpm-wake-pattern.md): the naked
// outer clears the LPM bits in the STACKED SR (`bic.b #0xF0, 0(r1)`) so the CPU wakes, then tail-calls
// the inner which masks the slave IE at the RUNTIME base so the latched IFG won't re-fire before main
// re-arms it.
#[unsafe(naked)]
#[no_mangle]
unsafe extern "msp430-interrupt" fn EUSCI_B1() {
    core::arch::naked_asm!(
        "bic.b #0xF0, 0(r1)",
        "br #{inner}",
        inner = sym eusci_b1_inner,
    );
}

extern "msp430-interrupt" fn eusci_b1_inner() {
    // SAFETY: single-core; masks only its own source; runs only while main is in LPM / at the atomic sleep.
    let base = unsafe { B1_BASE };
    let b1 = b1(base);
    // IE &= !IE_SLAVE
    let ie = b1.ucb1ie().read().bits();
    b1.ucb1ie().write(|w| unsafe { w.bits(ie & !IE_SLAVE) });
}

/// Overlay the typed `e_usci_b1::RegisterBlock` on a RUNTIME base pointer (NOT the PAC's fixed-base
/// peripheral). The register-block layout is identical across the two parts, so the same accessors serve
/// whichever base the device-id selected.
#[inline(always)]
fn b1(base: usize) -> &'static pac::e_usci_b1::RegisterBlock {
    unsafe { &*(base as *const pac::e_usci_b1::RegisterBlock) }
}

#[entry]
fn main() -> ! {
    // Typed access to PMM / ports / WDT is fine: those blocks sit at IDENTICAL addresses on both parts
    // (the task's "ports share addresses"), so the fr2476 PAC's fixed-base peripherals are correct for
    // FR2355 too. Only eUSCI_B1 differs — and that is reached through the runtime base below.
    let p = unsafe { pac::Peripherals::steal() };

    // Hold the watchdog during bring-up.
    p.wdt_a.wdtctl().write(|w| unsafe { w.bits(WDT_HOLD) });
    // Unlock GPIO from the LPM5 default so port config takes effect.
    p.pmm.pm5ctl0().modify(|r, w| unsafe { w.bits(r.bits() & !LOCKLPM5) });

    // Runtime device-id → (base, pins, address).
    let id = unsafe { core::ptr::read_volatile(DEVICE_ID_ADDR as *const u16) };
    let (base, addr) = match id {
        ID_FR2355 => {
            // FR2355 Stem pins P4.6/P4.7 → UCB1 (SEL1=0, SEL0=1).
            p.p4.p4sel1().modify(|r, w| unsafe { w.bits(r.bits() & !P4_STEM_I2C_PINS) });
            p.p4.p4sel0().modify(|r, w| unsafe { w.bits(r.bits() | P4_STEM_I2C_PINS) });
            (B1_BASE_FR2355, ADDR_FR2355)
        }
        // FR2476 explicitly, and as the fallback (also covers FR2475 / an unexpected id on an FR247x board).
        ID_FR2476 | _ => {
            // FR2476 Stem pins P3.2/P3.6 → UCB1 (SEL1=0, SEL0=1).
            p.p3.p3sel1().modify(|r, w| unsafe { w.bits(r.bits() & !P3_STEM_I2C_PINS) });
            p.p3.p3sel0().modify(|r, w| unsafe { w.bits(r.bits() | P3_STEM_I2C_PINS) });
            (B1_BASE_FR2476, ADDR_FR2476)
        }
    };

    // Publish the base for the wake ISR BEFORE any IE is enabled.
    unsafe { B1_BASE = base };
    let b1 = b1(base);

    // Configure eUSCI_B1 as an I²C SLAVE at the role address (raw register writes at the runtime base).
    b1.ucb1ctlw0().write(|w| unsafe { w.bits(UCSWRST) }); // hold in reset
    b1.ucb1ctlw0().modify(|r, w| unsafe { w.bits(r.bits() | UCMODE_3 | UCSYNC) }); // I2C slave (no UCMST)
    b1.ucb1i2coa0().write(|w| unsafe { w.bits(addr | UCOAEN) }); // own-address
    b1.ucb1ctlw0().modify(|r, w| unsafe { w.bits(r.bits() & !UCSWRST) }); // release
    b1.ucb1ifg().write(|w| unsafe { w.bits(0) }); // clear stale flags
    // Pre-load TXBUF so the very first master read is answered; we reload it on each TxReq below.
    b1.ucb1txbuf().write(|w| unsafe { w.bits(FIXED_BYTE as u16) });

    // Serve loop: sleep in LPM0+GIE on the slave ISR (the proven 1b pattern). On wake the ISR has masked
    // IE; we drain the transaction (keep TXBUF loaded with the fixed byte) and re-arm before re-sleeping.
    loop {
        // Drain any pending slave events; always keep the fixed byte queued for a TX request.
        let ifg = b1.ucb1ifg().read().bits();
        if ifg & UCTXIFG0 != 0 {
            b1.ucb1txbuf().write(|w| unsafe { w.bits(FIXED_BYTE as u16) }); // answer the read (clears UCTXIFG0)
        }
        if ifg & UCRXIFG0 != 0 {
            let _ = b1.ucb1rxbuf().read().bits(); // ignore RX; the read clears UCRXIFG0
        }
        if ifg & (UCSTTIFG | UCSTPIFG) != 0 {
            b1.ucb1ifg().modify(|r, w| unsafe { w.bits(r.bits() & !(UCSTTIFG | UCSTPIFG)) });
        }

        // Atomic sleep: GIE off, arm the slave IE, then LPM0+GIE in one instruction so no ISR fires in the
        // gap (Samek's chicken-and-egg). A pending IFG fires immediately instead of sleeping forever.
        msp430::interrupt::disable();
        let ie = b1.ucb1ie().read().bits();
        b1.ucb1ie().write(|w| unsafe { w.bits(ie | IE_SLAVE) });
        unsafe { core::arch::asm!("bis #0x18, r2", options(nostack)) }; // CPUOFF|GIE
    }
}
