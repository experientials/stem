//! diag's cooperative tasks and the context they share.
//!
//! The [`sched`] crate is the generic engine; this module is diag's concrete use of it. `Cx` is
//! the context every task is handed: the peripherals handle plus a small **blackboard** for
//! cross-task data. Task-private state stays in the task struct (e.g. [`RadarTask::prev`]).
//!
//! This is where the radar became a first-class *task* rather than a once-per-pass snapshot:
//! [`RadarTask`] samples P2.4 continuously (fast while motion is asserted, idling slower) and
//! accumulates a motion window that [`PostTask`] reports and clears every ~3 s — so a trigger
//! that falls between POST passes is no longer missed.

use crate::hal::EusciI2c;
use crate::{buttons, diag, rcwl, ssd1306_raw as ssd, uart};
use devices::{apds9960::Apds9960, Device};
use crate::pac::Peripherals;
use sched::Task;

/// What the firmware is currently doing. **One button (S1) cycles through these in order** —
/// `Post → Margin → Soak → Post` — so a single press starts a stress test, the next press ends it
/// and jumps to the following one. A test that finishes on its own returns to `Post`. `Post` is the
/// default (boots into it) and the resting state; the bus tests were formerly `--features stress`.
#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Post,
    Margin,
    Soak,
    Signature,
}

impl Mode {
    /// The next state in the one-button cycle.
    pub fn next(self) -> Mode {
        match self {
            Mode::Post => Mode::Margin,
            Mode::Margin => Mode::Soak,
            Mode::Soak => Mode::Signature,
            Mode::Signature => Mode::Post,
        }
    }

    /// Short label for the OLED / console (the on-screen mode name).
    pub fn label(self) -> &'static str {
        match self {
            Mode::Post => "DIAG",
            Mode::Margin => "I2C SPEED",
            Mode::Soak => "I2C SCAN",
            Mode::Signature => "I2C SIG",
        }
    }
}

/// Context handed to every task each `poll`. `p` is the hardware; the rest is the cross-task
/// blackboard. Keep shared state here small and explicit — task-private state belongs in the task.
pub struct Cx<'a> {
    pub p: &'a Peripherals,
    /// Any motion seen since the last POST report.
    pub radar_seen: bool,
    /// Rising edges (fresh triggers) counted since the last POST report.
    pub radar_edges: u16,
    /// Latest APDS-9960 proximity (0 far .. 255 near); `None` if absent / not yet valid.
    pub apds_prox: Option<u8>,
    /// Current mode (advanced by `ButtonTask`, read by everyone).
    pub mode: Mode,
    // Latest POST verdict, published by `PostTask`, rendered by `UiTask` (single display owner):
    pub post_ok: bool,
    pub post_present: u16,
    pub post_total: u16,
    /// Whether the OLED took its last `init()` (published by `UiTask`, reported by `PostTask`).
    /// Restores the "OLED rendered / init FAILED" health signal after the display moved to UiTask.
    pub oled_ok: bool,
    // Progress (bottom-line "N of M" on the OLED): current step + total for the active mode.
    pub prog_cur: u8,
    pub prog_tot: u8,
    /// Current POST test's name (for the DIAG progress line).
    pub post_step_name: &'static str,
    // Recorded errors (the OLED error shorthand + reported by the stress modes):
    pub err_nack: u32,
    pub err_corrupt: u32,
    // Stress status published by `StressTask`, rendered by `UiTask`:
    /// 0 idle, 1 sweeping, 2 sweep done, 3 soaking.
    pub st_phase: u8,
    pub st_khz: u16, // current sweep step (kHz)
    pub st_max: u16, // best clean clock found
    pub st_secs: u32, // soak elapsed
    pub st_txn_m: u32, // soak transactions (millions + remainder)
    pub st_txn: u32,
}

impl<'a> Cx<'a> {
    pub fn new(p: &'a Peripherals) -> Self {
        Self {
            p,
            radar_seen: false,
            radar_edges: 0,
            apds_prox: None,
            mode: Mode::Post,
            post_ok: false,
            post_present: 0,
            post_total: 0,
            oled_ok: true,
            prog_cur: 0,
            prog_tot: 0,
            post_step_name: "",
            err_nack: 0,
            err_corrupt: 0,
            st_phase: 0,
            st_khz: 0,
            st_max: 0,
            st_secs: 0,
            st_txn_m: 0,
            st_txn: 0,
        }
    }
}

/// Samples the RCWL-0516 OUT line (P2.4) and accumulates a motion window into the blackboard.
///
/// Variable cadence (the [`sched`] "fixed/variable frequency" case): it samples tight while OUT is
/// asserted — to time the pulse and not miss a short one — and backs off when idle, where a 20 Hz
/// glance is plenty to catch the rising edge of the next ~2 s trigger. Private state is just the
/// previous level, for edge detection.
pub struct RadarTask {
    prev: bool,
}

