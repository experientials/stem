//! I²C stress — **clock-margin sweep** + **error-rate soak**, re-homed from `diag/src/stress.rs` as
//! diag-MODE bodies (DESIGN.md "CONTINUAL diag is a MODE controlled by command/register"). The old
//! cooperative `sched::Task` + `Cx`/OLED-blackboard wrapper is DROPPED:
//!   - **margin** is a bounded linear sweep (`DiagMode::Margin`, runs once, then the node proceeds);
//!   - **soak** is the continual mode (`DiagMode::Soak`) — a cooperative loop that services the Stem
//!     slave each pass so the master can disable it (`diagmode off`) at any time.
//!
//! Same verified-read logic as the original (write/read a datasheet-constant register and COMPARE, so bit
//! corruption is caught, not just NACKs) over the shared raw `bsp::i2c`. Targets are **guarded by the scan
//! presence bitmap** — an absent sensor is skipped, not a false fail (an improvement over the original's
//! hardcoded target that false-fails on a different bench).

use crate::DiagMode;
use bsp::fmt as uart;
use bsp::pac::Peripherals;
use leaf_scan::enumerate;
use stem_node::{ec, node};

/// A device with a readable **constant** register, so each transaction is checkable. `setup`: if `Some(v)`
/// WRITE `v` to `reg` first and expect to read it back (a write-verify, for devices with no datasheet
/// constant — a stuck bus then reads the wrong value → FAIL instead of a false pass).
struct Target {
    addr: u8,
    reg: u8,
    val: u8,
    setup: Option<u8>,
}

// WHO_AM_I / ID registers with datasheet-constant values — the same ones the POST registry verifies.
// APDS-9960 + VL53L0X are on the sensor bench; each guarded by scan presence below.
static TARGETS: &[Target] = &[
    Target { addr: 0x39, reg: 0x92, val: 0xAB, setup: None }, // APDS-9960 ID (0x92 = 0xAB)
    Target { addr: 0x29, reg: 0xC0, val: 0xEE, setup: None }, // VL53L0X model id (0xC0 = 0xEE)
];

