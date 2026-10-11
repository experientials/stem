#![no_std]

//! `diag` — the node-generic **extended POST** (DESIGN.md "diag — a `--diag` build flag that fills a POST
//! extension point"). Runs AFTER the normal POST and BEFORE normal operation:
//!
//!   1. a boot **window** during which the node services the Stem slave (so a master monitoring boot can
//!      send `SkipDiag`) — and, later, polls the S1 button;
//!   2. unless skipped, the per-device diag checks (boot-reason, ADC board-health, I2C scan + per-device
//!      exercises) — and, later, the long-running task-based modes (stress/soak/signature).
//!
//! diag is ADDITIVE: either way the node returns to the caller and proceeds to normal operation.
//!
//! **Why a crate, not a module in `modules/deepsleep`:** this is node-generic (what *any* Stembus node
//! does for an extended self-test), so every node module keeps only a thin `#[cfg(feature = "diag")]
//! diag::run(...)` hook and nothing node-generic bulks one module. Built only when the module's `diag`
//! feature pulls this crate in (an optional dep). MSP430-oriented for now (ADC/boot-reason use `bsp`'s
//! PAC); the bus/skip/scan paths are already generic over the `node-hal` seams.
//!
//! The Leaf bus is grabbed via the `node-hal::LeafBus` seam directly (`acquire`/`release`) rather than the
//! product's `ModeMachine` — same pin-route + master bring-up, but it leaves the mode machine unperturbed
//! (the node stays Passive throughout; diag runs before normal operation).

mod signature;
mod stress;

use bsp::fmt as uart;
use bsp::hal::EusciI2c;
use bsp::pac::Peripherals;
use devices::mc6470::{Mc6470, ADDR as MC6470_ADDR};
use devices::si7021::{Si7021, ADDR as SI7021_ADDR, PART_SI7021};
use devices::{Device, Error};
use leaf_scan::enumerate;
use node_hal::LeafBus;
use stem_node::{ec, node};
pub use stembus::diag_mode::DiagMode;

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
    // Window elapsed without a skip → run the extended POST.
    uart::puts(p, "diag: extended POST\n");

    // Boot reason FIRST (MCU-self, no bus): WHY the MCU last reset — the dominant instrument for a
    // reset loop (BOR/SVSH ⇒ brownout/SVS; WDT/PUC ⇒ software). Drains SYSRSTIV (read-clears), so all
    // pending causes are listed, dominant first. Guards against a runaway (max 6 reads → None).
    uart::puts(p, "  reset: ");
    let mut printed = 0u8;
    for _ in 0..6 {
        let (code, why) = bsp::bootreason::take_primary(p);
        if code == 0 {
            break;
        }
        if printed > 0 {
            uart::puts(p, ", ");
        }
        uart::puts(p, why);
        uart::puts(p, "(");
        uart::dec(p, code);
        uart::puts(p, ")");
        printed += 1;
    }
    if printed == 0 {
        uart::puts(p, "none(0)");
    }
    uart::putc(p, b'\n');

    // Board health (MCU-self, no bus): supply rail + die temperature from the ADC's internal channels.
    bsp::adc::init(p);
    uart::puts(p, "  board: DVCC=");
    uart::dec(p, bsp::adc::dvcc_mv(p));
    uart::puts(p, "mV  Tdie=");
    uart::dec_i16(p, bsp::adc::die_temp_c(p));
    uart::puts(p, "C\n");

    // I2C device tests (re-homed from diag/'s t_scan + t_devices, over the shared `leaf-scan`): acquire the
    // Leaf sensor bus (via the LeafBus seam — the node boots Passive), scan + report the connected devices'
    // presence/inventory, then RELEASE back to Passive so normal operation starts off the bus.
    hw.acquire();
    let scan = enumerate::scan(hw);
    enumerate::report(hw, &scan);
    exercise_devices(p, &scan); // per-device value reads (re-homed t_gravity/t_temphum), guarded by presence

    // CONTINUAL diag MODE (DESIGN.md "CONTINUAL diag is a MODE controlled by command/register"): the master
    // (SoM/stem-probe) selected one via `SetDiagMode` during the boot window above; the bus is acquired.
    // DEFAULT Off → nothing runs → the node proceeds to normal operation. Margin/Signature are bounded
    // (run once); Soak is cooperative (services the slave each pass, so `diagmode off` stops it at runtime).
    match live.diag_mode() {
        DiagMode::Off | DiagMode::Unknown(_) => {}
        DiagMode::Margin => stress::margin_sweep(p, &scan),
        DiagMode::Signature => signature::signature_battery(p, &scan),
        DiagMode::Soak => stress::soak_loop(hw, ec_slave, live, p, &scan),
    }
    hw.release();

    uart::puts(p, "diag: done\n");
}

/// Per-device value exercises over the (already-acquired) Leaf bus — re-homed from diag/'s `t_gravity` +
/// `t_temphum`, reusing the SAME proven `devices` drivers (no fork). Each is guarded by the scan's presence
/// bitmap, so an absent sensor is skipped (not a false failure). On a bench with only an OLED these both
/// skip; wire the sensor and the real read runs.
fn exercise_devices(p: &Peripherals, scan: &enumerate::Scan) {
    let mut bus = EusciI2c::new(p);

    // MC6470 accelerometer — gravity sanity (|a| ≈ 1 g).
    if scan.present.get(MC6470_ADDR) {
        uart::puts(p, "  MC6470 accel: ");
        match Mc6470::measure(&mut bus) {
            Ok(a) => {
                uart::puts(p, "|a|=");
                uart::dec(p, a.magnitude_mg() as u16);
                uart::puts(p, if a.is_gravity() { "mg OK\n" } else { "mg OUT-OF-RANGE\n" });
            }
            Err(Error::NotReady) => uart::puts(p, "not ready (wake latency), skip\n"),
            Err(_) => uart::puts(p, "read FAILED\n"),
        }
    }

    // Si7021 temperature + humidity — real values + CRC.
    if scan.present.get(SI7021_ADDR) {
        uart::puts(p, "  Si7021 T/RH: ");
        if let Ok(id) = Si7021::part_id(&mut bus) {
            uart::puts(p, if id == PART_SI7021 { "(Si7021) " } else { "(HTU21/SHT21?) " });
        }
        match Si7021::measure(&mut bus) {
            Ok(r) => {
                uart::puts(p, "T=");
                uart::fixed2(p, r.temp_c_centi);
                uart::puts(p, "C RH=");
                uart::fixed2(p, r.rh_centi);
                uart::puts(p, if r.crc_ok { "% OK\n" } else { "% crc BAD\n" });
            }
            Err(_) => uart::puts(p, "read FAILED\n"),
        }
    }
}