impl RadarTask {
    const FAST_MS: u32 = 5; // while OUT is high
    const IDLE_MS: u32 = 50; // while OUT is idle

    pub const fn new() -> Self {
        Self { prev: false }
    }
}

impl<'a> Task<Cx<'a>> for RadarTask {
    fn poll(&mut self, cx: &mut Cx<'a>, _now: u32) -> Option<u32> {
        let hi = rcwl::motion(cx.p);
        if hi && !self.prev {
            cx.radar_edges = cx.radar_edges.saturating_add(1); // a fresh trigger
        }
        cx.radar_seen |= hi;
        self.prev = hi;
        Some(if hi { Self::FAST_MS } else { Self::IDLE_MS })
    }

    fn name(&self) -> &'static str {
        "radar"
    }
}

/// Exercises the APDS-9960 proximity engine (beyond the WHO_AM_I check in the registry): enables
/// it once — retrying while absent, so a sensor plugged in after boot still comes up — then
/// samples PDATA into the blackboard. Non-blocking: the sensor free-runs and we read the latest
/// byte. Hot-unplug is handled (a read failure + missing device re-arms `enable`), matching diag's
/// "watch it re-test live on the bench" ethos.
pub struct ProximityTask {
    enabled: bool,
}

impl ProximityTask {
    const RATE_MS: u32 = 200; // 5 Hz once running
    const RETRY_MS: u32 = 1000; // slower poll while the device is absent

    pub const fn new() -> Self {
        Self { enabled: false }
    }
}

impl<'a> Task<Cx<'a>> for ProximityTask {
    fn poll(&mut self, cx: &mut Cx<'a>, _now: u32) -> Option<u32> {
        // Pause during the bus stress tests so unrelated I²C traffic doesn't skew the soak counts.
        if cx.mode != Mode::Post {
            cx.apds_prox = None;
            return Some(Self::RETRY_MS);
        }
        // Same shared driver prod uses (devices::apds9960) over diag's embedded-hal bus seam —
        // no forked per-sensor code. `enable` once, then non-blocking `sample` the free-runner.
        let mut bus = EusciI2c::new(cx.p);
        if !self.enabled {
            if Apds9960::enable(&mut bus).is_ok() {
                self.enabled = true;
            } else {
                cx.apds_prox = None; // still absent — back off
                return Some(Self::RETRY_MS);
            }
        }
        match Apds9960::sample(&mut bus) {
            Ok(Some(v)) => {
                cx.apds_prox = Some(v);
                Some(Self::RATE_MS)
            }
            // Not valid this instant, or gone — probe to tell them apart (as before).
            Ok(None) => {
                cx.apds_prox = None;
                if devices::present(&mut bus, Apds9960::ADDR) {
                    Some(Self::RATE_MS) // present, just not valid this instant
                } else {
                    self.enabled = false; // unplugged — re-init when it returns
                    Some(Self::RETRY_MS)
                }
            }
            // Bus error (NACK / stuck) — treat as gone and re-arm enable when it returns.
            Err(_) => {
                cx.apds_prox = None;
                self.enabled = false;
                Some(Self::RETRY_MS)
            }
        }
    }

    fn name(&self) -> &'static str {
        "prox"
    }
}

/// The power-on self-test, run **cooperatively — one `Test` from `diag::TESTS` per tick**. That lets
/// the OLED show `DIAG N of M` live and stops the ~1.5 s scan from hogging the loop (each test is
/// one bounded poll). On the last test it prints the summary, drives the LED-matrix verdict, reports
/// the sensor-window blackboard, then idles ~3 s before the next pass. Only runs in `Post` mode.
pub struct PostTask {
    step: usize,
    pass: u16,
    fail: u16,
    skip: u16,
}

impl PostTask {
    pub const fn new() -> Self {
        Self { step: 0, pass: 0, fail: 0, skip: 0 }
    }
}

