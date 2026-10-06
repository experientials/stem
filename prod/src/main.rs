#![no_main]
#![no_std]
#![feature(asm_experimental_arch)] // core::arch::asm! for SCG0 in clock.rs (fr247x); harmless otherwise

//! bob-929 PRODUCT firmware (Rust) — **Stembus node**, primary dev target MSP430FR2476 (FR247x).
//!
//! DEV SEQUENCING (Henrik, 2026-09-21): developed now on the on-hand **FR2476** boards — a **dual-I2C**
//! part (2× eUSCI_B), so it exercises the full node role: a **Stem-bus I2C SLAVE** control surface to
//! the SoM (regmap.rs) *and* a local **sensor-bus I2C MASTER**. The production dual-I2C part is the
//! **FR2155** (FR215x); the port is a PAC/memory.x swap once an FR2355 board (its strict superset) is
//! available to sign off — the one thing FR2476 can't test is the 2nd eCOMP (1 vs 2). The cheap 1× I2C
//! **FR2433** slave-only node is a separate build (the family axis) added later. See prod/DESIGN.md +
//! docs/MCU_SELECTION.md.
//!
//! `just prod build` gates the image against the FR247x 32 KB toolchain window / 8 KB SRAM
//! (scripts/size-check.sh).
//!
//! STATUS: scaffold only. Boots, holds the watchdog, unlocks GPIO, and idles. The node duties below
//! are stubs — see prod/README.md and derive them from STEM-EXPANDER.md / I2C-API.md / objectives.
//!
//! Register access goes through the vendored `msp430fr2476` PAC (typed peripherals), same family as
//! diag — so diag's clock.rs / uart.rs / i2c.rs are direct references. Diag runs eUSCI_B as a MASTER;
//! the MCU-bus surface here must be an I2C SLAVE (a different code path), while the sensor bus reuses
//! diag's master path. `p.cs` is the clock system; `p.e_usci_b0`/`p.e_usci_b1` are the two I2C blocks.

extern crate panic_msp430; // infinitely-looping panic handler (a hang trips the watchdog → reset)

use msp430_rt::entry;

// Family axis: exactly one family feature selects the PAC (and, via build.rs, the memory map).
#[cfg(not(any(feature = "_dual", feature = "fr24xx")))]
compile_error!("prod: enable one family — fr247x (default) | fr215x | fr235x | fr24xx");
#[cfg(all(feature = "_dual", feature = "fr24xx"))]
compile_error!("prod: enable exactly ONE family — a dual-I²C family XOR fr24xx, not both");

// OPERATING MODES (DESIGN.md): compile in ≥1 mode; sensing needs a 2nd I²C so it's dual-I²C only.
#[cfg(not(any(feature = "mode-passive", feature = "mode-sensing")))]
compile_error!("prod: enable at least one mode — `--features mode-passive` and/or `mode-sensing`");
#[cfg(all(feature = "fr24xx", feature = "mode-sensing"))]
compile_error!("prod: `mode-sensing` requires a dual-I²C family — FR2433 (fr24xx) is Passive-only");

// The chip-select PAC alias + the shared board/hal/i2c layer live in `bsp` (STEM-DIRECTION.md: diag
// and prod share ONE family/board axis). Re-export `pac` so every module keeps
// `use crate::pac::Peripherals`; the family feature forwards to exactly one bsp PAC. `hal`/`i2c` (the
// dual-I²C Leaf bus) are re-exported for the dual families — the SAME shared driver diag uses.
pub(crate) use bsp::pac;
use crate::pac::Peripherals;
#[cfg(feature = "_dual")]
pub(crate) use bsp::{hal, i2c};
#[cfg(feature = "_dual")]
use core::cell::Cell; // async Leaf-scanner mailbox (arm/cancel/result) — see run_dual's serve loop
#[cfg(feature = "_dual")]
use core::future::Future; // for `.poll()` on the pinned scanner future in the serve loop

