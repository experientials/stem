//! I²C stress & margin — now a **cooperative task** selected from the menu (no longer a separate
//! `--features stress` build). It hammers the bus two ways and reports over UART + a compact OLED
//! status (see DIAGNOSTICS.md for pass thresholds):
//!   1. **Clock-margin sweep** — raise SCL 100 kHz → 1 MHz and, at each step, do many *verified*
//!      reads (read a known-valued ID register and compare). Highest zero-error clock = the bus's
//!      reliability margin.
//!   2. **Error-rate soak** — hold 100 kHz (the operating clock) and read continuously, counting
//!      NACKs + bit-corruptions per transaction, tracking latency min/max. Runs until you leave.
//!
//! "Verified" (compare a known value) is the point: it catches **bit corruption**, not just NACKs.
//!
//! Because the scheduler is cooperative (never block), each `poll` does a **bounded batch** of
//! reads and yields — so the radar/button/UI tasks stay responsive while a margin sweep or a
//! multi-day soak runs. State that a blocking loop would keep on the stack lives in the task.

use crate::tasks::{Cx, Mode};
use crate::{i2c, uart, usec};
use crate::pac::Peripherals;
use sched::Task;

/// A device we can read a fixed-value register from, so each transaction is checkable. `setup`:
/// if `Some(v)`, WRITE `v` to `reg` before each read (and expect to read it back) — a write-verify.
/// This matters when no register holds a datasheet-constant: a plain read of a reset default (0x00 /
/// 0xFF) would **false-pass** a stuck-low / stuck-high bus. Writing a non-trivial pattern (0xA5) first
/// means a stuck bus reads back the wrong value → FAIL, and it exercises the write path too.
struct Target {
    addr: u8,
    reg: u8,
    val: u8,
    setup: Option<u8>,
}

// WHO_AM_I / ID registers with datasheet-constant values (same ones the POST registry verifies).
#[allow(dead_code)]
static TARGETS_SENSORS: &[Target] = &[
    Target { addr: 0x39, reg: 0x92, val: 0xAB, setup: None }, // APDS-9960
    Target { addr: 0x29, reg: 0xC0, val: 0xEE, setup: None }, // VL53L0X
];

// BENCH DIAGNOSIS (Q-SENSING-ENUM, FR2355 Leaf): the only Leaf device is a PCA9698 I/O extender @0x20.
// It has no datasheet-constant register, so we WRITE-VERIFY a non-trivial 0xA5 to the Polarity-Inversion
// bank-0 reg (0x10, reset 0x00 — harmless, we don't use polarity inversion). 0xA5 ≠ 0x00/0xFF, so a
// stuck-low/high or bit-corrupt bus FAILS at every clock instead of false-passing. Swap back to
// TARGETS_SENSORS for sensor-bench runs.
static TARGETS: &[Target] = &[
    Target { addr: 0x20, reg: 0x10, val: 0xA5, setup: Some(0xA5) },
];

/// SCL = SMCLK (1 MHz) / brw.
struct Step {
    khz: u16,
    brw: u16,
}
static LADDER: &[Step] = &[
    // Low end added for the FR2355 Leaf investigation: if a fault is rise-time/capacitance it clears as
    // the clock drops; if it persists unchanged down to 10 kHz it is NOT a speed-margin problem.
    Step { khz: 10, brw: 100 },
    Step { khz: 25, brw: 40 },
    Step { khz: 50, brw: 20 },
    Step { khz: 100, brw: 10 },
    Step { khz: 200, brw: 5 },
    Step { khz: 333, brw: 3 },
    Step { khz: 500, brw: 2 },
    Step { khz: 1000, brw: 1 },
];

const NOMINAL_BRW: u16 = 10; // 100 kHz — the operating clock, used for the soak
const MARGIN_REPS: u16 = 200; // verified reads per target, per clock step
const REPORT_EVERY_SECS: u32 = 10; // cadence of the cumulative soak report line
const MARGIN_BATCH: u16 = 24; // reads per poll during the sweep (bounded → yields to other tasks)
const SOAK_BATCH: u16 = 40; // read-rounds per poll during the soak
const FAIL_CUTOFF: u16 = 24; // consecutive NACKs at one clock ⇒ unusable, stop hammering it

/// One verified read. `Ok(latency_us)`, or `Err(true)` on a corrupt value / `Err(false)` on
/// NACK/timeout.
fn verified_read(p: &Peripherals, t: &Target) -> Result<u16, bool> {
    if let Some(v) = t.setup {
        // Write-verify: set the register to a known non-trivial pattern first. A NACK here is a
        // transport failure (Err false); the read below then confirms the value survived.
        if !i2c::write(p, t.addr, &[t.reg, v]) {
            return Err(false);
        }
    }
    let t0 = usec::now(p);
    let mut b = [0u8; 1];
    if !i2c::read_reg(p, t.addr, t.reg, &mut b) {
        return Err(false); // NACK / timeout
    }
    let dt = usec::now(p).wrapping_sub(t0);
    if b[0] != t.val {
        return Err(true); // corrupt
    }
    Ok(dt)
}

