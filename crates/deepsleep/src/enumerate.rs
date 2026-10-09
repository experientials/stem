//! Sensor-bus enumeration — scan the local I2C bus (eUSCI_B0 master) through the shared [`devices`]
//! library, with **bus-health detection** so a wedged bus is reported as a fault instead of 112
//! phantom devices. The scan result feeds the debug registers (always); the human report is
//! `console`-gated (dev only). This is the enumerate-the-bus / report-state slice.

use core::cell::Cell;

// De-welded from the concrete MSP430 PAC/bsp (DEEPSLEEP-EXTRACTION.md Phase 6b): the bus ops go through
// the host-clean `node_hal::LeafBus` seam and the console report through `node_hal::Console`. `bsp::StemHw`
// is the sole MSP430 impl; the bin constructs it and passes `&hw`. No `bsp::` type is named here.
use node_hal::LeafBus;
#[cfg(feature = "console")]
use node_hal::Console;
#[cfg(feature = "console")]
use devices::{
    apds9960::Apds9960,
    mc6470::{Mc6470, Mc6470Mag},
    si7021::Si7021,
    vl53l0x::Vl53l0x,
    Device, Error,
};
// Ranging types are used ONLY by the --debug async `vl53l0x_range_demo` (report keeps just the
// identity read) — gate them on `debug` so a console-but-not-debug build has no unused imports.
#[cfg(feature = "debug")]
use devices::vl53l0x::{Attention, RangeTracker, Vl53l0xRanging};

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
pub fn scan<H: LeafBus>(hw: &H) -> Scan {
    // Always attempt recovery (recover() fast-paths when already idle and clears an SCL-low wedge as
    // well as SDA-low); its result is true only when BOTH lines ended idle-high.
    let recovered = hw.recover();

    let mut bus = hw.bus();
    let mut pr = Presence::new();
    let mut a = LO;
    while a <= HI {
        if devices::present(&mut bus, a) {
            pr.set(a);
        }
        a += 1;
    }

    let (sda1, scl1) = hw.bus_levels();
    let faulted = !recovered || !(sda1 && scl1) || pr.count() > SANITY_MAX;
    Scan { present: pr, faulted }
}

/// Async Leaf scan (ADR-0013 — replaces the hand-written `ScanJob` state machine). Same probing /
/// recovery / fault logic as [`scan`], but `.await`s [`sched::yield_now`] after each probe so
/// `stem::poll` + the console keep breathing (the never-block contract, ADR-0006). Checks `cancel`
/// each probe and aborts (returns `None`) the instant we leave Sensing — critical: a released sensor
/// bus must never be driven against its new owner. Holds only `p` (shared `&`) + its own small state
/// across awaits — never `rf` — and RETURNS the `Scan`, so it can't conflict with the slave's
/// `&mut rf` (the borrow rule; docs/TASK-STYLE-GUIDE.md).
pub async fn scan_async<H: LeafBus>(hw: &H, cancel: &Cell<bool>) -> Option<Scan> {
    let recovered = hw.recover();
    let mut pr = Presence::new();
    let mut a = LO;
    while a <= HI {
        if cancel.get() {
            return None; // left Sensing mid-scan — drop the bus immediately
        }
        {
            let mut bus = hw.bus();
            if devices::present(&mut bus, a) {
                pr.set(a);
            }
        } // `bus` dropped before the await — nothing borrowed across the yield but `hw`/`pr`/`a`
        a += 1;
        sched::yield_now().await;
    }
    let (sda1, scl1) = hw.bus_levels();
    let faulted = !recovered || !(sda1 && scl1) || pr.count() > SANITY_MAX;
    Some(Scan { present: pr, faulted })
}

/// Long-lived Leaf-scan TASK (pinned once by the caller, polled every main-loop pass). Idles —
/// yielding — until `arm` is set (a switch into Sensing), runs one [`scan_async`], deposits the
/// result in `out` for the loop to publish into the RegFile, then re-arms for the next Sensing entry.
/// This is the async-native `sched` task that replaces the loop's manual `ScanJob` threading: ONE
/// future, no per-arm re-creation, no `alloc`, no unsafe pin. The `Cell`s are the mailbox — touched
/// only at yield boundaries, never held across `.await`.
// Bound `LeafBus + Console`: the bus scan needs `LeafBus`; the `#[cfg(debug)]` ranging demo additionally
// needs `Console` to print. The `Console` bound is unconditional (a `#[cfg]` can't gate one `where`
// predicate) but harmless — `bsp::StemHw` impls `Console` for every build, so it's always satisfiable;
// non-debug builds just never call a `Console` method here.
pub async fn leaf_scanner<H: LeafBus + Console>(
    hw: &H,
    arm: &Cell<bool>,
    cancel: &Cell<bool>,
    out: &Cell<Option<Scan>>,
) -> ! {
    loop {
        while !arm.replace(false) {
            sched::yield_now().await; // idle until a Sensing switch arms a scan
        }
        cancel.set(false);
        if let Some(scan) = scan_async(hw, cancel).await {
            // --debug: async wake-on-approach ranging demo if a VL53L0X answered. It yields between
            // samples, so the Stem slave stays responsive during the ~5 s window (ADR-0006). Then publish.
            #[cfg(feature = "debug")]
            if !scan.faulted && scan.present.get(Vl53l0x::ADDR) && !cancel.get() {
                vl53l0x_range_demo(hw, cancel).await;
            }
            out.set(Some(scan)); // the loop applies it (only while still Sensing)
        }
    }
}