pub use bsp::model; // Runtime MSP430 model detection + pin mapping — now shared in bsp (STEM-DIRECTION.md
                    // "model-detect + pinmap → bsp"); both prod boot and diag POST read one detector.
mod regmap; // I²C-slave register-map contract (PCA9698 emulation + Thepia extensions). See regmap.rs.

// DUAL-I²C bring-up modules — shared across all dual families (fr247x/fr215x/fr235x, the `_dual`
// marker). They talk to the chip through the `pac` alias, so the SAME code serves every dual PAC; only
// the alias differs. The FR24xx (single-I²C) path is a separate idle scaffold. Portable device drivers
// live in `crates/devices`; only this glue is chip-specific.
// clock init serves the dual path AND the fr24xx console path (a locked 1 MHz SMCLK for a clean UART).
#[cfg(any(feature = "_dual", feature = "console"))]
mod clock;
#[cfg(feature = "_dual")]
mod enumerate;
// The I²C-slave surface (status + stem transport + regmap) serves BOTH the dual node (eUSCI_B1) and
// the fr24xx Passive node (its single eUSCI_B0) — it's the fr24xx node's whole job. `enumerate`/`mode`
// stay dual-only (no sensor bus / mode machine on the single-I²C part).
#[cfg(any(feature = "_dual", feature = "fr24xx"))]
mod status;
#[cfg(feature = "_dual")]
mod mode;
#[cfg(any(feature = "_dual", feature = "fr24xx"))]
mod stem;
// UART logging is the `console` feature (dev only). Off = silent production image; state then lives
// only in the debug registers (served by the I2C-slave surface). See status.rs / the console question.
// Console/UART is available to ANY family with `console` on (dual + fr24xx). `uart.rs` unifies the
// eUSCI_A0 field-name difference (e_usci_a0 vs usci_a0_uart_mode) via a macro.
#[cfg(feature = "console")]
mod uart;
// Boot POST: `post::fram_crcs` (per-sector FRAM CRC16) runs on EVERY build incl. silent (ADR-0010 —
// the M7 reads the raw CRCs over I²C and judges). The UART echo (`boot_log`/`debug_leaf`) inside is
// `console`/`debug`-gated. So the module is ALWAYS compiled.
mod post;

/// Compiled-in identity stamp (see build.rs). Printed at boot once the UART stub lands.
const FW_BUILD: &str = env!("PROD_BUILD");

/// Firmware VERSION NUMBER — semantic `major.minor.patch`, sourced from the crate version in
/// `Cargo.toml` (bump it per release). Distinct from `FW_BUILD` (the git/build stamp): the version is
/// the human-facing number, exposed both on the boot banner AND as debug registers `0x3B–0x3D` so the
/// SoM can read the firmware version by inspecting the MSP over the I2C-slave surface.
pub const FW_VERSION: &str = env!("CARGO_PKG_VERSION"); // "0.1.0"
pub const FW_VER_MAJOR: u8 = parse_u8(env!("CARGO_PKG_VERSION_MAJOR"));
pub const FW_VER_MINOR: u8 = parse_u8(env!("CARGO_PKG_VERSION_MINOR"));
pub const FW_VER_PATCH: u8 = parse_u8(env!("CARGO_PKG_VERSION_PATCH"));

/// const decimal parse (0..=255) — for the CARGO_PKG_VERSION_* fields at compile time.
const fn parse_u8(s: &str) -> u8 {
    let b = s.as_bytes();
    let mut v = 0u8;
    let mut i = 0;
    while i < b.len() {
        v = v.wrapping_mul(10).wrapping_add(b[i] - b'0');
        i += 1;
    }
    v
}

// WDT hold/arm/pet + the ~16 s reset-mode backstop policy now live in the `board` role
// (`bsp::board::wdt_hold`/`wdt_arm_backstop`/`wdt_pet`) — one definition shared with diag, with the
// field-name delta (dual `wdt_a` vs fr2433 `watchdog_timer`) owned there, so prod is role-based.
// PM5CTL0.LOCKLPM5 — set out of reset on FRAM parts; must be cleared to make GPIO live.
const LOCKLPM5: u16 = 0x0001;