/// Running soak totals. Transaction count is split into millions + remainder so a multi-week run
/// doesn't overflow a single u32 (~12 days at ~4 k txn/s). Latency min/max are cumulative.
struct Totals {
    txn_m: u32,
    txn: u32,
    nack: u32,
    corrupt: u32,
    lat_min: u16,
    lat_max: u16,
    any_ok: bool,
}

impl Totals {
    const fn new() -> Self {
        Self { txn_m: 0, txn: 0, nack: 0, corrupt: 0, lat_min: 0xFFFF, lat_max: 0, any_ok: false }
    }
    fn bump_txn(&mut self) {
        self.txn += 1;
        if self.txn >= 1_000_000 {
            self.txn_m += 1;
            self.txn -= 1_000_000;
        }
    }
    fn clean(&self) -> bool {
        self.nack == 0 && self.corrupt == 0
    }
}

/// The stress runner as a cooperative task. It watches `cx.mode`: idle unless the menu selected
/// `Margin` or `Soak`, at which point it (re)initialises on entry and then advances one bounded
/// batch per `poll`, publishing progress to the blackboard for `UiTask` to render.
pub struct StressTask {
    last_mode: Option<Mode>,
    // margin-sweep state
    li: usize,       // ladder index
    ti: usize,       // target index within the current step
    rep: u16,        // reps done for the current target
    consec: u16,     // consecutive NACKs (early-out)
    nack: u16,       // per-step tallies
    corrupt: u16,
    txn: u16,
    best_khz: u16,
    best_brw: u16,
    // soak state
    tot: Totals,
    start_ms: u32,
    next_report: u32,
    window: u32, // txns since the last report line (for the rate figure)
}

impl StressTask {
    pub const fn new() -> Self {
        Self {
            last_mode: None,
            li: 0,
            ti: 0,
            rep: 0,
            consec: 0,
            nack: 0,
            corrupt: 0,
            txn: 0,
            best_khz: 0,
            best_brw: NOMINAL_BRW,
            tot: Totals::new(),
            start_ms: 0,
            next_report: 0,
            window: 0,
        }
    }

    fn margin_poll(&mut self, cx: &mut Cx) -> Option<u32> {
        let p = cx.p;
        if self.last_mode != Some(Mode::Margin) {
            self.last_mode = Some(Mode::Margin);
            self.li = 0;
            self.ti = 0;
            self.rep = 0;
            self.consec = 0;
            self.nack = 0;
            self.corrupt = 0;
            self.txn = 0;
            self.best_khz = 0;
            self.best_brw = NOMINAL_BRW;
            i2c::set_brw(p, LADDER[0].brw);
            uart::puts(p, "\n-- I2C clock-margin sweep --\n");
            cx.st_phase = 1;
            cx.st_max = 0;
            cx.st_khz = LADDER[0].khz;
            cx.prog_tot = LADDER.len() as u8; // "N of 5" progress
            cx.prog_cur = 1;
            cx.err_nack = 0;
            cx.err_corrupt = 0;
        }
        if self.li >= LADDER.len() {
            // Sweep finished on its own → return to POST (one-button model: a completed test goes
            // back to the resting state). The result line + margin were already printed above.
            cx.st_phase = 2;
            uart::puts(p, "-- margin sweep complete -> POST --\n");
            cx.mode = Mode::Post;
            return Some(200);
        }
        cx.st_khz = LADDER[self.li].khz;
        cx.prog_cur = (self.li + 1) as u8;
        cx.err_nack = self.nack as u32; // errors at the current clock step
        cx.err_corrupt = self.corrupt as u32;
        let mut budget = MARGIN_BATCH;
        while budget > 0 {
            budget -= 1;
            crate::pet_wdt(p); // a read at an unusable clock burns its full timeout; feed per read
            self.txn += 1;
            match verified_read(p, &TARGETS[self.ti]) {
                Ok(_) => self.consec = 0,
                Err(false) => {
                    self.nack += 1;
                    self.consec += 1;
                }
                Err(true) => self.corrupt += 1,
            }
            self.rep += 1;
            if self.rep >= MARGIN_REPS || self.consec >= FAIL_CUTOFF {
                self.rep = 0;
                self.consec = 0;
                self.ti += 1;
                if self.ti >= TARGETS.len() {
                    self.ti = 0;
                    step_line(p, LADDER[self.li].khz, self.txn, self.nack, self.corrupt);
                    if self.nack == 0 && self.corrupt == 0 && LADDER[self.li].khz > self.best_khz {
                        self.best_khz = LADDER[self.li].khz;
                        self.best_brw = LADDER[self.li].brw;
                    }
                    self.nack = 0;
                    self.corrupt = 0;
                    self.txn = 0;
                    self.li += 1;
                    if self.li < LADDER.len() {
                        i2c::set_brw(p, LADDER[self.li].brw);
                    } else {
                        uart::puts(p, "  margin: highest clean SCL = ");
                        uart::dec(p, self.best_khz);
                        uart::puts(p, " kHz\n");
                        cx.st_max = self.best_khz;
                        cx.st_phase = 2;
                    }
                    break; // yield after finishing a step
                }
            }
        }
        Some(1)
    }

