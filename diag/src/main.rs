#![no_main]
#![no_std]

//! bob-929 diagnostic firmware (Rust) — MSP430FR2476.
//!
//! Power-on self-test in the spirit of an Amiga diag ROM: scan the I2C bus and
//! exercise each known device, reporting over the eZ-FET backchannel UART and,
//! when present, on the IS31FL3730 LED matrix. Register access goes through the
//! vendored `msp430fr2476` PAC (typed peripherals) using proven bit values from
//! the FR2476 header — not raw pointers.

extern crate panic_msp430; // infinitely-looping panic handler

use msp430_rt::entry;
use crate::pac::Peripherals;

// The chip-select PAC alias + the low-level board/hal/i2c layer live in the shared `bsp` crate now
// (STEM-DIRECTION.md: board/hal must be shared, not inside diag). Re-export them so every diag module
// keeps using `crate::{pac,board,hal,i2c}` unchanged. The chip feature is forwarded to bsp.
pub use bsp::{board, hal, i2c, model, pac};

mod adc;
mod buttons;
mod clock;
mod diag;
mod is31;
mod rcwl;
mod regs;
mod signature;
mod ssd1306_raw;
mod stress;
mod tasks;
mod uart;
mod usec;

// WDT control (hold/arm/pet + the ~16 s reset-mode backstop policy) now lives in the `board` role
// (`board::wdt_hold`/`wdt_arm_backstop`/`wdt_pet`) — one definition shared with prod, field-name delta
// owned there. Backstop is far longer than a worst-case POST cycle (3 s delay + a few s of I2C), so
// normal running never trips it; a real hang (or a panic-msp430 loop) does → the board resets + retries.
const LOCKLPM5: u16 = 0x0001;

// The eUSCI bus pins (Leaf-I2C P1.2/3, console-UART) are routed via the `board` module now.
const P2_SDB: u8 = 0x20; // P2.5 -> IS31FL3730 SDB (drive high to enable). NOT P2.0 — crystal XOUT.
const P2_RCWL_OUT: u8 = 0x10; // P2.4 <- RCWL-0516 radar OUT (GPIO input, port-interrupt capable).

// Boot banner (ASCII "DIAG") — a hard visual break so every reset is obvious in a scrollback.
// Note: `\<newline>` string continuation eats leading whitespace, so no art row may start with a
// space — the letters are laid out to begin on a non-space column.
const BANNER: &str = "\n\
========================================\n\
###    ###    ##    ###\n\
#  #    #    #  #   #   \n\
#  #    #    ####   # ##\n\
#  #    #    #  #   #  #\n\
###    ###   #  #    ###\n\
========================================\n";

/// Pet the watchdog backstop — every scheduler pass, and inside the stress runner's long loops so a
/// multi-second soak doesn't trip the ~16 s reset. Thin delegate to the `board` WDT role (one shared
/// definition); kept so existing `crate::pet_wdt` callers (stress.rs / signature.rs) are unchanged.
fn pet_wdt(p: &Peripherals) {
    board::wdt_pet(p);
}