#[entry]
fn main() -> ! {
    // steal() (not take()) to match diag and avoid pulling in the critical-section machinery; the
    // scaffold is single-threaded with no ISRs yet, so exclusive access is trivially upheld.
    let p = unsafe { Peripherals::steal() };

    // HOLD the watchdog during bring-up (deterministic, unguarded — clock/pins/enumerate/splash);
    // `bsp::board::wdt_arm_backstop` before the serve loop switches it to the ~16 s reset-mode backstop
    // that `bsp::board::wdt_pet` reloads each pass. The per-chip WDT field-name delta lives in the role.
    bsp::board::wdt_hold(&p);

    // Unlock the I/O from the LPM5 default so port config takes effect (both families have PMM).
    p.pmm.pm5ctl0().modify(|r, w| unsafe { w.bits(r.bits() & !LOCKLPM5) });

    // Keep the build stamp live for the family paths below.
    let _ = FW_BUILD;

    #[cfg(feature = "_dual")]
    run_dual(&p);

    #[cfg(feature = "fr24xx")]
    run_fr24xx(&p);
}

/// Dual-I²C node bring-up (fr247x/fr215x/fr235x — via the `pac` alias): clock → pins → sensor-bus I2C
/// master → mode machine → OLED splash → boot banner → enumerate → report/serve state (UART commands +
/// eUSCI_B1 slave). Same code for every dual family; only the PAC alias differs.
#[cfg(feature = "_dual")]
fn run_dual(p: &Peripherals) -> ! {
    // Console UART pins via the board — board-aware (UCA0/P1.4-5 on FR2476, UCA1/P4.2-3 on FR2355);
    // needed for `console`, harmless otherwise. The SENSOR I²C pins (P1.2/P1.3 → UCB0) are owned by the
    // MODE machine: acquired in Sensing, released in Passive, so a Passive node stays off the bus.
    clock::init_1mhz(p);
    bsp::board::route_console_uart_pins(p);

    let model = model::detect();
    // Role-based Stem I²C address (I2C-API.md): FR2355=0x20, FR2476=0x21. The model→role→addr codec is
    // the shared wire contract in `stembus::role` (bsp::model stays protocol-free), so compute it here.
    let stem_addr = stembus::role::role_for_model(model.device_id()).addr() as u16;

    // Bring up the Stem-bus I²C SLAVE (eUSCI_B1) FIRST — it is the SoM's always-on control channel and
    // must be reachable the instant we boot, regardless of sensor-bus state. (Previously it came up
    // AFTER the Leaf enumerate, so a wedged/slow sensor bus — e.g. two masters contending — delayed or
    // could indefinitely block the SoM's only way in. See DESIGN.md "Boot logic".) `stem::init` only
    // configures eUSCI_B1 + its pins; the RegFile it serves is assembled once status is ready, below.
    stem::init(p, stem_addr);
    let mut slave = stem::Slave::new();

    // Enter the boot-default operating mode. In Sensing this ACQUIRES the sensor bus (B0 master); in
    // Passive it RELEASES it (the SoM owns it). Default is **Passive** — at boot the SoM/host is active
    // and owns the buses; it commands Sensing only when it suspends (mode::Mode::default_boot).
    let mut mach = mode::ModeMachine::boot(p);

    // Sensor scan runs ONLY in Sensing (Passive never masters the sensor bus). It populates the
    // debug registers the SoM reads over the I2C-slave surface. UART reporting below is `console`-gated.
    #[allow(unused_mut)]
    let mut last = if mach.is_sensing() {
        enumerate::scan(p)
    } else {
        enumerate::Scan::absent()
    };

    // OLED boot splash — only when we master the sensor bus (Sensing) and an OLED answered. In Passive
    // the OLED is the SoM's to drive, so we don't touch it.
    if mach.is_sensing() {
        oled_splash(p, &last);
    }

    // Assemble the RegFile the Stem slave serves: live status snapshot (+ PCA9698 shadow). `slave` is
    // the per-transaction state. Polled in the loop below.
    let mut st = if mach.is_sensing() {
        status::Status::from_scan(model, &last)
    } else {
        status::Status::booted(model) // Passive: not enumerated (didn't master the bus)
    };
    st.set_mode(mach.current().code()); // publish the boot mode at MODE_CTRL (0x3E)
    let mut rf = stem::RegFile::new(st);

    // POST integrity (ADR-0010): compute the per-sector FRAM CRCs ONCE, ALWAYS (even the silent image),
    // and store them for the M7 to read over I²C at DBG_CRC (0x3F). The MSP only reports; the M7 judges.
    let crc = post::fram_crcs();
    rf.set_crc(crc);

    // Dev observability: banner + human report + on-demand commands over the backchannel UART.
    #[cfg(feature = "console")]
    {
        uart::init(p);
        // LEAN default (DESIGN.md console log, Henrik 2026-10-04): version + per-sector FRAM checksums.
        post::boot_log(p, model, mach.current().name(), &crc);

        // --debug: the post-POST "what was found" (enumerate report + debug regs + Leaf/eUSCI diagnostic)
        // + the interactive s/r/d/m/l commands. "Not a full diag, but some basic info."
        #[cfg(feature = "debug")]
        {
            uart::puts(p, "commands: s=scan  r=report  d=debug regs  m=switch mode  l=loopback self-test\n");
            enumerate::report(p, &last);
            rf.status.dump(p);
            post::debug_leaf(p);
            // Boot loopback self-test: eUSCI_B0 MASTER → eUSCI_B1 SLAVE (needs Stem↔Leaf jumpers,
            // stem::LOOPBACK_JUMPER_HINT). Only meaningful in Sensing (B0 up as master).
            if mach.is_sensing() {
                let lb = stem::loopback_selftest(p, &mut slave, &mut rf, stem_addr);
                uart::puts(p, "loopback B0->B1 @0x");
                uart::hex8(p, stem_addr as u8);
                uart::puts(p, " (boot self-test): ");
                if !lb.ran {
                    uart::puts(p, "no response (fit jumpers ");
                    uart::puts(p, stem::LOOPBACK_JUMPER_HINT);
                    uart::puts(p, ")\n");
                } else {
                    uart::puts(p, "iface=0x");
                    uart::hex8(p, lb.iface);
                    uart::puts(p, " verMaj=");
                    uart::dec(p, lb.ver_major as u16);
                    uart::puts(p, if lb.passed() { "  PASS\n" } else { "  FAIL\n" });
                }
            }
        }
    }

    // Arm the hang backstop now that the deterministic bring-up — incl. the ~5 s OLED splash and any
    // console boot prints — is done. From here a hang past ~16 s resets the node instead of wedging it.
    bsp::board::wdt_arm_backstop(p);

    // ONE cooperative serve loop for EVERY dual build (ADR-0006): service the Stem slave and advance
    // the Leaf enumerate incrementally each pass so neither a scan nor a mode switch stalls the SoM's
    // control channel. The console/`debug` extras (auto-report + interactive s/r/d/m/l commands) are
    // cfg-gated INSIDE this one loop — previously this was two near-identical loops (console vs silent).
    // Leaf-scan TASK (ADR-0013): ONE async `leaf_scanner` future, pinned once and polled every pass,
    // replaces the hand-written `ScanJob`/`pump_scan` threading. The `Cell` mailbox arms it (on a
    // Sensing switch), cancels it (on leaving Sensing — so a released bus is never driven), and carries
    // its result out for us to publish. It borrows only `p` + these Cells — never `rf` — so it never
    // conflicts with the slave's `&mut rf` (the borrow rule; docs/TASK-STYLE-GUIDE.md).
    let arm = Cell::new(false);
    let cancel = Cell::new(false);
    let out: Cell<Option<enumerate::Scan>> = Cell::new(None);
    let mut scanner = core::pin::pin!(enumerate::leaf_scanner(p, &arm, &cancel, &out));
    let waker = sched::noop_waker();
    let mut acx = core::task::Context::from_waker(&waker);

    loop {
        bsp::board::wdt_pet(p); // reload the backstop; a genuine hang stops petting → reset
        stem::poll(p, &mut slave, &mut rf); // service the SoM/Stem master
        let _ = scanner.as_mut().poll(&mut acx); // advance the Leaf scan (forever task → always Pending)
        // A completed scan deposits its result; publish it into the RegFile — but ONLY while still
        // Sensing (a result that lands after we left Sensing is stale and must not overwrite status).
        if let Some(scan) = out.take() {
            if mach.is_sensing() {
                last = scan;
                rf.status = status::Status::from_scan(model, &last);
                rf.status.set_mode(mach.current().code());
                // --debug: auto-print what the completed Sensing scan found (no keystroke needed).
                #[cfg(feature = "debug")]
                {
                    enumerate::report(p, &last);
                    post::debug_leaf(p);
                }
            }
        }
        // A SoM write to MODE_CTRL (0x3E) queues a mode switch; begin it here (non-blocking).
        if let Some(code) = rf.take_mode_request() {
            match mode::Mode::from_code(code) {
                Some(m) if m != mach.current() => {
                    begin_mode(p, &mut mach, &mut rf, model, &mut last, m);
                    arm_scan(&arm, &cancel, mach.is_sensing()); // arm/cancel the async scanner
                    #[cfg(feature = "debug")]
                    {
                        uart::puts(p, "mode (SoM) -> ");
                        uart::puts(p, mach.current().name());
                        uart::putc(p, b'\n');
                    }
                }
                Some(_) => {} // already in that mode — nothing to do
                None =>
                {
                    #[cfg(feature = "debug")]
                    {
                        uart::puts(p, "mode (SoM): ignoring unsupported code 0x");
                        uart::hex8(p, code);
                        uart::putc(p, b'\n');
                    }
                }
            }
        }
        #[cfg(feature = "debug")]
        if uart::rx_ready(p) {
            match uart::getc(p) {
                b's' | b'S' => {
                    if mach.is_sensing() {
                        // Manual dev scan: synchronous for an explicit keystroke. Cancel the async
                        // scan + drain any pending result so it can't overwrite this fresh one.
                        cancel.set(true);
                        arm.set(false);
                        let _ = out.take();
                        last = enumerate::scan(p);
                        enumerate::report(p, &last);
                        rf.status = status::Status::from_scan(model, &last);
                        rf.status.set_mode(mach.current().code());
                    } else {
                        uart::puts(p, "  (passive: not mastering the sensor bus)\n");
                    }
                }
                b'r' | b'R' => enumerate::report(p, &last),
                b'd' | b'D' => rf.status.dump(p),
                b'm' | b'M' => {
                    let next = mach.current().toggled();
                    begin_mode(p, &mut mach, &mut rf, model, &mut last, next);
                    arm_scan(&arm, &cancel, mach.is_sensing());
                    uart::puts(p, "mode -> ");
                    uart::puts(p, mach.current().name());
                    if mach.is_sensing() {
                        uart::puts(p, "  (Leaf enumerate running async; 'd'/'s' to see it)\n");
                    } else {
                        uart::puts(p, "  sensor bus released (SoM owns it)\n");
                    }
                }
                b'l' | b'L' => {
                    if mach.is_sensing() {
                        let lb = stem::loopback_selftest(p, &mut slave, &mut rf, stem_addr);
                        uart::puts(p, "loopback B0->B1 @0x");
                        uart::hex8(p, stem_addr as u8);
                        uart::puts(p, ": ");
                        if !lb.ran {
                            uart::puts(p, "no response (jumper ");
                            uart::puts(p, stem::LOOPBACK_JUMPER_HINT);
                            uart::puts(p, "?)\n");
                        } else {
                            uart::puts(p, "iface=0x");
                            uart::hex8(p, lb.iface);
                            uart::puts(p, " verMaj=");
                            uart::dec(p, lb.ver_major as u16);
                            uart::puts(p, if lb.passed() { "  PASS\n" } else { "  FAIL\n" });
                        }
                    } else {
                        uart::puts(p, "  (loopback needs Sensing: B0 must master the bus)\n");
                    }
                }
                b'\r' | b'\n' => {}
                _ => uart::puts(p, "  (s=scan r=report d=debug m=mode l=loopback)\n"),
            }
        }
    }
}

