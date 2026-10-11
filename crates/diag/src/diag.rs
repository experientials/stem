//! POST runner: bus scan (surfaces anything wired) + a registry of KNOWN devices checked by
//! presence and WHO_AM_I. Add a device with one line in `DEVICES`. Reported over UART, with a
//! pass/fail glyph on the LED matrix when present.

use crate::hal::EusciI2c;
use crate::{i2c, uart};
use devices::mc6470::{Mc6470, ADDR as MC6470_ADDR};
use devices::si7021::{Si7021, ADDR as SI7021_ADDR, PART_SI7021};
use devices::{Device, Error};
use crate::pac::Peripherals;

// 8x8 status glyphs (orientation may be mirrored/rotated on real hardware; fix once seen).
const GLYPH_CHECK: [u8; 8] = [0x01, 0x03, 0x06, 0x8C, 0xD8, 0x70, 0x20, 0x00];
const GLYPH_CROSS: [u8; 8] = [0x00, 0x42, 0x24, 0x18, 0x18, 0x24, 0x42, 0x00];

/// A known device: address, and an optional WHO_AM_I register + expected value.
/// `id_reg == NO_ID` means presence-only (no readable ID register).
struct Dev {
    name: &'static str,
    addr: u8,
    id_reg: u8,
    id_val: u8,
}
const NO_ID: u8 = 0xFF;

// The expected device set. Extend this to teach diag about a new part (see DESIGN.md).
// Later this becomes a per-product (Bob/Ziloo) manifest for config-drift checking.
static DEVICES: &[Dev] = &[
    Dev { name: "SSD1306 OLED  ", addr: 0x3C, id_reg: NO_ID, id_val: 0x00 },
    // MCP23017 16-bit I²C I/O expander (ports A/B). Default base address 0x20 (A2:A0 straps → 0x20–0x27)
    // — which COLLIDES with the Stem Supervisor role address, so an address-only hit here is ambiguous.
    // The MCP23017 has NO WHO_AM_I register (Microchip gives none), so this is NOT a true id check: it
    // reads IODIRA (reg 0x00), whose power-on reset value is 0xFF (all pins inputs). That is a POST-time
    // signature — valid on a freshly-reset board, BEFORE anything configures the expander (once driven,
    // IODIRA changes and this would read MISMATCH). It still beats a bare ACK: it proves a register-based
    // expander answers reg 0x00, not just that *something* sits at 0x20.
    Dev { name: "MCP23017 I/O  ", addr: 0x20, id_reg: 0x00, id_val: 0xFF },
    Dev { name: "VL53L0X ToF   ", addr: 0x29, id_reg: 0xC0, id_val: 0xEE },
    Dev { name: "APDS-9960     ", addr: 0x39, id_reg: 0x92, id_val: 0xAB },
    // 6DOF IMU 13 Click (MIKROE-4228) is an mCube MC6470 eCompass — accel + magnetometer as two
    // I2C sub-addresses (0x4C accel, 0x0C mag), NOT an ICM-42605. Presence-only for now; a WHO_AM_I
    // check needs the chip-ID registers verified against the datasheet (0x4C/0x0C both ACK the scan).
    Dev { name: "MC6470 accel  ", addr: 0x4C, id_reg: NO_ID, id_val: 0x00 },
    Dev { name: "MC6470 mag    ", addr: 0x0C, id_reg: NO_ID, id_val: 0x00 },
    Dev { name: "IS31FL3730 LED", addr: 0x60, id_reg: NO_ID, id_val: 0x00 },
    // Si7021 T/RH — presence-only here (its part-number ID is a multi-byte command sequence, not a
    // single-register WHO_AM_I). The `Si7021 T/RH` test reads the real values + identifies the part.
    Dev { name: "Si7021 T/RH   ", addr: 0x40, id_reg: NO_ID, id_val: 0x00 },
];

/// Is this address one of our expected devices? A scan hit that isn't = config drift.
fn is_expected(addr: u8) -> bool {
    DEVICES.iter().any(|d| d.addr == addr)
}

