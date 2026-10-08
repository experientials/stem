#![no_main]
#![no_std]
#![feature(asm_experimental_arch)] // core::arch::asm! for SCG0 in clock.rs (fr247x); harmless otherwise
#![cfg_attr(any(feature = "fr24xx", feature = "_dual"), feature(abi_msp430_interrupt))] // eUSCI_B slave
// wake ISRs (fr2433 UCB0 / dual UCB1) use `extern "msp430-interrupt"` — experimental ABI.

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
pub(crate) use bsp::hal; // oled_splash's EusciI2c; the Leaf driver `bsp::i2c` is now used directly by
                         // `deepsleep` (enumerate/stem), not re-exported through prod.
#[cfg(feature = "_dual")]
use core::cell::Cell; // async Leaf-scanner mailbox (arm/cancel/result) — see run_dual's serve loop
#[cfg(feature = "_dual")]
use core::future::Future; // for `.poll()` on the pinned scanner future in the serve loop

pub use bsp::model; // Runtime MSP430 model detection + pin mapping — now shared in bsp (STEM-DIRECTION.md
                    // "model-detect + pinmap → bsp"); both prod boot and diag POST read one detector.
pub use deepsleep::regmap; // I²C-slave register-map contract (PCA9698 emulation + Thepia extensions) —
                           // now in the `deepsleep` crate (DEEPSLEEP-EXTRACTION.md Phase 0a). Re-export
                           // keeps `crate::regmap::*` resolving for prod's modules unchanged.

// DUAL-I²C bring-up modules — shared across all dual families (fr247x/fr215x/fr235x, the `_dual`
// marker). They talk to the chip through the `pac` alias, so the SAME code serves every dual PAC; only
// the alias differs. The FR24xx (single-I²C) path is a separate idle scaffold. Portable device drivers
// live in `crates/devices`; only this glue is chip-specific.
// Clock init (locked 1 MHz SMCLK for a clean UART) now lives in `bsp::board::clock_init_1mhz`, per
// family — including an fr2433 module (ADR-0014 / CONV-1). prod no longer carries its own copy.
#[cfg(feature = "_dual")]
pub use deepsleep::enumerate; // Leaf sensor-bus enumeration — now in `deepsleep` (Phase 0a); re-export
                             // keeps `crate::enumerate::*` resolving for main.rs/status.rs unchanged.
// The I²C-slave surface (status + stem transport + regmap) serves BOTH the dual node (eUSCI_B1) and
// the fr24xx Passive node (its single eUSCI_B0) — it's the fr24xx node's whole job. `enumerate`/`mode`
// stay dual-only (no sensor bus / mode machine on the single-I²C part).
#[cfg(any(feature = "_dual", feature = "fr24xx"))]
pub use deepsleep::status; // identity/status model (regmap 0x30–0x3F) — now in deepsleep (Phase 0b)
#[cfg(feature = "_dual")]
pub use deepsleep::mode; // operating-mode machine (Passive↔Sensing) — now in deepsleep (Phase 0c)
#[cfg(any(feature = "_dual", feature = "fr24xx"))]
pub use deepsleep::stem; // I²C-slave dispatch (RegFile/Slave/poll) — now in `deepsleep` (Phase 1a);
                         // its transport (eUSCI programming + pins) is `bsp::stem_i2c`. Re-export keeps
                         // `crate::stem::*` resolving for main.rs unchanged.
#[cfg(all(feature = "fr24xx", feature = "cros_ec"))]
pub use deepsleep::ec; // cros_ec EC-slave glue (COMMANDS.md §0 HELLO milestone) — replaces the regmap
                       // slave on the FR2433 cros_ec build; drives the same LPM pender + wake ISR.
