#![no_main]
#![no_std]
#![feature(asm_experimental_arch)] // core::arch::asm! (enter_lpm0_gie) + naked_asm! on msp430
#![feature(abi_msp430_interrupt)] // eUSCI_B0 slave wake ISR uses `extern "msp430-interrupt"`

//! bob-929 I/O EXPANDER firmware (Rust) — **FR2433 single-I²C slave-only Stembus node**.
//!
//! Extracted from `prod/` in the STEM-DIRECTION Repo-layout reorg (S2a): prod/ is now the DUAL-I²C
//! family axis (FR2476/FR2355/FR2155); this is the standalone FR2433 I/O expander. One chip, bound by
//! crate composition — no family if-ladder.
//!
//! **cros_ec ONLY.** This image speaks the ChromeOS EC host-command protocol (cros_ec v3) on its one
//! eUSCI_B0 — the default command interface (COMMANDS.md §0). The legacy PCA9698 register-map slave is
//! dropped by omission: none of that register-file path is built here, and `deepsleep`'s cros_ec path is
//! compiled unconditionally (enabled on the dependency, not a crate feature — there is no feature gate).
//!
//! FR2433 is Passive-only (no sensor bus → no scan/mode machine): it publishes a booted, Passive
//! identity/status snapshot behind the cros_ec EC-slave surface and serves host commands forever, sleeping
//! in LPM between transactions and waking on the eUSCI_B0 slave ISR.

extern crate panic_msp430; // infinitely-looping panic handler (a hang trips the watchdog → reset)

use msp430_rt::entry;

// Exactly one chip — guard against an accidental `--no-default-features` with nothing selected.
#[cfg(not(feature = "fr2433"))]
compile_error!("expander: enable the `fr2433` family feature (it selects the PAC + memory map)");
// A mode is required (compile-in ≥1). FR2433 is Passive-only — Sensing needs a 2nd I²C.
#[cfg(not(feature = "mode-passive"))]
compile_error!("expander: enable `mode-passive` (FR2433 is Passive-only — no Sensing on a single-I²C part)");

// The chip-select PAC alias + the shared board/hal/i2c layer live in `bsp`. Re-export `pac` so uart.rs
// keeps `use crate::pac::Peripherals`; the `fr2433` feature forwards to exactly one bsp PAC.
pub(crate) use bsp::pac;
use crate::pac::Peripherals;

use bsp::model; // Runtime MSP430 model detection + pin mapping (shared in bsp).

// The deepsleep capability modules this node uses (all cros_ec, PAC-free — driven through node-hal seams):
//   status — identity/status snapshot    post   — boot POST (FRAM CRC) + console boot log
//   stem   — StemSlave transport init    ec     — cros_ec v3 EC-slave glue    node — StaticNode adapter
use stem_node::{ec, node, post, status, stem};

// UART logging is the `console` feature (dev only). Off = silent production image. `uart.rs` delegates to
// `bsp::board::console_uart_*` (FR2433 backchannel on UCA0/P1.4-5).
#[cfg(feature = "console")]
mod uart;

/// Compiled-in identity stamp (see build.rs). Printed at boot via the console build.
const FW_BUILD: &str = env!("PROD_BUILD");

// PM5CTL0.LOCKLPM5 — set out of reset on FRAM parts; must be cleared to make GPIO live.
const LOCKLPM5: u16 = 0x0001;

#[entry]
fn main() -> ! {
    // steal() (not take()) to match diag/prod and avoid the critical-section machinery.
    let p = unsafe { Peripherals::steal() };

    // HOLD the watchdog during deterministic bring-up; the serve loop switches it to the ~16 s backstop.
    bsp::board::wdt_hold(&p);

    // Unlock the I/O from the LPM5 default so port config takes effect.
    p.pmm.pm5ctl0().modify(|r, w| unsafe { w.bits(r.bits() & !LOCKLPM5) });

    let _ = FW_BUILD;

    run(&p)
}