/// BEGIN a mode switch — from the bench `m` key OR a SoM write to `MODE_CTRL` (0x3E) — WITHOUT blocking
/// the Stem control channel (ADR-0006). Does only the **fast** part: hand off the sensor bus
/// (`ModeMachine::switch` = pin mux + eUSCI bring-up/reset) and republish the live mode at 0x3E so the
/// SoM's readback is correct immediately. Arming/cancelling the async Leaf enumerate is the caller's
/// job via [`arm_scan`] (the scanner is a long-lived async task polled in the loop, ADR-0013).
/// Centralises what both triggers do so they can't drift.
#[cfg(feature = "_dual")]
fn begin_mode(
    p: &Peripherals,
    mach: &mut mode::ModeMachine,
    rf: &mut stem::RegFile,
    model: model::Model,
    last: &mut enumerate::Scan,
    new: mode::Mode,
) {
    mach.switch(p, new); // sequenced bus acquire/release (no-op if already there) — fast, no scan
    if !mach.is_sensing() {
        *last = enumerate::Scan::absent(); // leaving Sensing: we hold no sensor data
    }
    // dev_count stays at the booted default until the async scan lands (Sensing) / stays absent (Passive).
    rf.status = status::Status::booted(model);
    rf.status.set_mode(mach.current().code());
}