/// Report the enumeration over UART (console only). Leads with the bus level; on a fault it says so
/// and prints the ACK count instead of 112 bogus per-address lines. Known devices are labeled.
#[cfg(feature = "console")]
pub fn report<H: LeafBus + Console>(hw: &H, scan: &Scan) {
    let (sda, scl) = hw.bus_levels();
    hw.puts("bus SDA=");
    hw.putc(if sda { b'H' } else { b'L' });
    hw.puts(" SCL=");
    hw.putc(if scl { b'H' } else { b'L' });

    if scan.faulted {
        hw.puts("  !! BUS FAULT (SDA/SCL stuck / all-ACK) — scan unreliable [");
        hw.dec(scan.present.count());
        hw.puts(" ACKs]\n");
        return;
    }

    hw.puts("\nI2C bus:");
    let mut a = LO;
    let mut any = false;
    while a <= HI {
        if scan.present.get(a) {
            any = true;
            hw.puts(" 0x");
            hw.hex8(a);
            match devices::known(a) {
                Some(k) => {
                    hw.putc(b'(');
                    hw.puts(k.name);
                    hw.putc(b')');
                }
                None => hw.putc(b'?'),
            }
        }
        a += 1;
    }
    if !any {
        hw.puts(" (none)");
    }
    hw.puts("  [");
    hw.dec(scan.present.count());
    hw.puts(" found]\n");

    // Exercise the first real shared Device driver end-to-end: WHO_AM_I + a live proximity read.
    // Only when the sensor actually ACKed and the bus is healthy (a faulted scan is meaningless).
    if !scan.faulted && scan.present.get(Apds9960::ADDR) {
        let mut bus = hw.bus();
        hw.puts("APDS-9960: ");
        match Apds9960::identify(&mut bus) {
            Ok(()) => hw.puts("id ok"),
            Err(_) => hw.puts("id FAIL"),
        }
        match Apds9960::measure(&mut bus) {
            Ok(prox) => {
                hw.puts("  prox=");
                hw.dec(prox as u16);
            }
            Err(Error::NotReady) => hw.puts("  prox=not-ready"),
            Err(_) => hw.puts("  prox=err"),
        }
        hw.putc(b'\n');
    }

    // Second shared Device: Si7021/HTU21/SHT21 T/RH — WHO-responds + a live no-hold read.
    if !scan.faulted && scan.present.get(Si7021::ADDR) {
        let mut bus = hw.bus();
        hw.puts("Si7021: ");
        match Si7021::identify(&mut bus) {
            Ok(()) => hw.puts("id ok"),
            Err(_) => hw.puts("id FAIL"),
        }
        match Si7021::measure(&mut bus) {
            Ok(r) => {
                hw.puts("  T=");
                hw.fixed2(r.temp_c_centi);
                hw.puts("C RH=");
                hw.fixed2(r.rh_centi);
                hw.puts("% crc ");
                hw.puts(if r.crc_ok { "OK" } else { "BAD" });
            }
            Err(Error::NotReady) => hw.puts("  read not-ready"),
            Err(_) => hw.puts("  read err"),
        }
        hw.putc(b'\n');
    }

    // Third shared Device: MC6470 accel — gravity-sanity (|a| ~ 1 g) via the shared driver math.
    if !scan.faulted && scan.present.get(Mc6470::ADDR) {
        let mut bus = hw.bus();
        hw.puts("MC6470: ");
        match Mc6470::measure(&mut bus) {
            Ok(a) => {
                hw.puts("x=");
                hw.dec_i16(a.x);
                hw.puts(" y=");
                hw.dec_i16(a.y);
                hw.puts(" z=");
                hw.dec_i16(a.z);
                hw.puts(" |a|=");
                hw.dec(a.magnitude_mg() as u16);
                hw.puts("mg ");
                hw.puts(if a.is_gravity() { "OK" } else { "OUT-OF-RANGE" });
            }
            Err(Error::NotReady) => hw.puts("not ready (wake latency)"),
            Err(_) => hw.puts("read err"),
        }
        hw.putc(b'\n');
    }

    // MC6470 magnetometer (0x0C) — the eCompass mag half: WHO_AM_I + a forced field read.
    if !scan.faulted && scan.present.get(Mc6470Mag::ADDR) {
        let mut bus = hw.bus();
        hw.puts("MC6470 mag: ");
        match Mc6470Mag::identify(&mut bus) {
            Ok(()) => hw.puts("id ok"),
            Err(_) => hw.puts("id FAIL"),
        }
        match Mc6470Mag::measure(&mut bus) {
            Ok(m) => {
                hw.puts("  x=");
                hw.dec_i16(m.x);
                hw.puts(" y=");
                hw.dec_i16(m.y);
                hw.puts(" z=");
                hw.dec_i16(m.z);
                hw.puts(" |B|=");
                hw.fixed2(m.magnitude_centi_ut());
                hw.puts("uT");
            }
            Err(Error::NotReady) => hw.puts("  not ready"),
            Err(_) => hw.puts("  read err"),
        }
        hw.putc(b'\n');
    }

    // Fourth shared Device: VL53L0X ToF — identity level (WHO_AM_I 0xEE); ranging is a tracked TODO.
    if !scan.faulted && scan.present.get(Vl53l0x::ADDR) {
        let mut bus = hw.bus();
        hw.puts("VL53L0X: ");
        match Vl53l0x::measure(&mut bus) {
            Ok(ids) => {
                hw.puts("id ok  model=");
                hw.hex8(ids.model);
                hw.puts(" rev=");
                hw.hex8(ids.revision);
            }
            Err(Error::Identity) => hw.puts("id FAIL (wrong model)"),
            Err(_) => hw.puts("read err"),
        }
        hw.putc(b'\n');
        // The ~5 s wake-on-approach ranging demo moved OUT of this synchronous report into the async
        // `vl53l0x_range_demo` (run by `leaf_scanner` on a Sensing scan completion, --debug only) — so
        // it no longer blocks the Stem slave for 5 s (ADR-0006).
    }
}