/// Scan the bus; return the count of **unexpected** addresses (ACKed but not in the registry).
fn scan(p: &Peripherals) -> u16 {
    // Idle bus level (P1IN reflects the real pad even when muxed to UCB0):
    //   L = no pull-ups reaching us -> EB unpowered or SDA/SCL not connected.
    //   H = healthy idle bus.
    let lv = p.p1.p1in().read().bits();
    uart::puts(p, "bus idle SDA=");
    uart::putc(p, if lv & 0x04 != 0 { b'H' } else { b'L' }); // P1.2
    uart::puts(p, " SCL=");
    uart::putc(p, if lv & 0x08 != 0 { b'H' } else { b'L' }); // P1.3
    if lv & 0x04 == 0 {
        uart::puts(
            p,
            if i2c::recover(p) {
                " (recovered)"
            } else {
                " (recover FAILED - short?)"
            },
        );
    }
    uart::puts(p, "\nI2C scan:");
    let mut found = 0u16;
    let mut unexpected = 0u16;
    let mut a = 0x08u8;
    while a <= 0x77 {
        if i2c::probe(p, a) {
            uart::puts(p, " 0x");
            uart::hex8(p, a);
            if !is_expected(a) {
                uart::puts(p, "?"); // ACKs but not a known device = config drift
                unexpected += 1;
            }
            found += 1;
        }
        a += 1;
    }
    if found == 0 {
        uart::puts(p, " (none)");
    }
    if unexpected > 0 {
        uart::puts(p, "  (");
        uart::dec(p, unexpected);
        uart::puts(p, " UNEXPECTED)");
    }
    uart::puts(p, "\n");
    unexpected
}

/// Check every known device. Returns (present, total, faults):
///   present = devices that ACKed their address
///   total   = size of the registry
///   faults  = present-but-broken (wrong WHO_AM_I, or the ID read failed)
/// `faults` is the meaningful bench verdict: green when everything that answered is healthy.
/// (Missing devices become faults only once we validate against a per-product manifest — obj 2.)
fn check_devices(p: &Peripherals) -> (u16, u16, u16) {
    let total = DEVICES.len() as u16;
    let mut present = 0u16;
    let mut faults = 0u16;
    uart::puts(p, "devices:\n");
    for d in DEVICES {
        uart::puts(p, "  ");
        uart::puts(p, d.name);
        uart::puts(p, " @0x");
        uart::hex8(p, d.addr);
        uart::puts(p, ": ");
        if !i2c::probe(p, d.addr) {
            uart::puts(p, "absent\n");
            continue;
        }
        present += 1;
        if d.id_reg == NO_ID {
            uart::puts(p, "present\n");
            continue;
        }
        let mut id = [0u8; 1];
        if !i2c::read_reg(p, d.addr, d.id_reg, &mut id) {
            uart::puts(p, "present, id-read FAILED\n");
            faults += 1;
            continue;
        }
        uart::puts(p, "id=");
        uart::hex8(p, id[0]);
        if id[0] == d.id_val {
            uart::puts(p, " OK\n");
        } else {
            uart::puts(p, " MISMATCH want=");
            uart::hex8(p, d.id_val);
            uart::puts(p, "\n");
            faults += 1;
        }
    }
    (present, total, faults)
}

/// A one-shot diagnostic test: prints its own section and returns a verdict. **Reorder `TESTS` to
/// change the run order**; a future button menu / script selects a subset. This is diag's "bag of
/// tests" — distinct from the continuous `sched::Task`s (radar/proximity), which run over time
/// rather than once-to-a-verdict.
pub enum Outcome {
    Pass,
    Fail,
    /// Device absent / not applicable — reported, does NOT fail the verdict. This is how an
    /// optional device (e.g. the OLED on the product) is handled.
    Skip,
}

struct Test {
    name: &'static str,
    run: fn(&Peripherals) -> Outcome,
}

// The ordered test bag — reorder these lines to change what runs when.
static TESTS: &[Test] = &[
    Test { name: "bus scan", run: t_scan },
    Test { name: "device inventory", run: t_devices },
    Test { name: "MC6470 gravity", run: t_gravity },
    Test { name: "Si7021 T/RH", run: t_temphum },
];