/// FR2433 single-I²C **slave-only** I/O-expander node (cros_ec). Its whole job: answer the SoM/host on
/// the one eUSCI_B0 (UCB0, P1.2/P1.3) over the cros_ec v3 EC-slave protocol — identity/status/version +
/// per-sector FRAM CRC. Passive-only, so it publishes a booted/Passive snapshot and serves host commands
/// forever, sleeping in LPM between transactions ([`expander_idle`]).
fn run(p: &Peripherals) -> ! {
    let model = model::detect();
    // Host-clean chip identity for the de-welded `deepsleep::{post,status}`, built once from the detected
    // model; `model` stays for the role/addr codec below.
    let chip = node_hal::ChipId { device_id: model.device_id(), family_ok: model.matches_build_family() };

    // Publish identity + booted/Passive status behind the slave surface (no sensor scan on this part).
    let mut st = status::Status::booted(chip, FW_BUILD);
    st.set_mode(stembus::mode::Mode::Passive.code()); // Passive-only expander — report it at MODE_CTRL
    // POST integrity (ADR-0010): per-sector FRAM CRCs, always — the host reads them over the EC surface.
    let crc = post::fram_crcs();
    // role-based (I2C-API.md): FR2433 = 0x22 (I/O-expander). Shared codec in stembus::role.
    let stem_addr = stembus::role::role_for_model(model.device_id()).addr() as u16;
    // MSP430 hardware handle for the de-welded `deepsleep::stem`/`ec` seams: the dispatch is generic over
    // node-hal traits, so wrap the PAC `Peripherals` once and pass `&hw`. The LPM/ISR path
    // (`expander_idle` → `bsp::stem_i2c::irq_enable(p)`) still uses `p` directly (bin-owned).
    let hw = bsp::StemHw(p);
    stem::init(&hw, stem_addr); // eUSCI_B0 as I²C slave @ stem_addr on P1.2/P1.3

    // Dev console (`console` feature): a locked 1 MHz clock (clean UART baud) + the eUSCI_A0 backchannel
    // on P1.4/P1.5 (distinct from UCB0 on P1.2/P1.3 — no pin clash). Print the banner + debug window
    // ONCE, then fall through to serving. The slave itself needs no clock (the master drives SCL), so a
    // production (console-off) build serves it on the default DCO with no clock init.
    #[cfg(feature = "console")]
    {
        bsp::board::clock_init_1mhz(p);
        bsp::board::route_console_uart_pins(p); // P1.4/P1.5 -> UCA0 (FR2433 backchannel)
        uart::init(p);
        // Lean boot log: version + per-sector FRAM checksums. Passive-only → the constant "passive".
        post::boot_log(&hw, chip, "passive", &crc, FW_BUILD);
        uart::puts(p, "expander (cros_ec) I2C-slave @0x");
        uart::hex8(p, stem_addr as u8);
        uart::putc(p, b'\n');
        st.dump(&hw); // the exact 0x30–0x3F bytes a master would read
    }

    // INTERRUPT-DRIVEN + LPM dispatch as an ASYNC `sched` task (ADR-0013 §5 LPM pender). FR2433 is
    // Passive-only: its whole job is to answer the host on eUSCI_B0, so it SLEEPS in LPM between
    // transactions and wakes on the slave ISR. `ec::serve_ec` is a never-completing future that `.await`s
    // one SlaveEvent at a time; `sched::run_pended` polls it and runs `expander_idle` whenever a poll round
    // finds the bus idle (no progress + no pending wake).
    //
    // WATCHDOG vs LPM (slau445i §12): a sleeping CPU can't pet, and the reset-mode WDT has a clock
    // fail-safe (ACLK loss → VLOCLK) so LPM does NOT stop it. So `expander_idle` HOLDs the backstop across
    // the sleep and re-arms it on wake; the backstop guards only the awake servicing.
    bsp::board::wdt_arm_backstop(p); // guard the first poll round before the executor can idle

    // cros_ec v3 EC-slave. Build the Node adapter from the boot snapshot (identity/status/CRC). Standard
    // commands (HELLO/proto) need no node; the vendor+postcard path (ident/status/…) dispatches against it.
    let ident = st.identity(stem_addr as u8);
    let status = st.status_data(stembus::command::WakeReason::None);
    let mut node = node::StaticNode::new(ident, status, crc);
    let mut ec_slave = stembus::cros_ec::EcSlave::new();
    let mut task = core::pin::pin!(ec::serve_ec(&hw, &mut ec_slave, &mut node));
    sched::run_pended(task.as_mut(), &STEM_PENDER, || expander_idle(p));
    // `serve_ec` never completes, so `run_pended` never returns; satisfy the `-> !` signature.
    loop {}
}