impl<'a> Task<Cx<'a>> for PostTask {
    fn poll(&mut self, cx: &mut Cx<'a>, _now: u32) -> Option<u32> {
        if cx.mode != Mode::Post {
            self.step = 0; // restart the pass cleanly when we return to DIAG
            return Some(3000);
        }
        let p = cx.p;
        if self.step == 0 {
            diag::banner(p);
            self.pass = 0;
            self.fail = 0;
            self.skip = 0;
            cx.prog_tot = diag::test_count() as u8;
        }

        // Run exactly one test this tick, and publish progress for the OLED.
        cx.post_step_name = diag::test_name(self.step);
        match diag::run_test(p, self.step) {
            diag::Outcome::Pass => self.pass += 1,
            diag::Outcome::Fail => self.fail += 1,
            diag::Outcome::Skip => self.skip += 1,
        }
        self.step += 1;
        cx.prog_cur = self.step as u8;

        if self.step < diag::test_count() {
            return Some(40); // next test soon; UiTask renders the step between ticks
        }

        // Pass complete: summary, verdict, health + sensor report, then idle before the next pass.
        diag::summary(p, self.pass, self.fail, self.skip);
        let ok = self.fail == 0;
        diag::led_verdict(p, ok);
        cx.post_ok = ok;
        cx.post_present = self.pass;
        cx.post_total = self.pass + self.fail + self.skip;
        cx.err_nack = self.fail as u32; // DIAG "errors" = failed tests

        uart::puts(p, "  OLED: ");
        uart::puts(p, if cx.oled_ok { "ok\n" } else { "INIT FAILED\n" });
        uart::puts(p, "  radar window (P2.4): ");
        uart::puts(p, if cx.radar_seen { "motion, " } else { "idle, " });
        uart::dec(p, cx.radar_edges);
        uart::puts(p, " trigger(s)\n");
        cx.radar_seen = false;
        cx.radar_edges = 0;
        uart::puts(p, "  APDS-9960 prox (0x39): ");
        match cx.apds_prox {
            Some(v) => uart::dec(p, v as u16),
            None => uart::puts(p, "n/a"),
        }
        uart::puts(p, "\n");

        self.step = 0;
        Some(3000)
    }

    fn name(&self) -> &'static str {
        "post"
    }
}

/// The one physical button (S1 / P4.0). Each **press edge** advances the mode one step in the cycle
/// `Post → Margin → Soak → Post` — so from POST a press starts the margin sweep, the next press ends
/// it and starts the soak, the next returns to POST. Only edges act (a held button doesn't repeat).
/// Debounced by the fixed poll cadence. StressTask is told to (re)start via `st_phase = 0`.
pub struct ButtonTask {
    prev: bool,
}

impl ButtonTask {
    pub const fn new() -> Self {
        Self { prev: false }
    }
}

impl<'a> Task<Cx<'a>> for ButtonTask {
    fn poll(&mut self, cx: &mut Cx<'a>, _now: u32) -> Option<u32> {
        let s1 = buttons::s1(cx.p);
        let edge = s1 && !self.prev;
        self.prev = s1;
        if edge {
            cx.mode = cx.mode.next();
            cx.st_phase = 0; // a fresh run is starting (or returning to POST)
            uart::puts(cx.p, "\n[S1] -> ");
            uart::puts(cx.p, cx.mode.label());
            uart::puts(cx.p, "\n");
        }
        // Console command — the same shared backchannel UART (no S1 needed, so the host/M7 can drive
        // the menu). Explicit keys select a mode directly; Enter/space mirrors the button (next mode).
        if uart::rx_ready(cx.p) {
            let next = match uart::getc(cx.p) {
                b'p' | b'P' => Some(Mode::Post),
                b'm' | b'M' => Some(Mode::Margin),
                b's' | b'S' => Some(Mode::Soak),
                b'g' | b'G' => Some(Mode::Signature),
                b'\r' | b'\n' | b' ' => Some(cx.mode.next()),
                _ => None,
            };
            if let Some(m) = next {
                cx.mode = m;
                cx.st_phase = 0;
                uart::puts(cx.p, "\n[key] -> ");
                uart::puts(cx.p, cx.mode.label());
                uart::puts(cx.p, "\n");
            }
        }
        Some(25) // ~40 Hz scan = the debounce interval
    }

    fn name(&self) -> &'static str {
        "btn"
    }
}

/// **Sole owner of the OLED.** Reflects what the system is doing as a passive status screen (no
/// navigation). Common 128×32 layout: **line 0 = mode name**, lines 1–2 = detail + the error
/// shorthand, **line 3 (bottom) = `N of M` progress**. Boots with a self-test: the panel fills fully
/// ON (~0.7 s, an unambiguous "the raw driver works" signal) then shows `BOOT / SELF TEST`, then the
/// mode screens (`DIAG` / `I2C SPEED` / `I2C SCAN`). All values come from the blackboard.
pub struct UiTask {
    started: bool,
    t0: u32,          // boot start (ms)
    boot_text: bool,  // drawn the "BOOT" text phase yet?
    screen: Option<Mode>,
}

impl UiTask {
    const BOOT_FILL_MS: u32 = 700; // white self-test flash
    const BOOT_MS: u32 = 1600; // total boot splash before the first mode screen

    pub const fn new() -> Self {
        Self { started: false, t0: 0, boot_text: false, screen: None }
    }

    /// Draw the bottom-line `N of M` progress.
    fn progress(p: &crate::pac::Peripherals, cur: u8, tot: u8) {
        let c = ssd::num(p, 3, 0, cur as u32);
        let c = ssd::text(p, 3, c, " of ");
        let c = ssd::num(p, 3, c, tot as u32);
        ssd::clear_eol(p, 3, c);
    }

