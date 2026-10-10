//! diag extension — the `--diag` build's **extended POST** (DESIGN.md "diag — a `--diag` build flag that
//! fills a POST extension point"). Runs AFTER the normal POST and BEFORE normal operation:
//!
//!   1. a boot **window** during which the node services the Stem slave (so a master monitoring boot can
//!      send `SkipDiag`) — and, later, polls the S1 button;
//!   2. unless skipped, the **per-device diag tasks** (I2C exercise / radar / ADC health / stress), run as
//!      `sched` tasks interleaved with the slave service so they never block the control channel.
//!
//! diag is ADDITIVE: either way the node returns here and proceeds to normal operation. This module is
//! compiled ONLY in a `--diag` build (feature `diag`); a normal build leaves the hook empty.
//!
//! SLICE 1 (this file): the window + `SkipDiag` + the cooperative-service skeleton, with a placeholder
//! extended-POST body. Per-device exercises are re-homed from the `diag/` crate incrementally (then
//! `diag/` is deleted); the window is also the future entry for command/condition-triggered diag.

use crate::pac::Peripherals;
use crate::{ec, node, uart};

// Boot window ≈ a few seconds at 1 MHz SMCLK. The DESIGN target is ~2 s; this first slice uses a generous
// span so the skip is testable by hand on the bench. Each pass services the slave + checks for a
// `SkipDiag`; a short nop delay paces the loop. The WDT is HELD during bring-up (armed only after this,
// before the serve loop), so a multi-second window here does not trip the backstop.
const WINDOW_PASSES: u16 = 400;
const PACE_NOPS: u16 = 12_000; // ~12k cycles/pass × 400 ≈ ~5 s at 1 MHz

/// Run the diag boot window + (unless skipped) the extended POST. Returns to the caller either way; the
/// node then proceeds to normal operation.
pub fn run(
    hw: &bsp::StemHw<'_>,
    ec_slave: &mut stembus::cros_ec::EcSlave,
    live: &mut node::LiveNode,
    p: &Peripherals,
) {
    uart::puts(p, "diag: boot window — send 'skipdiag' (or S1, later) to skip extended POST\n");
    for _ in 0..WINDOW_PASSES {
        // Cooperative service: drain any Stem transaction so a master's SkipDiag (or any command) lands.
        ec::poll(hw, ec_slave, live);
        if live.take_skip_request() {
            uart::puts(p, "diag: skipped by master\n");
            return;
        }
        for _ in 0..PACE_NOPS {
            msp430::asm::nop();
        }
    }
    // Window elapsed without a skip → run the extended POST (per-device diag tasks).
    uart::puts(p, "diag: extended POST\n");
    // TODO(slice 2+): acquire the Leaf bus (Sensing) → per-device exercise tasks (I2C devices, radar,
    // ADC rail/temp health) as sched tasks interleaved with `ec::poll` → release; re-homed from `diag/`.
    uart::puts(p, "diag: done\n");
}