    fn soak_poll(&mut self, cx: &mut Cx, now: u32) -> Option<u32> {
        let p = cx.p;
        if self.last_mode != Some(Mode::Soak) {
            self.last_mode = Some(Mode::Soak);
            i2c::set_brw(p, NOMINAL_BRW); // soak at the 100 kHz operating clock
            self.tot = Totals::new();
            self.start_ms = now;
            self.next_report = now.wrapping_add(REPORT_EVERY_SECS * 1000);
            self.window = 0;
            uart::puts(p, "\n-- I2C error-rate soak @100 kHz (cumulative) --\n");
            cx.st_phase = 3;
        }
        let mut budget = SOAK_BATCH;
        while budget > 0 {
            budget -= 1;
            for t in TARGETS {
                self.tot.bump_txn();
                self.window += 1;
                match verified_read(p, t) {
                    Ok(dt) => {
                        self.tot.any_ok = true;
                        if dt < self.tot.lat_min {
                            self.tot.lat_min = dt;
                        }
                        if dt > self.tot.lat_max {
                            self.tot.lat_max = dt;
                        }
                    }
                    Err(false) => self.tot.nack += 1,
                    Err(true) => self.tot.corrupt += 1,
                }
            }
        }
        crate::pet_wdt(p);
        let secs = now.wrapping_sub(self.start_ms) / 1000;
        if now >= self.next_report {
            report(p, secs, &self.tot, self.window / REPORT_EVERY_SECS);
            self.window = 0;
            self.next_report = self.next_report.wrapping_add(REPORT_EVERY_SECS * 1000);
        }
        cx.st_secs = secs;
        cx.st_txn_m = self.tot.txn_m;
        cx.st_txn = self.tot.txn;
        cx.err_nack = self.tot.nack;
        cx.err_corrupt = self.tot.corrupt;
        Some(1)
    }
}

impl<'a> Task<Cx<'a>> for StressTask {
    fn poll(&mut self, cx: &mut Cx<'a>, now: u32) -> Option<u32> {
        match cx.mode {
            Mode::Margin => self.margin_poll(cx),
            Mode::Soak => self.soak_poll(cx, now),
            m => {
                self.last_mode = Some(m); // reset entry-detection so a re-entry re-initialises
                Some(100)
            }
        }
    }

    fn name(&self) -> &'static str {
        "stress"
    }
}

/// One margin-sweep step line: `  <khz> kHz: <txn> txn, <nack> nack, <corrupt> corrupt  OK/FAIL`.
fn step_line(p: &Peripherals, khz: u16, txn: u16, nack: u16, corrupt: u16) {
    uart::puts(p, "  ");
    uart::dec(p, khz);
    uart::puts(p, " kHz: ");
    uart::dec(p, txn);
    uart::puts(p, " txn, ");
    uart::dec(p, nack);
    uart::puts(p, " nack, ");
    uart::dec(p, corrupt);
    uart::puts(p, " corrupt  ");
    uart::puts(p, if nack == 0 && corrupt == 0 { "OK\n" } else { "FAIL\n" });
}

/// One cumulative soak report line: `[t=Ns] txns=..  err=n+c  rate=../s  lat=min-max us  PASS/FAIL`.
fn report(p: &Peripherals, secs: u32, tot: &Totals, rate: u32) {
    uart::puts(p, "  [t=");
    dec32(p, secs);
    uart::puts(p, "s] txns=");
    if tot.txn_m > 0 {
        dec32(p, tot.txn_m);
        uart::puts(p, "M ");
    }
    dec32(p, tot.txn);
    uart::puts(p, "  err=");
    dec32(p, tot.nack);
    uart::puts(p, "+");
    dec32(p, tot.corrupt);
    uart::puts(p, "  rate=");
    dec32(p, rate);
    uart::puts(p, "/s  lat=");
    if tot.any_ok {
        uart::dec(p, tot.lat_min);
        uart::puts(p, "-");
        uart::dec(p, tot.lat_max);
        uart::puts(p, "us");
    } else {
        uart::puts(p, "n/a");
    }
    uart::puts(p, if tot.clean() { "  PASS\n" } else { "  FAIL\n" });
}

/// u32 decimal (uart::dec is u16-only; soak counts overflow that in seconds).
fn dec32(p: &Peripherals, mut v: u32) {
    if v == 0 {
        uart::putc(p, b'0');
        return;
    }
    let mut buf = [0u8; 10];
    let mut i = 0;
    while v > 0 {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        uart::putc(p, buf[i]);
    }
}