    /// Draw the error shorthand line (`NACK n CRPT n`).
    fn errors(p: &crate::pac::Peripherals, line: u8, nack: u32, corrupt: u32) {
        let c = ssd::text(p, line, 0, "NACK ");
        let c = ssd::num(p, line, c, nack);
        let c = ssd::text(p, line, c, " CRPT ");
        let c = ssd::num(p, line, c, corrupt);
        ssd::clear_eol(p, line, c);
    }
}

impl<'a> Task<Cx<'a>> for UiTask {
    fn poll(&mut self, cx: &mut Cx<'a>, now: u32) -> Option<u32> {
        let p = cx.p;

        // Boot self-test: init + fill fully ON. A lit panel proves the raw driver before any layout.
        if !self.started {
            self.started = true;
            self.t0 = now;
            cx.oled_ok = ssd::init(p);
            ssd::fill(p, 0xFF);
            return Some(150);
        }
        let dt = now.wrapping_sub(self.t0);
        if dt < Self::BOOT_MS {
            if dt >= Self::BOOT_FILL_MS && !self.boot_text {
                self.boot_text = true; // white flash done → show the BOOT splash text
                ssd::clear(p);
                ssd::text(p, 0, 0, "BOOT");
                ssd::text(p, 1, 0, "SELF TEST");
                ssd::text(p, 3, 0, if cx.oled_ok { "OLED OK" } else { "OLED FAIL" });
            }
            return Some(100);
        }

        let mode = cx.mode;
        let entering = self.screen != Some(mode);
        if entering {
            self.screen = Some(mode);
            cx.oled_ok = ssd::init(p);
            ssd::clear(p);
        }

        match mode {
            // DIAG — line1 = current test, line2 = verdict, bottom = N of M.
            Mode::Post => {
                ssd::text(p, 0, 0, "DIAG");
                let c = ssd::text(p, 1, 0, cx.post_step_name);
                ssd::clear_eol(p, 1, c);
                let c = ssd::text(p, 2, 0, if cx.post_ok { "OK" } else { "FAULT" });
                ssd::clear_eol(p, 2, c);
                Self::progress(p, cx.prog_cur, cx.prog_tot);
            }
            // I2C SPEED — line1 = clock (or MAX when done), line2 = errors, bottom = N of 5.
            Mode::Margin => {
                ssd::text(p, 0, 0, "I2C SPEED");
                if cx.st_phase >= 2 {
                    let c = ssd::text(p, 1, 0, "MAX ");
                    let c = ssd::num(p, 1, c, cx.st_max as u32);
                    let c = ssd::text(p, 1, c, " KHZ");
                    ssd::clear_eol(p, 1, c);
                    let c = ssd::text(p, 3, 0, "DONE");
                    ssd::clear_eol(p, 3, c);
                } else {
                    let c = ssd::num(p, 1, 0, cx.st_khz as u32);
                    let c = ssd::text(p, 1, c, " KHZ");
                    ssd::clear_eol(p, 1, c);
                    Self::progress(p, cx.prog_cur, cx.prog_tot);
                }
                Self::errors(p, 2, cx.err_nack, cx.err_corrupt);
            }
            // I2C SCAN — line1 = elapsed, line2 = errors, bottom = transaction count.
            Mode::Soak => {
                ssd::text(p, 0, 0, "I2C SCAN");
                let c = ssd::text(p, 1, 0, "T ");
                let c = ssd::num(p, 1, c, cx.st_secs);
                let c = ssd::text(p, 1, c, "S");
                ssd::clear_eol(p, 1, c);
                Self::errors(p, 2, cx.err_nack, cx.err_corrupt);
                let mut c = 0u8;
                if cx.st_txn_m > 0 {
                    c = ssd::num(p, 3, 0, cx.st_txn_m);
                    c = ssd::text(p, 3, c, "M ");
                }
                let c = ssd::num(p, 3, c, cx.st_txn);
                let c = ssd::text(p, 3, c, " TX");
                ssd::clear_eol(p, 3, c);
            }
            // I2C SIG — the signature-pattern battery runs once and prints to UART; the OLED just
            // names it (progress/results live on the console + the logic-analyzer trace).
            Mode::Signature => {
                ssd::text(p, 0, 0, "I2C SIG");
                let c = ssd::text(p, 1, 0, "SEE UART");
                ssd::clear_eol(p, 1, c);
                let c = ssd::text(p, 2, 0, "+ SCOPE");
                ssd::clear_eol(p, 2, c);
            }
        }
        Some(200) // 5 Hz refresh
    }

    fn name(&self) -> &'static str {
        "ui"
    }
}
