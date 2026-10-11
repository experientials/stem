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

use crate::hal::EusciI2c;
use crate::pac::Peripherals;
use crate::{ec, enumerate, mode, node, uart};
use devices::mc6470::{Mc6470, ADDR as MC6470_ADDR};
use devices::si7021::{Si7021, ADDR as SI7021_ADDR, PART_SI7021};
use devices::{Device, Error};

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
    mach: &mut mode::ModeMachine,
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

    // I2C device tests (re-homed from diag/'s t_scan + t_devices, over the shared `leaf-scan`): acquire the
    // Leaf sensor bus (a temporary Sensing switch — the node boots Passive), scan + report the connected
    // devices' presence/inventory, then RELEASE back to Passive so normal operation starts off the bus.
    mach.switch(hw, mode::Mode::Sensing);
    let scan = enumerate::scan(hw);
    enumerate::report(hw, &scan);
    exercise_devices(p, &scan); // per-device value reads (re-homed t_gravity/t_temphum), guarded by presence
    // TODO(slice 3): radar (GPIO) + ADC rail/temp health + stress/soak/signature as sched tasks.
    mach.switch(hw, mode::Mode::Passive);

    uart::puts(p, "diag: done\n");
}

/// Per-device value exercises over the (already-acquired) Leaf bus — re-homed from diag/'s `t_gravity` +
/// `t_temphum`, reusing the SAME proven `devices` drivers (no fork). Each is guarded by the scan's presence
/// bitmap, so an absent sensor is skipped (not a false failure). On a bench with only an OLED these both
/// skip; wire the sensor and the real read runs.
#[cfg(feature = "diag")]
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