#[entry]
fn main() -> ! {
    // Single-threaded, no ISRs -> steal is fine and avoids the critical-section feature.
    let p = unsafe { Peripherals::steal() };

    board::wdt_hold(&p); // stop watchdog during deterministic bring-up
    board::clock_init_1mhz(&p);

    // Bus pins, by semantic role, from the board: Leaf-I²C (sensors, P1.2/3 → UCB0) and the
    // console UART backchannel (chip/board-specific instance + pins).
    board::route_leaf_i2c_pins(&p);
    board::route_console_uart_pins(&p);
    p.pmm.pm5ctl0().modify(|r, w| unsafe { w.bits(r.bits() & !LOCKLPM5) });

    // Enable the IS31FL3730 (SDB high on P2.5). Set output high before direction.
    p.p2.p2sel0().modify(|r, w| unsafe { w.bits(r.bits() & !P2_SDB) }); // ensure GPIO
    p.p2.p2out().modify(|r, w| unsafe { w.bits(r.bits() | P2_SDB) });
    p.p2.p2dir().modify(|r, w| unsafe { w.bits(r.bits() | P2_SDB) });

    // RCWL-0516 radar OUT on P2.4: GPIO input with a pulldown, so an unplugged/idle sensor reads
    // LOW instead of floating. OUT is push-pull (drives HIGH ~3.3 V on motion), so the pulldown
    // only loads it (~µA) while asserted — it doesn't fight the driver. Read in `rcwl`; P2.4 is
    // port-interrupt capable if we later wake on motion (not enabled here — diag just polls).
    p.p2.p2sel1().modify(|r, w| unsafe { w.bits(r.bits() & !P2_RCWL_OUT) }); // SEL=00 -> GPIO
    p.p2.p2sel0().modify(|r, w| unsafe { w.bits(r.bits() & !P2_RCWL_OUT) });
    p.p2.p2dir().modify(|r, w| unsafe { w.bits(r.bits() & !P2_RCWL_OUT) }); // input
    p.p2.p2out().modify(|r, w| unsafe { w.bits(r.bits() & !P2_RCWL_OUT) }); // OUT=0 selects pulldown
    p.p2.p2ren().modify(|r, w| unsafe { w.bits(r.bits() | P2_RCWL_OUT) }); // enable the resistor

    uart::init(&p);
    i2c::init(&p);
    adc::init(&p); // internal 1.5-V ref + ADC core, for measured Vcc/temperature in the board stats
    buttons::init(&p); // S1/S2 for the OLED menu (GPIO in + pull-up)

    // Reset delineation: a chunky ASCII banner so each boot/reset is unmistakable when scrolling
    // back through a long `just monitor` capture (retro diag-ROM spirit). Build stamp rides along
    // so the reset boundary also tells you which firmware just came up.
    uart::puts(&p, BANNER);
    uart::puts(&p, "  build ");
    uart::puts(&p, env!("DIAG_BUILD"));
    uart::putc(&p, b'\n');
    uart::puts(&p, "keys (or S1): p=POST  m=I2C-SPEED sweep  s=I2C-SCAN soak  g=I2C-SIG patterns\n");
    // Basic board stats read from the silicon (chip id, revisions, die serial) — not a hardcoded
    // model. Then the live SFR dump (reset cause, clock lock, pin mux) we reason from all run.
    regs::stats(&p);
    regs::dump(&p);

    // Millisecond time base for the cooperative scheduler (TB0 off ACLK, polled — no ISR).
    let mut clock = clock::Clock::start(&p);

    // Arm the watchdog backstop now that init is done (init itself is deterministic and runs
    // unguarded). Every scheduler pass pets it; anything that wedges past ~16 s resets the board —
    // which is exactly the backstop cooperative scheduling needs, since one task that ignores the
    // never-block contract has no other way to be preempted.
    board::wdt_arm_backstop(&p);

    // µs time base (TA0 off SMCLK) — the stress task times each I²C transaction with it.
    usec::start(&p);

    // One unified firmware: the POST, the sensor tasks, and the (formerly `--features stress`) bus
    // tests all run as cooperative tasks, switched at runtime via the button/OLED menu (see
    // tasks::Mode). `cx` carries the shared blackboard; each task owns its private state. Boots into
    // Mode::Post, so behaviour is the live self-test until S1 opens the menu.
    {
        let mut cx = tasks::Cx::new(&p);
        let mut radar = tasks::RadarTask::new();
        let mut prox = tasks::ProximityTask::new();
        let mut post = tasks::PostTask::new();
        let mut btn = tasks::ButtonTask::new();
        let mut ui = tasks::UiTask::new();
        let mut stress = stress::StressTask::new();
        let mut sig = signature::SignatureTask::new();
        let mut slots = [
            sched::Slot::every(50, &mut radar), // self-adjusts to 5 ms while motion is asserted
            sched::Slot::every(200, &mut prox), // APDS-9960 proximity, 5 Hz (Post mode only)
            sched::Slot::every(3000, &mut post), // POST scan/verdict (Post mode only)
            sched::Slot::every(25, &mut btn),   // button scan / menu navigation
            sched::Slot::every(200, &mut ui),   // OLED menu + stress status (raw driver)
            sched::Slot::every(5, &mut stress), // bounded stress batches; self-idles when inactive
            sched::Slot::every(5, &mut sig),    // signature-pattern battery; self-idles when inactive
        ];

        let mut next_stats = 10_000u32; // scheduler deadline telemetry every ~10 s
        loop {
            pet_wdt(&p);
            let now = clock.now_ms(&p);
            sched::tick(&mut slots, &mut cx, now);
            if now >= next_stats {
                // Per-task deadline health: worst lateness + missed-slot count. Under real load the
                // ~3 s POST hogs the loop for tens of ms, so radar/prox show that as their max-late —
                // the direct, quantified view of the "task running too long" concern.
                uart::puts(&p, "sched:");
                for s in &slots {
                    uart::puts(&p, " ");
                    uart::puts(&p, s.name());
                    uart::puts(&p, "(late");
                    uart::dec(&p, s.max_late().min(65535) as u16);
                    uart::puts(&p, "ms ovr");
                    uart::dec(&p, s.overruns().min(65535) as u16);
                    uart::puts(&p, ")");
                }
                uart::puts(&p, "\n");
                // Live board health (rail + die temp), re-measured each interval — a supervisor
                // watches these trend, and it keeps them visible if the boot [board] block scrolled
                // past or was missed on monitor attach.
                uart::puts(&p, "board: ");
                regs::vcc_temp(&p);
                uart::puts(&p, "\n");
                next_stats = now.wrapping_add(10_000);
            }
        }
    }
}

// Debug builds emit calls to abort(); MSP430 has no meaningful abort.
#[no_mangle]
extern "C" fn abort() -> ! {
    panic!();
}