/// Arm or cancel the async Leaf [`enumerate::leaf_scanner`] on a mode transition: entering Sensing
/// arms a fresh scan; leaving Sensing cancels any in-flight scan IMMEDIATELY (so the just-released
/// sensor bus is never driven against its new owner — a two-master fault). The scanner itself is
/// polled every pass in the serve loop; this only flips its mailbox flags.
#[cfg(feature = "_dual")]
fn arm_scan(arm: &Cell<bool>, cancel: &Cell<bool>, sensing: bool) {
    if sensing {
        cancel.set(false);
        arm.set(true);
    } else {
        cancel.set(true);
        arm.set(false);
    }
}

/// Boot splash on a connected OLED: firmware version + boot success for ~5 s, then blank the panel
/// (never leave it showing garbage / an uninitialised white raster). No-op if no OLED answered.
/// Panel-parameterised via `devices::ssd1306` — swap `SSD1306_128X32` for another size as needed.
#[cfg(feature = "_dual")]
fn oled_splash(p: &Peripherals, scan: &enumerate::Scan) {
    use devices::ssd1306::{Oled, SSD1306_128X32};
    let oled = Oled::new(SSD1306_128X32);
    if !scan.present.get(oled.addr()) {
        return; // no OLED connected → nothing to show
    }
    let mut bus = crate::hal::EusciI2c::new(p);
    if oled.init(&mut bus).is_err() {
        return;
    }
    let _ = oled.clear(&mut bus);
    let _ = oled.text(&mut bus, 0, 0, if scan.faulted { "PROD FAULT" } else { "PROD OK" });
    if let Ok(c) = oled.text(&mut bus, 2, 0, "V") {
        let _ = oled.text(&mut bus, 2, c, FW_VERSION);
    }
    // Hold ~5 s (30k nops ≈ 100 ms on the 1 MHz clock, ×50), then turn the display off.
    for _ in 0..50u16 {
        for _ in 0..30_000u16 {
            msp430::asm::nop();
        }
    }
    let _ = oled.off(&mut bus);
}