/// Async VL53L0X wake-on-approach ranging demo (--debug). Moved out of the synchronous `report` so it
/// no longer stalls the Stem slave for ~5 s (ADR-0006): the per-sample pacing burns the ~100 ms in
/// nop-chunks separated by `sched::yield_now().await`, so the serve loop services `stem::poll`
/// ~10×/sample while the ~5 s hand-wave window is preserved. Aborts immediately if `cancel` is set
/// (we left Sensing → must stop driving the bus). Run by [`leaf_scanner`] on a Sensing scan completion.
#[cfg(feature = "debug")]
async fn vl53l0x_range_demo<H: LeafBus + Console>(hw: &H, cancel: &Cell<bool>) {
    let mut bus = hw.bus();
    match Vl53l0xRanging::init(&mut bus) {
        Ok(rng) => {
            hw.puts("VL53L0X range (move a hand toward it):\n");
            let mut tracker = RangeTracker::new(300, 30); // near <30cm, 3cm trend hysteresis
            let mut i = 0u8;
            while i < 40 {
                if cancel.get() {
                    break; // left Sensing mid-demo — stop driving the sensor bus
                }
                // ~100 ms pacing that STILL services the slave: burn the time in nop-chunks with a
                // cooperative yield between each, so `stem::poll` runs ~10×/sample (vs the old 5 s stall).
                // The `msp430::asm::nop()` pacing stays (dropping the `msp430` dep is not a 6b goal).
                for _ in 0..10u16 {
                    for _ in 0..3_000u16 {
                        msp430::asm::nop();
                    }
                    sched::yield_now().await;
                }
                match rng.read_range(&mut bus) {
                    Ok(mm) => {
                        tracker.push(mm);
                        hw.puts("  ");
                        hw.dec(mm);
                        hw.puts("mm ");
                        hw.puts(match tracker.classify() {
                            Attention::NoTarget => "-",
                            Attention::Approaching => "APPROACHING",
                            Attention::Receding => "receding",
                            Attention::Stationary => "stationary",
                        });
                        if tracker.is_near() {
                            hw.puts(" NEAR");
                        }
                        hw.putc(b'\n');
                    }
                    Err(_) => hw.puts("  read err\n"),
                }
                i += 1;
            }
        }
        Err(_) => hw.puts("VL53L0X ranging init FAILED\n"),
    }
}
