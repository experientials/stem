//! Sensor-bus enumeration — scan the local I2C bus (eUSCI_B0 master) through the shared [`devices`]
//! library, with **bus-health detection** so a wedged bus is reported as a fault instead of 112
//! phantom devices. The scan result feeds the debug registers (always); the human report is
//! `console`-gated (dev only). This is the enumerate-the-bus / report-state slice.

use crate::i2c;
use crate::hal::EusciI2c;
#[cfg(feature = "console")]
use crate::uart;
#[cfg(feature = "console")]
use devices::{
    apds9960::Apds9960,
    mc6470::{Mc6470, Mc6470Mag},
    si7021::Si7021,
    vl53l0x::{Attention, RangeTracker, Vl53l0x, Vl53l0xRanging},
    Device, Error,
};
use crate::pac::Peripherals;

const LO: u8 = 0x08; // first 7-bit I2C address we scan
const HI: u8 = 0x77; // last (0x00-0x07 and 0x78-0x7F are reserved)

/// A real sensor bus won't have this many devices — more ⇒ a stuck SDA making every probe false-ACK.
const SANITY_MAX: u16 = 24;

/// Which 7-bit addresses ACKed on the last scan — a 128-bit presence bitmap (the reportable state).
pub struct Presence {
    bits: [u8; 16],
}

impl Presence {
    pub const fn new() -> Self {
        Self { bits: [0; 16] }
    }
    fn set(&mut self, a: u8) {
        self.bits[(a >> 3) as usize] |= 1 << (a & 7);
    }
    pub fn get(&self, a: u8) -> bool {
        self.bits[(a >> 3) as usize] & (1 << (a & 7)) != 0
    }
    pub fn count(&self) -> u16 {
        self.bits.iter().map(|b| b.count_ones() as u16).sum()
    }
}

/// Outcome of a scan: the presence bitmap plus whether the bus looked wedged (results unreliable).
pub struct Scan {
    pub present: Presence,
    pub faulted: bool,
}

impl Scan {
    /// A "no scan happened" result — used in Passive mode, where the MSP never masters the sensor
    /// bus. Empty presence, not faulted (nothing was probed).
    pub const fn absent() -> Self {
        Self {
            present: Presence::new(),
            faulted: false,
        }
    }
}

/// Scan the sensor bus. First recovers a wedged bus (either a slave holding SDA low or our own eUSCI
/// holding SCL low both make the scan unreliable), then probes 0x08..=0x77 via `devices::present`, and
/// flags `faulted` if the bus is still not idle-high on *both* lines or the hit count is implausibly
/// high. Populates state read by the debug registers — always runs (not console-gated); the
/// *reporting* is what's gated.
pub fn scan(p: &Peripherals) -> Scan {
    // Always attempt recovery (recover() fast-paths when already idle and clears an SCL-low wedge as
    // well as SDA-low); its result is true only when BOTH lines ended idle-high.
    let recovered = i2c::recover(p);

    let mut bus = EusciI2c::new(p);
    let mut pr = Presence::new();
    let mut a = LO;
    while a <= HI {
        if devices::present(&mut bus, a) {
            pr.set(a);
        }
        a += 1;
    }

    let (sda1, scl1) = i2c::bus_levels(p);
    let faulted = !recovered || !(sda1 && scl1) || pr.count() > SANITY_MAX;
    Scan { present: pr, faulted }
}

/// An **incremental** sensor-bus scan (ADR-0006). Same probing as [`scan`], but split so the caller
/// advances it a few addresses per main-loop pass — `stem::poll` runs between steps, so the Stem slave
/// + console never stall on a mode switch (the inline `scan` blocked them; on a wedged bus, badly).
/// One-shot bus recovery happens on the first [`step`](ScanJob::step); the result is published once, when
/// the address window is exhausted.
pub struct ScanJob {
    pr: Presence,
    next: u8,         // next 7-bit address to probe; the job is complete once this passes HI
    recovered: bool,  // did the one-shot recovery leave the bus idle-high (both lines)?
    started: bool,    // has the first step run the recovery yet?
}

impl ScanJob {
    pub const fn new() -> Self {
        Self {
            pr: Presence::new(),
            next: LO,
            recovered: false,
            started: false,
        }
    }