/// Model name for the banner (no core::fmt on this budget). Used by both node paths under `console`.
#[cfg(feature = "console")]
fn model_name(m: model::Model) -> &'static str {
    stembus::role::model_name(m.device_id()) // shared codec (one source; same labels as the masters)
}

/// FR24xx (FR2433) single-I²C **slave-only** node. Its whole job: answer the SoM's PCA9698-emulation
/// register file on the one eUSCI_B0 (UCB0, P1.2/P1.3) — identity/status/version (real) + the GPIO
/// bank shadow (regmap.rs / stem.rs, the SAME contract the dual node serves on eUSCI_B1). FR2433 is
/// Passive-only (no sensor master → no scan/mode machine), so it publishes a booted, Passive status
/// and serves reads/writes forever. Physical GPIO/ADC backing of the banks is the next step (#3).
#[cfg(feature = "fr24xx")]
fn run_fr24xx(p: &Peripherals) -> ! {
    let model = model::detect();

    // Publish identity + booted/Passive status behind the slave surface (no sensor scan on this part).
    let mut st = status::Status::booted(model);
    st.set_mode(regmap::MODE_CODE_PASSIVE);
    let mut rf = stem::RegFile::new(st);
    // POST integrity (ADR-0010): per-sector FRAM CRCs, always — the M7 reads them over I²C at 0x3F.
    let crc = post::fram_crcs();
    rf.set_crc(crc);
    let mut slave = stem::Slave::new();
    // role-based (I2C-API.md): FR2433 = 0x22 (I/O-expander placeholder). Shared codec in stembus::role.
    let stem_addr = stembus::role::role_for_model(model.device_id()).addr() as u16;
    stem::init(p, stem_addr); // eUSCI_B0 as I²C slave @ stem_addr on P1.2/P1.3

    // Dev console (`console` feature): a locked 1 MHz clock (clean UART baud) + the eUSCI_A0 backchannel
    // on P1.4/P1.5 (distinct from UCB0 on P1.2/P1.3 — no pin clash). Print the banner + debug window
    // ONCE, then fall through to serving the slave. The slave itself needs no clock (the master drives
    // SCL), so a production (console-off) build serves it on the default DCO with no clock init.
    #[cfg(feature = "console")]
    {
        const P1_UART_PINS: u8 = 0x30; // BIT4|BIT5 -> UCA0 TXD/RXD on P1.4/P1.5 (FR2433 backchannel)
        clock::init_1mhz(p);
        p.p1.p1sel1().modify(|r, w| unsafe { w.bits(r.bits() & !P1_UART_PINS) });
        p.p1.p1sel0().modify(|r, w| unsafe { w.bits(r.bits() | P1_UART_PINS) });
        uart::init(p);
        // Lean boot log: version + per-sector FRAM checksums (same as the dual node). fr24xx is
        // Passive-only (no mode machine), so the mode name is the constant "passive".
        post::boot_log(p, model, "passive", &crc);
        uart::puts(p, "fr24xx I2C-slave @0x");
        uart::hex8(p, stem_addr as u8);
        uart::putc(p, b'\n');
        rf.status.dump(p); // the exact 0x30–0x3F bytes a master would read
        // Prove the physical GPIO bank backing end-to-end (no I²C master needed): drive P1.0 via the
        // regmap OP/IOC path and sense it back on IP. Auto-runs so hwd's READ-ONLY console captures
        // PASS/FAIL with no keystroke (matches the dual node's boot self-test discipline).
        let gp = stem::gpio_selftest(p, &mut rf);
        uart::puts(p, "gpio bank0 self-test (P1.0 drive->sense): ");
        uart::puts(p, if gp { "PASS\n" } else { "FAIL\n" });
    }

    // Arm the hang backstop (deterministic bring-up done). Serve the slave surface forever. A SoM write
    // to MODE_CTRL (0x3E) has nowhere to go on a Passive-only part — drain and ignore it so the request
    // can't wedge; reads still return PASSIVE.
    bsp::board::wdt_arm_backstop(p);
    loop {
        bsp::board::wdt_pet(p); // reload the backstop; a hang in the serve loop resets the node
        stem::poll(p, &mut slave, &mut rf);
        let _ = rf.take_mode_request();
    }
}
