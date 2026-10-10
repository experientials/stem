#![no_main]
#![no_std]
#![feature(asm_experimental_arch)] // core::arch::asm! for SCG0 in clock.rs (fr247x); harmless otherwise
#![cfg_attr(feature = "_dual", feature(abi_msp430_interrupt))] // eUSCI_B1 slave
// wake ISR (dual UCB1) uses `extern "msp430-interrupt"` — experimental ABI.

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

// Family axis: exactly one DUAL-I²C family feature selects the PAC (and, via build.rs, the memory map).
// (The single-I²C FR2433 node is a separate binary now — `modules/expander/`, S2a.)
#[cfg(not(feature = "_dual"))]
compile_error!("prod: enable one dual-I²C family — fr247x (default) | fr215x | fr235x");

// OPERATING MODES (DESIGN.md): compile in ≥1 mode; sensing needs a 2nd I²C (always available on dual).
#[cfg(not(any(feature = "mode-passive", feature = "mode-sensing")))]
compile_error!("prod: enable at least one mode — `--features mode-passive` and/or `mode-sensing`");

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
// (The `regmap` re-export was removed with the regmap drop — cros_ec is the sole command interface. The
// shared wire constants `mode.rs` needs live in `stembus::regmap`, imported there directly.)

// DUAL-I²C bring-up modules — shared across all dual families (fr247x/fr215x/fr235x, the `_dual`
// marker). They talk to the chip through the `pac` alias, so the SAME code serves every dual PAC; only
// the alias differs. The FR24xx (single-I²C) path is a separate idle scaffold. Portable device drivers
// live in `crates/devices`; only this glue is chip-specific.
// Clock init (locked 1 MHz SMCLK for a clean UART) now lives in `bsp::board::clock_init_1mhz`, per
// family — including an fr2433 module (ADR-0014 / CONV-1). prod no longer carries its own copy.
#[cfg(feature = "_dual")]
pub use leaf_scan::enumerate; // Leaf sensor-bus enumeration — now in `deepsleep` (Phase 0a); re-export
                             // keeps `crate::enumerate::*` resolving for main.rs/status.rs unchanged.
// The I²C-slave surface (status + stem transport + regmap) serves the dual node (eUSCI_B1).
// `enumerate`/`mode` are also dual (sensor bus + mode machine).
#[cfg(feature = "_dual")]
pub use stem_node::status; // identity/status model (regmap 0x30–0x3F) — now in deepsleep (Phase 0b)
#[cfg(feature = "_dual")]
pub mod mode; // operating-mode machine (Passive↔Sensing) — now in deepsleep (Phase 0c)
#[cfg(feature = "_dual")]
pub use stem_node::stem; // Stem-slave TRANSPORT bring-up (`stem::init`); eUSCI programming + pins is
                         // `bsp::stem_i2c`. (The regmap RegFile/Slave/poll it used to carry is gone.)
pub use stem_node::ec; // cros_ec EC-slave glue (COMMANDS.md §0) — the node's command interface.
pub use stem_node::node; // the Node/Operate adapter the cros_ec vendor+postcard path dispatches against
                         // (LiveNode on dual).
