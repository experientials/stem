//! Boot reason — decode `SYSRSTIV`, the reset-cause register, into the human-readable reason the MCU
//! last reset (brownout / SVS / watchdog / PUC / software / FRAM error …). The MCU-self "WHY did we
//! just (re)boot" number that nothing else can give — the first thing you want on a **reset loop**.
//!
//! This is a universal capability (every chip, including the FR2433 slave), so — unlike `bsp::adc`
//! (Leaf-only) — it is NOT `_leaf`-gated. It decodes the **raw u16** rather than the PAC's typed
//! `Sysrstiv` enum, because that enum exists on the fr2476/fr2355/fr2xxx PACs but NOT on the older
//! fr2433 PAC generation (there `SYSRSTIV` is raw bits). The numeric codes are hardware-defined and
//! identical across the MSP430 FR2xxx family.
//!
//! **Read-clears (standard MSP430 `*IV` semantics):** reading `SYSRSTIV` returns the HIGHEST-priority
//! pending reset/NMI cause and **clears that flag**; reading again yields the next pending cause, down
//! to `0` (none). So [`take_primary`] is one read (the dominant cause); drain in a loop to list all.
//!
//! Codes are quoted verbatim from the fr2476 PAC's `sysrstiv` enum doc comments (svd2rust, from TI's
//! SVD) — e.g. `2: Brownout`, `14: SVSHIFG SVSH event`, `22: WDTIFG watchdog timeout`,
//! `36: FLL unlock (PUC)`. See `pac/msp430fr2476/src/lib.rs` (search `pub enum Sysrstiv`).

use crate::pac::Peripherals;

/// Decode a raw `SYSRSTIV` code to a short static label. Unknown/reserved codes fall through to
/// `"?"` (the raw number is reported alongside, so an unmapped value is never silently lost).
pub fn label(code: u16) -> &'static str {
    match code {
        0 => "none",
        2 => "BOR (brownout)",
        4 => "RST/NMI",
        6 => "SW-BOR",
        8 => "LPMx.5 wakeup",
        10 => "security-violation",
        14 => "SVSH",
        20 => "SW-POR",
        22 => "WDT-timeout",
        24 => "WDT-pw-violation",
        26 => "FRCTL-pw-violation",
        28 => "FRAM-bit-error",
        30 => "peripheral-fetch",
        32 => "PMM-pw-violation",
        36 => "FLL-unlock (PUC)",
        12 | 16 | 18 | 34 => "reserved",
        _ => "?",
    }
}

/// Read **and clear** the highest-priority pending reset cause → `(raw_code, label)`. `code == 0`
/// means nothing pending (all causes already drained). Call again to drain the next cause.
pub fn take_primary(p: &Peripherals) -> (u16, &'static str) {
    let code = p.sys.sysrstiv().read().bits();
    (code, label(code))
}