/// eUSCI_B0 slave ISR — HAND-ROLLED wake trampoline (alternative to the msp430-rt `wake_cpu` macro, whose
/// `jmp {inner}` truncates under LTO: R_MSP430_10_PCREL, ±1 KB). `br #{inner}` is an ABSOLUTE branch
/// (R_MSP430_16, full range) → placement-independent, LTO-safe, self-contained. The naked outer clears
/// SCG1|SCG0|OSCOFF|CPUOFF (0xF0) in the stacked SR (0(r1), r1=SP) so the CPU returns to active mode.
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
/// which pops the (wake-modified) SR. Reached via `br` from the trampoline.
///
/// Does **no byte work**: it only MASKS the slave IE so the still-latched IFG won't immediately re-fire
/// while the CPU is awake, then returns — the trampoline already cleared `CPUOFF`, so the main loop resumes
/// and drains `next_event()` + re-enables the IE.
extern "msp430-interrupt" fn usci_b0_inner() {
    // SAFETY: single-core; this ISR masks its own source and runs only while main is in LPM (or at the
    // atomic sleep instruction), so there is no concurrent RMW of UCB0IE with the main context.
    let p = unsafe { Peripherals::steal() };
    bsp::stem_i2c::irq_disable(&p);
    STEM_PENDER.wake(); // tell the sched executor a wake arrived (closes the wake-during-poll race)
}

/// The LPM pender for the Stem-slave dispatch task (ADR-0013 §5). The ISR `wake()`s it; the `run_pended`
/// executor idles (LPM) only when a poll round made no progress and this flag is clear.
static STEM_PENDER: sched::Pender = sched::Pender::new();

/// Enter LPM0 with interrupts enabled, atomically (SR = r2; CPUOFF|GIE = 0x18). One instruction, so there
/// is no window between "decide to sleep" and "an interrupt can wake me" (Samek's chicken-and-egg). The
/// eUSCI_B0 slave ISR wakes us (its trampoline clears CPUOFF in the stacked SR); a pending IFG at this
/// point fires immediately rather than sleeping. Default (memory-clobbering) asm → a barrier.
#[inline(always)]
fn enter_lpm0_gie() {
    unsafe { core::arch::asm!("bis #0x18, r2", options(nostack)) };
}

/// The serve-loop idle: disable GIE, arm the slave IE, HOLD the WDT backstop across LPM, sleep atomically
/// ([`enter_lpm0_gie`]), then re-arm the backstop on wake (slau445i §12 WDT-vs-LPM). Ordering: GIE off
/// BEFORE arming the slave IE, so no ISR fires between the arm and the atomic sleep.
#[inline(always)]
fn expander_idle(p: &Peripherals) {
    msp430::interrupt::disable();
    bsp::stem_i2c::irq_enable(p);
    bsp::board::wdt_hold(p);
    enter_lpm0_gie(); // wakes on the eUSCI_B0 slave ISR (masks IE + STEM_PENDER.wake())
    bsp::board::wdt_arm_backstop(p); // awake again — re-guard the next poll round
}
