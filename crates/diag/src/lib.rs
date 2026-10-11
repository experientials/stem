#![no_std]

//! `diag` — the DIAG **harness** over the shared node feature set (diag & prod are two harnesses over
//! ONE feature set; prod ⊆ diag). Folded in from the former standalone `diag/` binary: the working
//! cooperative-`sched` tasks (`Cx` + POST / stress / signature / radar / proximity / UI) migrate here
//! **verbatim** (only imports change — `crate::{i2c,pac,board,hal,model,adc}` re-export `bsp`, the
//! display DRIVERS are `devices::{ssd1306,is31}`). No rewrite: a `sched::Task` is the degenerate async
//! task of the one engine (ADR-0013).
//!
//! **Integration (Henrik 2026-10-11):** in a `--diag` build the node runs [`run`] — diag **kicks in ~2 s
//! into POST unless skipped** (a master sends `SkipDiag` in the window → return to normal operation),
//! then runs the diag `sched` harness. A **persisted mode enables the soak tests, toggled through
//! commands** (`SetDiagMode` → `LiveNode::diag_mode`, FRAM-persist pending) — bridged to `tasks::Mode`
//! each loop so the SoM/stem-probe drives it.

pub use bsp::{adc, board, hal, i2c, model, pac};

mod buttons;
mod clock;
mod diag;
mod rcwl;
mod regs;
mod signature;
mod stress;
mod tasks;
mod uart;
mod usec;

use crate::pac::Peripherals;
use stem_node::{ec, node};
use stembus::diag_mode::DiagMode;
use tasks::Mode;

// PM5CTL0.LOCKLPM5 — set out of reset on FRAM parts; cleared to make GPIO live (deepsleep clears it too;
// idempotent). IS31 SDB on P2.5 (drive high to enable); RCWL-0516 radar OUT on P2.4 (GPIO in + pulldown).
const P2_SDB: u8 = 0x20;
const P2_RCWL_OUT: u8 = 0x10;

/// Pet the watchdog backstop — kept so the migrated `stress`/`signature` tasks' `crate::pet_wdt` callers
/// are unchanged. Thin delegate to the shared `board` WDT role.
pub fn pet_wdt(p: &Peripherals) {
    board::wdt_pet(p);
}

// Boot window ≈ ~2 s: service the Stem slave so a master monitoring boot can send `SkipDiag` to omit the
// diag harness and go straight to normal operation. WDT held during bring-up, so the window can't trip it.
const WINDOW_PASSES: u16 = 160;
const PACE_NOPS: u16 = 12_000; // ~12k cycles/pass × 160 ≈ ~2 s at 1 MHz

/// Map the persisted continual-diag selector to the harness mode. `Off` rests in POST; a set mode runs
/// its test (soak = the continual one). This is the command/register toggle (`SetDiagMode`).
fn mode_from(code: DiagMode) -> Mode {
    match code {
        DiagMode::Margin => Mode::Margin,
        DiagMode::Soak => Mode::Soak,
        DiagMode::Signature => Mode::Signature,
        DiagMode::Off | DiagMode::Unknown(_) => Mode::Post,
    }
}

/// The `--diag` entry: the ~2 s skip window, then (unless skipped) the diag `sched` harness. Called from
/// the node's `#[cfg(feature="diag")]` boot hook with the node already brought up (Stem slave serving).
pub fn run(
    hw: &bsp::StemHw<'_>,
    ec_slave: &mut stembus::cros_ec::EcSlave,
    live: &mut node::LiveNode,
    p: &Peripherals,
) {
    uart::puts(p, "diag: ~2s window — send 'skipdiag' to skip the diag harness\n");
    for _ in 0..WINDOW_PASSES {
        ec::poll(hw, ec_slave, live);
        if live.take_skip_request() {
            uart::puts(p, "diag: skipped -> normal operation\n");
            return;
        }
        for _ in 0..PACE_NOPS {
            msp430::asm::nop();
        }
    }

    // Not skipped → bring up the diag-harness extras the node boot didn't (Leaf master bus + sensors +
    // the IS31 SDB enable + the RCWL input + the µs/ms time bases + the live register dump).
    board::route_leaf_i2c_pins(p);
    i2c::init(p);
    // IS31FL3730 enable: SDB high on P2.5 (GPIO out, set high before direction).
    p.p2.p2sel0().modify(|r, w| unsafe { w.bits(r.bits() & !P2_SDB) });
    p.p2.p2out().modify(|r, w| unsafe { w.bits(r.bits() | P2_SDB) });
    p.p2.p2dir().modify(|r, w| unsafe { w.bits(r.bits() | P2_SDB) });
    // RCWL-0516 radar OUT on P2.4: GPIO input + pulldown (idle/unplugged reads LOW).
    p.p2.p2sel1().modify(|r, w| unsafe { w.bits(r.bits() & !P2_RCWL_OUT) });
    p.p2.p2sel0().modify(|r, w| unsafe { w.bits(r.bits() & !P2_RCWL_OUT) });
    p.p2.p2dir().modify(|r, w| unsafe { w.bits(r.bits() & !P2_RCWL_OUT) });
    p.p2.p2out().modify(|r, w| unsafe { w.bits(r.bits() & !P2_RCWL_OUT) });
    p.p2.p2ren().modify(|r, w| unsafe { w.bits(r.bits() | P2_RCWL_OUT) });
    adc::init(p);
    buttons::init(p);
    regs::stats(p);
    regs::dump(p);
    usec::start(p);
    let mut clock = clock::Clock::start(p);

    // The diag harness: the migrated cooperative tasks, driven by `sched::tick`. `cx.mode` is bridged
    // from the persisted `diag_mode` each pass (commands toggle it); `ec::poll` keeps the control
    // channel live so `SetDiagMode`/`SkipDiag` land. ButtonTask is dropped — mode comes from commands.
    let mut cx = tasks::Cx::new(p);
    let mut radar = tasks::RadarTask::new();
    let mut prox = tasks::ProximityTask::new();
    let mut post = tasks::PostTask::new();
    let mut ui = tasks::UiTask::new();
    let mut stress = stress::StressTask::new();
    let mut sig = signature::SignatureTask::new();
    let mut slots = [
        sched::Slot::every(50, &mut radar),
        sched::Slot::every(200, &mut prox),
        sched::Slot::every(3000, &mut post),
        sched::Slot::every(200, &mut ui),
        sched::Slot::every(5, &mut stress),
        sched::Slot::every(5, &mut sig),
    ];

    loop {
        pet_wdt(p);
        ec::poll(hw, ec_slave, live); // keep the control channel live (SetDiagMode/SkipDiag)
        cx.mode = mode_from(live.diag_mode()); // command/register toggle → harness mode
        let now = clock.now_ms(p);
        sched::tick(&mut slots, &mut cx, now);
    }
}
