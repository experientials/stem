//! Boot record — the machine-readable proof that the firmware booted and got how far.
//!
//! A fixed-layout struct at a stable symbol (`BOOTREC`, `#[no_mangle]`) that the firmware fills
//! during init. Two read paths, one source of truth (see the `prove-firmware-on-hardware` skill):
//!   - **SBW/JTAG** (tier-1 iteration): the host reads `&BOOTREC`'s address over the eZ-FET — works
//!     with NO console (needs `thepia hwd msp read`, a filed feature request). This is the proof
//!     channel when the UART is dead.
//!   - **Console** (dump below): human-readable, when the UART works.
//! `stage` advances through init so a hang is located by *how far it got*, not guesswork.
//!
//! Layout is `#[repr(C)]` + fixed field order so an external reader can decode it from raw bytes.
//! Address is taken from the ELF symbol table (`nm`/the .map) — no custom linker region needed yet;
//! pin it to a dedicated address when `thepia hwd msp read` lands and we want a hardcoded probe addr.

use crate::uart;
// Chip-agnostic PAC via the shared alias (diag re-exports bsp::pac). NOTE: this file is still
// dormant (not `mod`-declared) and its snapshot fields assume the fr2355 board (UCA1 ctlw0, p4sel0);
// full chip-agnosticism is CONV-2 in docs/BACKLOG.md — this de-hardcodes only the PAC import.
use crate::pac::Peripherals;

/// Boot-stage milestones (advance in `main` as init proceeds). A stalled boot reads back the last
/// stage it reached → pinpoints the failing init step (e.g. stuck at `ClockSet` ⇒ DCO/SMCLK bug).
#[repr(u8)]
#[derive(Clone, Copy)]
pub enum Stage {
    Entry = 1,     // main() entered, watchdog held
    ClockSet = 2,  // clock_init_1mhz returned
    PinsSet = 3,   // eUSCI pin-mux + LPM5 unlocked
    UartUp = 4,    // uart::init done
    PeriphUp = 5,  // i2c/adc/buttons init done
    PostReady = 6, // reached the POST loop — fully up
}

/// Self-test bit positions (set via [`selftest`] as diag exercises each subsystem).
pub mod st {
    pub const CLOCK_LOCK: u16 = 1 << 0;
    pub const UART_TX: u16 = 1 << 1;
    pub const I2C_BUS: u16 = 1 << 2;
    pub const ADC_REF: u16 = 1 << 3;
    // sensors etc. extend here; absent devices are recorded as not-set, explained in the log.
}

#[repr(C)]
pub struct BootRecord {
    pub magic: u16,       // 0xB007 once written — "the firmware ran and reached bootrec::init"
    pub boot_stage: u8,   // last Stage reached (see `Stage`)
    pub _pad: u8,         // keep u16/u32 fields aligned for an external decoder
    pub self_test: u16,   // bitmap of `st::*` results
    pub build_id: u32,    // FNV-1a of DIAG_BUILD — the same id prod exposes over I2C
    pub uart_ctlw0: u16,  // snapshot of eUSCI_A1 UCA1CTLW0 after uart::init (peripheral regs get reset
    pub uart_brw: u16,    // by the SBW connect, so we snapshot them into RAM which survives the halt)
    pub cs_ctl7: u16,     // snapshot of CSCTL7 — FLLUNLOCK bits (0x0300) clear ⇒ FLL locked ⇒ SMCLK exact
    pub p4sel0: u16,      // snapshot of P4SEL0 — bits 2/3 set (0x0C) ⇒ UCA1 UART routed to P4.2/P4.3
}

/// The record. `#[no_mangle]` → a stable symbol whose address the host reads from the ELF, then
/// pokes over SBW. Lives in RAM; written fresh each boot.
#[no_mangle]
pub static mut BOOTREC: BootRecord = BootRecord {
    magic: 0,
    boot_stage: 0,
    _pad: 0,
    self_test: 0,
    build_id: 0,
    uart_ctlw0: 0,
    uart_brw: 0,
    cs_ctl7: 0,
    p4sel0: 0,
};

const MAGIC: u16 = 0xB007;

/// FNV-1a 32-bit of the build stamp — a stable machine-readable id (same scheme as prod `status`).
fn build_id() -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in env!("DIAG_BUILD").bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Stamp identity + magic and mark `Entry`. Call first thing in `main` (after `steal`).
pub fn init() {
    // SAFETY: single-threaded, no ISRs (diag uses `Peripherals::steal`); the only writer.
    unsafe {
        BOOTREC.magic = MAGIC;
        BOOTREC.build_id = build_id();
        BOOTREC.boot_stage = Stage::Entry as u8;
    }
}

/// Advance the boot stage.
pub fn stage(s: Stage) {
    unsafe { BOOTREC.boot_stage = s as u8 };
}

/// Snapshot the live eUSCI_A1 UART config into RAM, so it's readable over SBW (the SBW connect resets
/// peripheral registers to default, but RAM survives). Call right after `uart::init`.
pub fn snapshot_uart(ctlw0: u16, brw: u16, csctl7: u16, p4sel0: u16) {
    unsafe {
        BOOTREC.uart_ctlw0 = ctlw0;
        BOOTREC.uart_brw = brw;
        BOOTREC.cs_ctl7 = csctl7;
        BOOTREC.p4sel0 = p4sel0;
    }
}

/// Record a self-test result bit.
pub fn selftest(bit: u16, pass: bool) {
    unsafe {
        if pass {
            BOOTREC.self_test |= bit;
        } else {
            BOOTREC.self_test &= !bit;
        }
    }
}

/// Human-readable dump on the console (tier-1 when the UART works; the SBW read is the fallback).
pub fn dump(p: &Peripherals) {
    // SAFETY: single reader here, no concurrent writer.
    let (magic, stage, selftest, id) =
        unsafe { (BOOTREC.magic, BOOTREC.boot_stage, BOOTREC.self_test, BOOTREC.build_id) };
    uart::puts(p, "bootrec magic=0x");
    uart::hex16(p, magic);
    uart::puts(p, " stage=");
    uart::dec(p, stage as u16);
    uart::puts(p, " selftest=0x");
    uart::hex16(p, selftest);
    uart::puts(p, " build_id=0x");
    uart::hex16(p, (id >> 16) as u16);
    uart::hex16(p, id as u16);
    uart::putc(p, b'\n');
}