/// Bus scan + config-drift: Fail if any unexpected address ACKed.
fn t_scan(p: &Peripherals) -> Outcome {
    if scan(p) > 0 {
        Outcome::Fail
    } else {
        Outcome::Pass
    }
}

/// Device inventory: presence + WHO_AM_I over the registry. Fail if anything present is faulty.
fn t_devices(p: &Peripherals) -> Outcome {
    let (present, total, faults) = check_devices(p);
    let missing = total - present;
    uart::puts(p, "  inventory: ");
    uart::dec(p, present);
    uart::puts(p, "/");
    uart::dec(p, total);
    uart::puts(p, " present");
    if missing > 0 {
        uart::puts(p, ", ");
        uart::dec(p, missing);
        uart::puts(p, " missing");
    }
    uart::puts(p, "\n");
    if faults > 0 {
        Outcome::Fail
    } else {
        Outcome::Pass
    }
}

/// MC6470 accel gravity-sanity (|a| ~ 1 g). Skip when absent (optional device), else Pass/Fail.
fn t_gravity(p: &Peripherals) -> Outcome {
    // Same shared driver prod uses (devices::mc6470) over diag's embedded-hal bus seam — no fork.
    let mut bus = EusciI2c::new(p);
    if !devices::present(&mut bus, MC6470_ADDR) {
        uart::puts(p, "  MC6470 accel: absent (skip)\n");
        return Outcome::Skip;
    }
    uart::puts(p, "  MC6470 accel: ");
    match Mc6470::measure(&mut bus) {
        Ok(a) => {
            uart::puts(p, "x=");
            uart::dec_i16(p, a.x);
            uart::puts(p, " y=");
            uart::dec_i16(p, a.y);
            uart::puts(p, " z=");
            uart::dec_i16(p, a.z);
            uart::puts(p, " |a|=");
            uart::dec(p, a.magnitude_mg() as u16);
            uart::puts(p, "mg ");
            if a.is_gravity() {
                uart::puts(p, "OK\n");
                Outcome::Pass
            } else {
                uart::puts(p, "OUT-OF-RANGE\n");
                Outcome::Fail
            }
        }
        // Not woken yet (cold-boot wake latency) — self-corrects next pass; skip, don't false-FAIL.
        Err(Error::NotReady) => {
            uart::puts(p, "not ready (wake latency), skip\n");
            Outcome::Skip
        }
        Err(_) => {
            uart::puts(p, "enable/read FAILED\n");
            Outcome::Fail
        }
    }
}

/// Si7021 temperature + humidity — read the REAL values. Triggers a no-hold RH measurement, reads
/// it back with a CRC-8 check, pulls temperature from the same acquisition, and identifies the part
/// (SNB_3 = 0x15). Skip when absent (optional device); Fail only if a present sensor gives a bad CRC
/// or an out-of-envelope reading (a plausible number that fails CRC is worse than a missing one).
fn t_temphum(p: &Peripherals) -> Outcome {
    // Same shared driver prod uses (devices::si7021) over diag's embedded-hal bus seam — no fork.
    let mut bus = EusciI2c::new(p);
    if !devices::present(&mut bus, SI7021_ADDR) {
        uart::puts(p, "  Si7021: absent (skip)\n");
        return Outcome::Skip;
    }
    if let Ok(id) = Si7021::part_id(&mut bus) {
        uart::puts(p, "  part=0x");
        uart::hex8(p, id);
        uart::puts(p, if id == PART_SI7021 {
            " (Si7021)"
        } else {
            " (HTU21/SHT21 family?)" // 0xF5/0xF3 shared; only 0x40+values must be right
        });
        if let Ok(rev) = Si7021::firmware_rev(&mut bus) {
            uart::puts(p, " fw=0x");
            uart::hex8(p, rev);
        }
        uart::puts(p, "\n");
    }
    match Si7021::measure(&mut bus) {
        Ok(r) => {
            uart::puts(p, "  T=");
            uart::fixed2(p, r.temp_c_centi);
            uart::puts(p, " C  RH=");
            uart::fixed2(p, r.rh_centi);
            uart::puts(p, " %  crc ");
            uart::puts(p, if r.crc_ok { "OK\n" } else { "BAD\n" });
            // Verdict: CRC must pass and both values within the sensor's operating envelope
            // (-40..+125 °C, 0..100 %RH). Catches a live-but-garbling bus, not just a NACK.
            let sane = r.crc_ok
                && r.temp_c_centi > -4000
                && r.temp_c_centi < 12500
                && (0..=10000).contains(&r.rh_centi);
            if sane {
                Outcome::Pass
            } else {
                Outcome::Fail
            }
        }
        Err(_) => {
            uart::puts(p, "  Si7021 read FAILED\n");
            Outcome::Fail
        }
    }
}