// UART logging is the `console` feature (dev only). Off = silent production image; state then lives
// only in the debug registers (served by the I2C-slave surface). See status.rs / the console question.
// Console/UART is available to any dual family with `console` on. `uart.rs` unifies the
// eUSCI_A0 field-name difference (e_usci_a0 vs usci_a0_uart_mode) via a macro.
#[cfg(feature = "console")]
mod uart;
// diag extension — the `--diag` build's extended-POST boot window (DESIGN.md "diag"). Dual-only + needs
// the cros_ec `ec`/`node` glue + console; compiled ONLY in a `--diag` build.
#[cfg(feature = "diag")]
mod diag_ext;
// Boot POST: `post::fram_crcs` (per-sector FRAM CRC16) runs on EVERY build incl. silent (ADR-0010 —
// the M7 reads the raw CRCs over I²C and judges). The UART echo (`boot_log`/`debug_leaf`) inside is
// `console`/`debug`-gated. So the module is ALWAYS compiled.
pub use stem_node::post; // boot POST integrity (FRAM CRC) + console boot log — now in deepsleep (Phase 0b)

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

    run_dual(&p)
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
    // Host-clean chip identity for the de-welded `deepsleep::{post,status}` (Phase 6c): built ONCE from
    // the detected model so those files name no `bsp::model::Model`. `model` itself stays for the role/addr
    // codec below (which is wire-contract, not deepsleep).
    let chip = node_hal::ChipId { device_id: model.device_id(), family_ok: model.matches_build_family() };
    // Role-based Stem I²C address (I2C-API.md): FR2355=0x20, FR2476=0x21. The model→role→addr codec is
    // the shared wire contract in `stembus::role` (bsp::model stays protocol-free), so compute it here.
    let stem_addr = stembus::role::role_for_model(model.device_id()).addr() as u16;

    // Bring up the Stem-bus I²C SLAVE (eUSCI_B1) FIRST — it is the SoM's always-on control channel and
    // must be reachable the instant we boot, regardless of sensor-bus state. (Previously it came up
    // AFTER the Leaf enumerate, so a wedged/slow sensor bus — e.g. two masters contending — delayed or
    // could indefinitely block the SoM's only way in. See DESIGN.md "Boot logic".) `stem::init` only
    // configures eUSCI_B1 + its pins; the live `status::Status` the cros_ec node serves is built below.
    // MSP430 hardware handle for the de-welded `deepsleep::stem` seams (Phase 6a): the dispatch is generic
    // over `node-hal` traits, so we wrap the PAC `Peripherals` once and pass `&hw` to every `stem::*` call.
    // The direct `bsp::stem_i2c::irq_*`/LPM calls below still use `p` (bin-owned, stays as-is).
    let hw = bsp::StemHw(p);
    stem::init(&hw, stem_addr); // configures eUSCI_B1 + pins — the cros_ec slave transport

    // Enter the boot-default operating mode. In Sensing this ACQUIRES the sensor bus (B0 master); in
    // Passive it RELEASES it (the SoM owns it). Default is **Passive** — at boot the SoM/host is active
    // and owns the buses; it commands Sensing only when it suspends (mode::Mode::default_boot).
    let mut mach = mode::ModeMachine::boot(&hw);

    // Sensor scan runs ONLY in Sensing (Passive never masters the sensor bus). It populates the
    // debug registers the SoM reads over the I2C-slave surface. UART reporting below is `console`-gated.
    #[allow(unused_mut)]
    let mut last = if mach.is_sensing() {
        enumerate::scan(&hw)
    } else {
        enumerate::Scan::absent()
    };

    // OLED boot splash — only when we master the sensor bus (Sensing) and an OLED answered. In Passive
    // the OLED is the SoM's to drive, so we don't touch it.
    if mach.is_sensing() {
        oled_splash(p, &last);
    }

    // The live status snapshot the cros_ec node serves. (Was wrapped in a `stem::RegFile` for the regmap
    // slave; with cros_ec the sole surface, the plain `Status` is the state — `LiveNode` reads it.)
    let mut st = if mach.is_sensing() {
        status::Status::from_scan(chip, &last, FW_BUILD)
    } else {
        status::Status::booted(chip, FW_BUILD) // Passive: not enumerated (didn't master the bus)
    };
    st.set_mode(mach.current().code()); // publish the boot mode

    // POST integrity (ADR-0010): compute the per-sector FRAM CRCs ONCE, ALWAYS (even the silent image),
    // and store them for the M7 to read over I²C (cros_ec `FramCrc`). The MSP only reports; the M7 judges.
    let crc = post::fram_crcs();

    // cros_ec EC-slave (COMMANDS.md §0): the vendor+postcard command surface on eUSCI_B1. `LiveNode` is
    // refreshed from `st` each loop pass and sinks `SetMode` requests for the loop to apply via the machine.
    let mut ec_slave = stembus::cros_ec::EcSlave::new();
    let mut live = node::LiveNode::new(
        st.identity(stem_addr as u8),
        st.status_data(stembus::command::WakeReason::None),
        crc,
    );

    // Dev observability: banner + human report + on-demand commands over the backchannel UART.
    #[cfg(feature = "console")]
    {
        uart::init(p);
        // LEAN default (DESIGN.md console log, Henrik 2026-10-04): version + per-sector FRAM checksums.
        post::boot_log(&hw, chip, mach.current().name(), &crc, FW_BUILD);

        // --debug: the post-POST "what was found" (enumerate report + debug regs + Leaf/eUSCI diagnostic)
        // + the interactive s/r/d/m/l commands. "Not a full diag, but some basic info."
        #[cfg(feature = "debug")]
        {
            uart::puts(p, "commands: s=scan  r=report  d=debug regs  m=switch mode\n");
            enumerate::report(&hw, &last);
            st.dump(&hw);
            post::debug_leaf(&hw);
            // (The boot loopback self-test was removed with the regmap drop — it exercised the regmap
            // RegFile slave via a B0→B1 jumper; the cros_ec surface is proven over the real Stem bus.)
        }
    }

    // diag EXTENSION (--diag build only): the extended-POST boot window. Runs the per-device diag tasks
    // UNLESS a master sends `SkipDiag` within the window — then returns here and proceeds to normal
    // operation (diag is ADDITIVE; DESIGN.md "diag — per-device POST extension"). WDT is still held here
    // (armed below), so the multi-second window is safe.
    #[cfg(feature = "diag")]
    diag_ext::run(&hw, &mut ec_slave, &mut live, p, &mut mach);

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
    // its result out for us to publish. It borrows only `p` + these Cells — never `st` — so it never
    // conflicts with the serve loop's `&mut st` (the borrow rule; docs/TASK-STYLE-GUIDE.md).
    let arm = Cell::new(false);
    let cancel = Cell::new(false);
    let out: Cell<Option<enumerate::Scan>> = Cell::new(None);
    let mut scanner = core::pin::pin!(enumerate::leaf_scanner(&hw, &arm, &cancel, &out));
    let waker = sched::noop_waker();
    let mut acx = core::task::Context::from_waker(&waker);

    loop {
        bsp::board::wdt_pet(p); // reload the backstop; a genuine hang stops petting → reset
        // Service the SoM/Stem master: the cros_ec EcSlave framer + vendor+postcard dispatch against the
        // live node (refreshed from `st` this pass).
        live.set_status(st.status_data(stembus::command::WakeReason::None));
        ec::poll(&hw, &mut ec_slave, &mut live);
        let _ = scanner.as_mut().poll(&mut acx); // advance the Leaf scan (forever task → always Pending)
        // A completed scan deposits its result; publish it into the live status — but ONLY while still
        // Sensing (a result that lands after we left Sensing is stale and must not overwrite status).
        if let Some(scan) = out.take() {
            if mach.is_sensing() {
                last = scan;
                st = status::Status::from_scan(chip, &last, FW_BUILD);
                st.set_mode(mach.current().code());
                // --debug: auto-print what the completed Sensing scan found (no keystroke needed).
                #[cfg(feature = "debug")]
                {
                    enumerate::report(&hw, &last);
                    post::debug_leaf(&hw);
                }
            }
        }
        // A SoM mode-switch request — a cros_ec `SetMode` recorded by the live node — queues a switch;
        // begin it here (non-blocking).
        let mode_req = live.take_mode_request();
        if let Some(code) = mode_req {
            match mode::Mode::from_code(code) {
                Some(m) if m != mach.current() => {
                    begin_mode(&hw, &mut mach, &mut st, chip, &mut last, m);
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
                        last = enumerate::scan(&hw);
                        enumerate::report(&hw, &last);
                        st = status::Status::from_scan(chip, &last, FW_BUILD);
                        st.set_mode(mach.current().code());
                    } else {
                        uart::puts(p, "  (passive: not mastering the sensor bus)\n");
                    }
                }
                b'r' | b'R' => enumerate::report(&hw, &last),
                b'd' | b'D' => st.dump(&hw),
                b'm' | b'M' => {
                    let next = mach.current().toggled();
                    begin_mode(&hw, &mut mach, &mut st, chip, &mut last, next);
                    arm_scan(&arm, &cancel, mach.is_sensing());
                    uart::puts(p, "mode -> ");
                    uart::puts(p, mach.current().name());
                    if mach.is_sensing() {
                        uart::puts(p, "  (Leaf enumerate running async; 'd'/'s' to see it)\n");
                    } else {
                        uart::puts(p, "  sensor bus released (SoM owns it)\n");
                    }
                }
                b'\r' | b'\n' => {}
                _ => uart::puts(p, "  (s=scan r=report d=debug m=mode)\n"),
            }
        }

        // DEEPSLEEP (silent builds): extend the Stem-slave LPM to UCB1. In Passive there is nothing to do
        // until the SoM talks, so sleep in LPM0 on the eUSCI_B1 slave ISR — the proven fr2433 pattern, on
        // UCB1. Sensing keeps busy-polling (the Leaf scanner needs the CPU); console/`debug` builds
        // busy-poll (the interactive menu polls `getc` — a UART-RX wake leaf is the deferred follow-up).
        // Ordering: GIE off BEFORE arming the slave IE, so no ISR fires between the arm and the atomic
        // sleep (which would mask IE then sleep forever). On wake the loop-top `wdt_pet` re-arms the
        // backstop (clears WDTHOLD) and `ec::poll` drains the transaction the ISR woke us for.
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
    hw: &bsp::StemHw<'_>,
    mach: &mut mode::ModeMachine,
    st: &mut status::Status,
    chip: node_hal::ChipId,
    last: &mut enumerate::Scan,
    new: mode::Mode,
) {
    mach.switch(hw, new); // sequenced bus acquire/release (no-op if already there) — fast, no scan
    if !mach.is_sensing() {
        *last = enumerate::Scan::absent(); // leaving Sensing: we hold no sensor data
    }
    // dev_count stays at the booted default until the async scan lands (Sensing) / stays absent (Passive).
    *st = status::Status::booted(chip, FW_BUILD);
    st.set_mode(mach.current().code());
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

/// Enter LPM0 with interrupts enabled, atomically (SR = r2; CPUOFF|GIE = 0x18). One instruction, so there
/// is no window between "decide to sleep" and "an interrupt can wake me" (Samek's chicken-and-egg;
/// docs/references/firmware-architecture.md §1). The eUSCI_B1 slave ISR wakes us (its `wake_cpu`
/// trampoline clears CPUOFF in the stacked SR); a pending IFG at this point fires immediately rather than
/// sleeping. Default (memory-clobbering) asm → a barrier, so loads/stores aren't reordered across sleep.
#[cfg(feature = "_dual")]
#[inline(always)]
fn enter_lpm0_gie() {
    unsafe { core::arch::asm!("bis #0x18, r2", options(nostack)) };
}

/// Dual Stem-slave (eUSCI_B1) wake ISR — a hand-rolled `br` trampoline on UCB1 (vector `EUSCI_B1`;
/// the same technique the expander uses on UCB0). Clears the LPM bits in the stacked SR so the CPU wakes; the
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