#[cfg(all(feature = "fr24xx", feature = "cros_ec"))]
pub use deepsleep::node; // the Node/Operate adapter the vendor+postcard path dispatches against.
// UART logging is the `console` feature (dev only). Off = silent production image; state then lives
// only in the debug registers (served by the I2C-slave surface). See status.rs / the console question.
// Console/UART is available to ANY family with `console` on (dual + fr24xx). `uart.rs` unifies the
// eUSCI_A0 field-name difference (e_usci_a0 vs usci_a0_uart_mode) via a macro.
#[cfg(feature = "console")]
mod uart;
// Boot POST: `post::fram_crcs` (per-sector FRAM CRC16) runs on EVERY build incl. silent (ADR-0010 —
// the M7 reads the raw CRCs over I²C and judges). The UART echo (`boot_log`/`debug_leaf`) inside is
// `console`/`debug`-gated. So the module is ALWAYS compiled.
pub use deepsleep::post; // boot POST integrity (FRAM CRC) + console boot log — now in deepsleep (Phase 0b)

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
    bsp::board::clock_init_1mhz(p);
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
        status::Status::from_scan(model, &last, FW_BUILD)
    } else {
        status::Status::booted(model, FW_BUILD) // Passive: not enumerated (didn't master the bus)
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
        post::boot_log(p, model, mach.current().name(), &crc, FW_BUILD);

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
                rf.status = status::Status::from_scan(model, &last, FW_BUILD);
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
                        rf.status = status::Status::from_scan(model, &last, FW_BUILD);
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

        // DEEPSLEEP (silent builds): extend the Stem-slave LPM to UCB1. In Passive there is nothing to do
        // until the SoM talks, so sleep in LPM0 on the eUSCI_B1 slave ISR — the proven fr2433 pattern, on
        // UCB1. Sensing keeps busy-polling (the Leaf scanner needs the CPU); console/`debug` builds
        // busy-poll (the interactive menu polls `getc` — a UART-RX wake leaf is the deferred follow-up).
        // Ordering: GIE off BEFORE arming the slave IE, so no ISR fires between the arm and the atomic
        // sleep (which would mask IE then sleep forever). On wake the loop-top `wdt_pet` re-arms the
        // backstop (clears WDTHOLD) and `stem::poll` drains the transaction the ISR woke us for.
        #[cfg(not(feature = "console"))]
        if !mach.is_sensing() {
            msp430::interrupt::disable();
            bsp::stem_i2c::irq_enable(p);
            bsp::board::wdt_hold(p);
            enter_lpm0_gie();
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
    rf.status = status::Status::booted(model, FW_BUILD);
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

/// PROBE (Phase 1b): eUSCI_B0 slave ISR on fr2433 — HAND-ROLLED wake trampoline (alternative to the
/// msp430-rt `wake_cpu` macro, whose `jmp {inner}` truncates under LTO: R_MSP430_10_PCREL, ±1 KB).
/// `br #{inner}` is an ABSOLUTE branch (R_MSP430_16, full range) → placement-independent, LTO-safe, and
/// self-contained (no dependency patch). The naked outer clears SCG1|SCG0|OSCOFF|CPUOFF (0xF0) in the
/// stacked SR (0(r1), r1=SP) so the CPU returns to active mode after the inner handler's `reti`.
#[cfg(feature = "fr24xx")]
#[unsafe(naked)]
#[no_mangle]
unsafe extern "msp430-interrupt" fn USCI_B0() {
    core::arch::naked_asm!(
        "bic.b #0xF0, 0(r1)", // wake: clear the LPM bits in the pushed SR
        "br #{inner}",        // absolute branch to the real handler (full range)
        inner = sym usci_b0_inner,
    );
}
/// Real eUSCI_B0 slave handler body. `extern "msp430-interrupt"` → the compiler emits `reti` at the end,
/// which pops the (wake-modified) SR. Reached via `br` from the trampoline above.
///
/// Does **no byte work** (§3 / docs/references/firmware-architecture.md): it only MASKS the slave IE so
/// the still-latched IFG won't immediately re-fire while the CPU is awake, then returns — the trampoline
/// already cleared `CPUOFF`, so the main loop resumes and drains `next_event()` + re-enables the IE. The
/// register file is never touched here, so it needs no critical section (it stays in the main context).
#[cfg(feature = "fr24xx")]
extern "msp430-interrupt" fn usci_b0_inner() {
    // SAFETY: single-core; this ISR masks its own source and runs only while main is in LPM (or at the
    // atomic sleep instruction), so there is no concurrent RMW of UCB0IE with the main context.
    let p = unsafe { Peripherals::steal() };
    bsp::stem_i2c::irq_disable(&p);
    STEM_PENDER.wake(); // tell the sched executor a wake arrived (closes the wake-during-poll race, 1c)
}

/// The LPM pender for the fr2433 Stem-slave dispatch task (ADR-0013 §5). The ISR `wake()`s it; the
/// `run_pended` executor idles (LPM) only when a poll round made no progress and this flag is clear.
#[cfg(feature = "fr24xx")]
static STEM_PENDER: sched::Pender = sched::Pender::new();

/// Enter LPM0 with interrupts enabled, atomically (SR = r2; CPUOFF|GIE = 0x18). One instruction, so there
/// is no window between "decide to sleep" and "an interrupt can wake me" (Samek's chicken-and-egg;
/// docs/references/firmware-architecture.md §1). The eUSCI_B0 slave ISR wakes us (its `wake_cpu`
/// trampoline clears CPUOFF in the stacked SR); a pending IFG at this point fires immediately rather than
/// sleeping. Default (memory-clobbering) asm → a barrier, so loads/stores aren't reordered across sleep.
#[cfg(any(feature = "fr24xx", feature = "_dual"))]
#[inline(always)]
fn enter_lpm0_gie() {
    unsafe { core::arch::asm!("bis #0x18, r2", options(nostack)) };
}

/// Dual Stem-slave (eUSCI_B1) wake ISR — the SAME hand-rolled `br` trampoline as fr2433's `USCI_B0`,
/// extended to UCB1 (vector `EUSCI_B1`). Clears the LPM bits in the stacked SR so the CPU wakes; the
/// inner masks UCB1's slave IE so the latched IFG won't re-fire before the serve loop drains it. No
/// pender here — the dual serve loop is the 1b-style hand loop (re-polls on wake), not `run_pended`.
#[cfg(feature = "_dual")]
#[unsafe(naked)]
#[no_mangle]
unsafe extern "msp430-interrupt" fn EUSCI_B1() {
    core::arch::naked_asm!(
        "bic.b #0xF0, 0(r1)",
        "br #{inner}",
        inner = sym eusci_b1_inner,
    );
}
#[cfg(feature = "_dual")]
extern "msp430-interrupt" fn eusci_b1_inner() {
    // SAFETY: single-core; masks its own source, runs only while main is in LPM (or at the atomic sleep).
    let p = unsafe { Peripherals::steal() };
    bsp::stem_i2c::irq_disable(&p);
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
    let mut st = status::Status::booted(model, FW_BUILD);
    st.set_mode(regmap::MODE_CODE_PASSIVE);
    let mut rf = stem::RegFile::new(st);
    // POST integrity (ADR-0010): per-sector FRAM CRCs, always — the M7 reads them over I²C at 0x3F.
    let crc = post::fram_crcs();
    rf.set_crc(crc);
    #[cfg(not(feature = "cros_ec"))]
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
        bsp::board::clock_init_1mhz(p);
        bsp::board::route_console_uart_pins(p); // P1.4/P1.5 -> UCA0 (FR2433 backchannel)
        uart::init(p);
        // Lean boot log: version + per-sector FRAM checksums (same as the dual node). fr24xx is
        // Passive-only (no mode machine), so the mode name is the constant "passive".
        post::boot_log(p, model, "passive", &crc, FW_BUILD);
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

    // INTERRUPT-DRIVEN + LPM dispatch as an ASYNC `sched` task (Phase 1c, ADR-0013 §5 LPM pender).
    // FR2433 is Passive-only: its whole job is to answer the Stem master on eUSCI_B0, so it SLEEPS in LPM
    // between transactions and wakes on the slave ISR (slau445i §24.3.10). `stem::serve` is a
    // never-completing future that `.await`s one SlaveEvent at a time; `sched::run_pended` polls it and
    // runs the `idle` closure below whenever a poll round finds the bus idle (no progress + no pending
    // wake) — functionally identical to 1b's hand loop, now expressed in the one async model the dual
    // multi-task reshape will reuse. See DEEPSLEEP-EXTRACTION.md §3 + docs/references/msp430-lpm-wake-pattern.md.
    //
    // WATCHDOG vs LPM (slau445i §12): a sleeping CPU can't pet, and the reset-mode WDT has a clock
    // fail-safe (ACLK loss → VLOCLK) so LPM does NOT stop it. So the `idle` closure HOLDs the backstop
    // across the sleep and re-arms it on wake; the backstop guards only the awake servicing (where a hang
    // can occur). Ordering: disable GIE BEFORE arming the slave IE, so no ISR fires between the arm and
    // the atomic `bis #0x18,r2` (which would otherwise mask IE and then sleep forever).
    bsp::board::wdt_arm_backstop(p); // guard the first poll round before the executor can idle

    // Serve the slave forever, sleeping in LPM between transactions ([`fr24xx_idle`]). The ONE difference
    // is which framing the slave speaks: the cros_ec milestone build (COMMANDS.md §0) runs the cros_ec v3
    // EC-slave framer; the default build runs the PCA9698-regmap slave. Both drive the identical LPM
    // pender + eUSCI_B0 wake ISR, so the low-power path is unchanged.
    #[cfg(not(feature = "cros_ec"))]
    {
        let mut task = core::pin::pin!(stem::serve(p, &mut slave, &mut rf));
        sched::run_pended(task.as_mut(), &STEM_PENDER, || fr24xx_idle(p));
    }
    #[cfg(feature = "cros_ec")]
    {
        // Build the Node adapter from the boot snapshot (identity/status/CRC). Standard commands
        // (HELLO/proto) need no node; the vendor+postcard path (ident/status/…) dispatches against it.
        let ident = rf.status.identity(stem_addr as u8);
        let status = rf.status.status_data(stembus::command::WakeReason::None);
        let mut node = node::StaticNode::new(ident, status, crc);
        let mut ec_slave = stembus::cros_ec::EcSlave::new();
        let mut task = core::pin::pin!(ec::serve_ec(p, &mut ec_slave, &mut node));
        sched::run_pended(task.as_mut(), &STEM_PENDER, || fr24xx_idle(p));
    }
    // `serve*` never completes, so `run_pended` never returns; satisfy the `-> !` signature.
    loop {}
}

/// The fr2433 serve-loop idle: disable GIE, arm the slave IE, HOLD the WDT backstop across LPM, sleep
/// atomically ([`enter_lpm0_gie`]), then re-arm the backstop on wake. Shared verbatim by the regmap and
/// cros_ec serve loops (slau445i §12 WDT-vs-LPM; see DEEPSLEEP-EXTRACTION.md Phase 1b).
#[cfg(feature = "fr24xx")]
#[inline(always)]
fn fr24xx_idle(p: &Peripherals) {
    msp430::interrupt::disable();
    bsp::stem_i2c::irq_enable(p);
    bsp::board::wdt_hold(p);
    enter_lpm0_gie(); // wakes on the eUSCI_B0 slave ISR (masks IE + STEM_PENDER.wake())
    bsp::board::wdt_arm_backstop(p); // awake again — re-guard the next poll round
}