// The POST test bag is driven cooperatively by `tasks::PostTask` — one test per scheduler tick — so
// the OLED can show `DIAG N of M` live and the ~1.5 s scan no longer hogs the loop. These are the
// pieces it orchestrates:

/// Number of tests in the bag.
pub fn test_count() -> usize {
    TESTS.len()
}

/// Run test `i` (prints its own section header + body) and return its outcome.
pub fn run_test(p: &Peripherals, i: usize) -> Outcome {
    uart::puts(p, "· "); // section header names the test (the registry identifier)
    uart::puts(p, TESTS[i].name);
    uart::puts(p, "\n");
    (TESTS[i].run)(p)
}

/// The current test's short name (for the OLED progress line).
pub fn test_name(i: usize) -> &'static str {
    TESTS[i].name
}

/// POST banner with the build stamp — printed once at the start of each pass. `env!` resolves it
/// from build.rs's DIAG_BUILD, so a technician who attached the monitor late can confirm the running
/// firmware within one cycle.
pub fn banner(p: &Peripherals) {
    uart::puts(p, "\n=== bob-929 diag POST · build ");
    uart::puts(p, option_env!("DIAG_BUILD").unwrap_or("dev"));
    uart::puts(p, " ===\n");
}

/// The summary line at the end of a pass.
pub fn summary(p: &Peripherals, pass: u16, fail: u16, skip: u16) {
    uart::puts(p, "summary: ");
    uart::dec(p, pass);
    uart::puts(p, " passed");
    if fail > 0 {
        uart::puts(p, ", ");
        uart::dec(p, fail);
        uart::puts(p, " FAILED");
    }
    if skip > 0 {
        uart::puts(p, ", ");
        uart::dec(p, skip);
        uart::puts(p, " skipped");
    }
    uart::puts(p, "\n");
}

/// LED-matrix verdict glyph (the OLED is rendered by UiTask). `ok` = no test FAILED. The bus-level
/// probe after the write catches a wedged bus.
pub fn led_verdict(p: &Peripherals, ok: bool) {
    // The LED matrix is just another I²C device (`devices::is31`), painted over the Leaf bus seam.
    let mut bus = EusciI2c::new(p);
    let m = devices::is31::Matrix::default_strap();
    if devices::present(&mut bus, m.addr()) {
        let i = m.init(&mut bus).is_ok();
        let s = m.show(&mut bus, if ok { &GLYPH_CHECK } else { &GLYPH_CROSS }).is_ok();
        uart::puts(p, "  IS31 init ");
        uart::puts(p, if i { "OK" } else { "FAIL" });
        uart::puts(p, " show ");
        uart::puts(p, if s { "OK" } else { "FAIL" });
        uart::puts(p, "\n");
        bus_lvl(p, "  bus after IS31: ");
    }
}

/// One-line SDA/SCL pad level (P1IN reflects the real line even when the pins are muxed to eUSCI).
/// Used to localize which write leaves the bus low.
fn bus_lvl(p: &Peripherals, label: &str) {
    let lv = p.p1.p1in().read().bits();
    uart::puts(p, label);
    uart::putc(p, if lv & 0x04 != 0 { b'H' } else { b'L' }); // SDA / P1.2
    uart::putc(p, b'/');
    uart::putc(p, if lv & 0x08 != 0 { b'H' } else { b'L' }); // SCL / P1.3
    uart::puts(p, "\n");
}