/// SCL = SMCLK (1 MHz) / brw.
struct Step {
    khz: u16,
    brw: u16,
}
static LADDER: &[Step] = &[
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
const SOAK_BATCH: u16 = 40; // read-rounds per cooperative soak pass (bounded → services the slave)
const SOAK_REPORT_PASSES: u32 = 2000; // soak report cadence (passes; no RTC → count, not seconds)

/// One verified read. `Ok(())`, or `Err(true)` on a corrupt value / `Err(false)` on NACK/timeout.
fn verified_read(p: &Peripherals, t: &Target) -> Result<(), bool> {
    if let Some(v) = t.setup {
        if !bsp::i2c::write(p, t.addr, &[t.reg, v]) {
            return Err(false);
        }
    }
    let mut b = [0u8; 1];
    if !bsp::i2c::read_reg(p, t.addr, t.reg, &mut b) {
        return Err(false); // NACK / timeout
    }
    if b[0] != t.val {
        return Err(true); // corrupt
    }
    Ok(())
}

/// How many targets are actually present on this bench (per the scan). Zero ⇒ nothing to exercise.
fn present_count(scan: &enumerate::Scan) -> u8 {
    TARGETS.iter().filter(|t| scan.present.get(t.addr)).count() as u8
}

/// `  <khz> kHz: <txn> txn, <nack> nack, <corrupt> corrupt  OK/FAIL`.
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

/// **Margin sweep** (`DiagMode::Margin`): raise SCL 10 kHz → 1 MHz and, at each step, do `MARGIN_REPS`
/// verified reads per present target, counting NACK + corruption. The highest zero-error clock is the
/// bus's reliability margin. Bounded → runs once, then the node proceeds to normal operation. (The WDT is
/// held throughout diag::run, so a slow step at an unusable clock cannot trip a backstop.)
pub fn margin_sweep(p: &Peripherals, scan: &enumerate::Scan) {
    uart::puts(p, "\n-- I2C clock-margin sweep --\n");
    if present_count(scan) == 0 {
        uart::puts(p, "  (no verifiable target present — skip)\n");
        return;
    }
    let mut best_khz = 0u16;
    for step in LADDER {
        bsp::i2c::set_brw(p, step.brw);
        let mut nack = 0u16;
        let mut corrupt = 0u16;
        let mut txn = 0u16;
        for t in TARGETS {
            if !scan.present.get(t.addr) {
                continue;
            }
            for _ in 0..MARGIN_REPS {
                txn += 1;
                match verified_read(p, t) {
                    Ok(()) => {}
                    Err(false) => nack += 1,
                    Err(true) => corrupt += 1,
                }
            }
        }
        step_line(p, step.khz, txn, nack, corrupt);
        if nack == 0 && corrupt == 0 && step.khz > best_khz {
            best_khz = step.khz;
        }
    }
    bsp::i2c::set_brw(p, NOMINAL_BRW); // restore the operating clock
    uart::puts(p, "  margin: highest clean SCL = ");
    uart::dec(p, best_khz);
    uart::puts(p, " kHz\n");
}

/// Cumulative soak totals. Transaction count splits into millions + remainder so a multi-day run doesn't
/// overflow a u32 (~12 days at ~4 k txn/s).
struct Totals {
    txn_m: u32,
    txn: u32,
    nack: u32,
    corrupt: u32,
}

impl Totals {
    const fn new() -> Self {
        Self { txn_m: 0, txn: 0, nack: 0, corrupt: 0 }
    }
    fn bump(&mut self) {
        self.txn += 1;
        if self.txn >= 1_000_000 {
            self.txn_m += 1;
            self.txn -= 1_000_000;
        }
    }
}

/// u32 decimal (uart::dec is u16-only; soak counts overflow that).
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

fn soak_report(p: &Peripherals, tot: &Totals) {
    uart::puts(p, "  soak: txns=");
    if tot.txn_m > 0 {
        dec32(p, tot.txn_m);
        uart::puts(p, "M ");
    }
    dec32(p, tot.txn);
    uart::puts(p, "  err=");
    dec32(p, tot.nack);
    uart::puts(p, "+");
    dec32(p, tot.corrupt);
    uart::puts(p, if tot.nack == 0 && tot.corrupt == 0 { "  PASS\n" } else { "  FAIL\n" });
}

/// **Error-rate soak** (`DiagMode::Soak`): hold 100 kHz and read continuously, counting NACK + bit
/// corruption. COOPERATIVE — each pass does a bounded batch then services the Stem slave (`ec::poll`), so
/// the master can send `diagmode off` to stop it (the control channel never blocks). Returns when the mode
/// is no longer `Soak`; the node then proceeds to normal operation.
pub fn soak_loop(
    hw: &bsp::StemHw<'_>,
    ec_slave: &mut stembus::cros_ec::EcSlave,
    live: &mut node::LiveNode,
    p: &Peripherals,
    scan: &enumerate::Scan,
) {
    uart::puts(p, "\n-- I2C error-rate soak @100 kHz (send 'diagmode off' to stop) --\n");
    if present_count(scan) == 0 {
        uart::puts(p, "  (no verifiable target present — skip)\n");
        return;
    }
    bsp::i2c::set_brw(p, NOMINAL_BRW);
    let mut tot = Totals::new();
    let mut passes: u32 = 0;
    loop {
        for _ in 0..SOAK_BATCH {
            for t in TARGETS {
                if !scan.present.get(t.addr) {
                    continue;
                }
                tot.bump();
                match verified_read(p, t) {
                    Ok(()) => {}
                    Err(false) => tot.nack += 1,
                    Err(true) => tot.corrupt += 1,
                }
            }
        }
        // Service the control channel so a `diagmode off` (or any command) lands, then check for stop.
        ec::poll(hw, ec_slave, live);
        if live.diag_mode() != DiagMode::Soak {
            break;
        }
        passes += 1;
        if passes % SOAK_REPORT_PASSES == 0 {
            soak_report(p, &tot);
        }
    }
    soak_report(p, &tot);
    uart::puts(p, "-- soak stopped (diagmode off) --\n");
}