    /// Advance up to `budget` address probes. Returns `Some(Scan)` exactly once — on the step that
    /// finishes the window — and `None` while more probing remains (or if called again after finishing).
    pub fn step(&mut self, p: &Peripherals, budget: u8) -> Option<Scan> {
        if self.next > HI {
            return None; // already complete (and reported)
        }
        if !self.started {
            // One-shot recovery up front — a wedged bus (SDA-low slave OR SCL-low master self-hold)
            // would false-ACK every probe. recover() handles both and returns true only when idle-high.
            self.recovered = i2c::recover(p);
            self.started = true;
        }
        let mut bus = EusciI2c::new(p);
        let mut did = 0u8;
        while self.next <= HI && did < budget {
            if devices::present(&mut bus, self.next) {
                self.pr.set(self.next);
            }
            self.next += 1;
            did += 1;
        }
        if self.next > HI {
            let (sda1, scl1) = i2c::bus_levels(p);
            let faulted = !self.recovered || !(sda1 && scl1) || self.pr.count() > SANITY_MAX;
            let present = core::mem::replace(&mut self.pr, Presence::new());
            return Some(Scan { present, faulted });
        }
        None
    }
}

/// Report the enumeration over UART (console only). Leads with the bus level; on a fault it says so
/// and prints the ACK count instead of 112 bogus per-address lines. Known devices are labeled.
#[cfg(feature = "console")]
pub fn report(p: &Peripherals, scan: &Scan) {
    let (sda, scl) = i2c::bus_levels(p);
    uart::puts(p, "bus SDA=");
    uart::putc(p, if sda { b'H' } else { b'L' });
    uart::puts(p, " SCL=");
    uart::putc(p, if scl { b'H' } else { b'L' });

    if scan.faulted {
        uart::puts(p, "  !! BUS FAULT (SDA/SCL stuck / all-ACK) — scan unreliable [");
        uart::dec(p, scan.present.count());
        uart::puts(p, " ACKs]\n");
        return;
    }

    uart::puts(p, "\nI2C bus:");
    let mut a = LO;
    let mut any = false;
    while a <= HI {
        if scan.present.get(a) {
            any = true;
            uart::puts(p, " 0x");
            uart::hex8(p, a);
            match devices::known(a) {
                Some(k) => {
                    uart::putc(p, b'(');
                    uart::puts(p, k.name);
                    uart::putc(p, b')');
                }
                None => uart::putc(p, b'?'),
            }
        }
        a += 1;
    }
    if !any {
        uart::puts(p, " (none)");
    }
    uart::puts(p, "  [");
    uart::dec(p, scan.present.count());
    uart::puts(p, " found]\n");

    // Exercise the first real shared Device driver end-to-end: WHO_AM_I + a live proximity read.
    // Only when the sensor actually ACKed and the bus is healthy (a faulted scan is meaningless).
    if !scan.faulted && scan.present.get(Apds9960::ADDR) {
        let mut bus = EusciI2c::new(p);
        uart::puts(p, "APDS-9960: ");
        match Apds9960::identify(&mut bus) {
            Ok(()) => uart::puts(p, "id ok"),
            Err(_) => uart::puts(p, "id FAIL"),
        }
        match Apds9960::measure(&mut bus) {
            Ok(prox) => {
                uart::puts(p, "  prox=");
                uart::dec(p, prox as u16);
            }
            Err(Error::NotReady) => uart::puts(p, "  prox=not-ready"),
            Err(_) => uart::puts(p, "  prox=err"),
        }
        uart::putc(p, b'\n');
    }

    // Second shared Device: Si7021/HTU21/SHT21 T/RH — WHO-responds + a live no-hold read.
    if !scan.faulted && scan.present.get(Si7021::ADDR) {
        let mut bus = EusciI2c::new(p);
        uart::puts(p, "Si7021: ");
        match Si7021::identify(&mut bus) {
            Ok(()) => uart::puts(p, "id ok"),
            Err(_) => uart::puts(p, "id FAIL"),
        }
        match Si7021::measure(&mut bus) {
            Ok(r) => {
                uart::puts(p, "  T=");
                uart::fixed2(p, r.temp_c_centi);
                uart::puts(p, "C RH=");
                uart::fixed2(p, r.rh_centi);
                uart::puts(p, "% crc ");
                uart::puts(p, if r.crc_ok { "OK" } else { "BAD" });
            }
            Err(Error::NotReady) => uart::puts(p, "  read not-ready"),
            Err(_) => uart::puts(p, "  read err"),
        }
        uart::putc(p, b'\n');
    }

    // Third shared Device: MC6470 accel — gravity-sanity (|a| ~ 1 g) via the shared driver math.
    if !scan.faulted && scan.present.get(Mc6470::ADDR) {
        let mut bus = EusciI2c::new(p);
        uart::puts(p, "MC6470: ");
        match Mc6470::measure(&mut bus) {
            Ok(a) => {
                uart::puts(p, "x=");
                dec_i16(p, a.x);
                uart::puts(p, " y=");
                dec_i16(p, a.y);
                uart::puts(p, " z=");
                dec_i16(p, a.z);
                uart::puts(p, " |a|=");
                uart::dec(p, a.magnitude_mg() as u16);
                uart::puts(p, "mg ");
                uart::puts(p, if a.is_gravity() { "OK" } else { "OUT-OF-RANGE" });
            }
            Err(Error::NotReady) => uart::puts(p, "not ready (wake latency)"),
            Err(_) => uart::puts(p, "read err"),
        }
        uart::putc(p, b'\n');
    }

    // MC6470 magnetometer (0x0C) — the eCompass mag half: WHO_AM_I + a forced field read.
    if !scan.faulted && scan.present.get(Mc6470Mag::ADDR) {
        let mut bus = EusciI2c::new(p);
        uart::puts(p, "MC6470 mag: ");
        match Mc6470Mag::identify(&mut bus) {
            Ok(()) => uart::puts(p, "id ok"),
            Err(_) => uart::puts(p, "id FAIL"),
        }
        match Mc6470Mag::measure(&mut bus) {
            Ok(m) => {
                uart::puts(p, "  x=");
                dec_i16(p, m.x);
                uart::puts(p, " y=");
                dec_i16(p, m.y);
                uart::puts(p, " z=");
                dec_i16(p, m.z);
                uart::puts(p, " |B|=");
                uart::fixed2(p, m.magnitude_centi_ut());
                uart::puts(p, "uT");
            }
            Err(Error::NotReady) => uart::puts(p, "  not ready"),
            Err(_) => uart::puts(p, "  read err"),
        }
        uart::putc(p, b'\n');
    }

    // Fourth shared Device: VL53L0X ToF — identity level (WHO_AM_I 0xEE); ranging is a tracked TODO.
    if !scan.faulted && scan.present.get(Vl53l0x::ADDR) {
        let mut bus = EusciI2c::new(p);
        uart::puts(p, "VL53L0X: ");
        match Vl53l0x::measure(&mut bus) {
            Ok(ids) => {
                uart::puts(p, "id ok  model=");
                uart::hex8(p, ids.model);
                uart::puts(p, " rev=");
                uart::hex8(p, ids.revision);
            }
            Err(Error::Identity) => uart::puts(p, "id FAIL (wrong model)"),
            Err(_) => uart::puts(p, "read err"),
        }
        uart::putc(p, b'\n');

        // Supervisor wake-on-approach: init ranging, then a short burst so a hand moving toward/away
        // shows APPROACHING / receding / near. (Coarse ToF trend — see devices::vl53l0x.)
        match Vl53l0xRanging::init(&mut bus) {
            Ok(rng) => {
                uart::puts(p, "VL53L0X range (move a hand toward it):\n");
                let mut tracker = RangeTracker::new(300, 30); // near <30cm, 3cm trend hysteresis
                let mut i = 0u8;
                while i < 40 {
                    // ~100 ms between samples → a ~5 s window to move a hand and watch the trend.
                    for _ in 0..30_000u16 {
                        msp430::asm::nop();
                    }
                    match rng.read_range(&mut bus) {
                        Ok(mm) => {
                            tracker.push(mm);
                            uart::puts(p, "  ");
                            uart::dec(p, mm);
                            uart::puts(p, "mm ");
                            uart::puts(
                                p,
                                match tracker.classify() {
                                    Attention::NoTarget => "-",
                                    Attention::Approaching => "APPROACHING",
                                    Attention::Receding => "receding",
                                    Attention::Stationary => "stationary",
                                },
                            );
                            if tracker.is_near() {
                                uart::puts(p, " NEAR");
                            }
                            uart::putc(p, b'\n');
                        }
                        Err(_) => uart::puts(p, "  read err\n"),
                    }
                    i += 1;
                }
            }
            Err(_) => uart::puts(p, "VL53L0X ranging init FAILED\n"),
        }
    }
}

/// Signed decimal for the accel axes (`uart::dec` is unsigned). Axes are ±8191, so `-v` fits i16.
#[cfg(feature = "console")]
fn dec_i16(p: &Peripherals, v: i16) {
    if v < 0 {
        uart::putc(p, b'-');
        uart::dec(p, (-v) as u16);
    } else {
        uart::dec(p, v as u16);
    }
}
