//! `board` = the **module / PCB** (STEM-DIRECTION.md "Granularity of board") — the ONLY place chip
//! + wiring specifics live. Everything else in diag is chip-agnostic (via the `crate::pac` alias),
//! so callers use `board::clock_init_1mhz(&p)` etc. regardless of target.
//!
//! A module carries one MCU, so each file fuses the **chip** facts (PAC, clock trim, timer instance)
//! and the **board/module wiring** (backchannel UART instance + pins) into one `<chip>_<module>`
//! file. The cargo feature (Cargo.toml `[features]`) names the **chip** (the PAC axis) and selects
//! exactly one module below; the module is named in the filename. If one MCU ever lands on ≥2
//! modules, split the chip facts into a `chip::` layer these build on.
//!
//! The deltas today (see FR2355-SCOPE.md): clock trim strategy, backchannel UART instance + pins,
//! the µs timer instance (Timer_A on FR2476, Timer_B on FR2355), and RAM size (that one in build.rs).
//! Future: the semantic bus map (`leaf_i2c`/`stem_i2c`/`console_uart`/`spi`) belongs here too.
//!
//! Each module exposes the same surface (role+type named):
//!   clock_init_1mhz(p) · route_console_uart_pins(p) · console_uart_init(p) · console_uart_tx(p, b)
//!   · console_uart_regs(p) · console_uart_pin_report(p) · usec_start(p) · usec_now(p) -> u16
//! Chip-common role bindings (leaf_i2c, its pin route) live below, shared across modules.

#[cfg(feature = "fr2476")]
mod fr2476_launchpad;
#[cfg(feature = "fr2476")]
pub use fr2476_launchpad::*;

// fr2155 rides the FR2355 LaunchPad (its register superset) — same board wiring, same module. prod
// validates the FR2155 production part on FR2355 hardware; diag does likewise.
#[cfg(any(feature = "fr2355", feature = "fr2155"))]
mod fr2355_launchpad;
#[cfg(any(feature = "fr2355", feature = "fr2155"))]
pub use fr2355_launchpad::*;

// FR2433 (single-I²C, slave-only). Carries clock + console only — no Leaf master, no usec/ms base
// (no scheduler on this part). Folded here from prod's inline fr24xx paths (ADR-0014 / CONV-1).
#[cfg(feature = "fr2433")]
mod fr2433_launchpad;
#[cfg(feature = "fr2433")]
pub use fr2433_launchpad::*;

// ── Leaf I²C (semantic role: sensors / LCD / non-MCU nodes; the MCU is master) ─────────────────
// Bus-type + role in the call (`leaf_i2c`), per the agreed board seam. On BOTH current LaunchPad
// modules Leaf = UCB0 on P1.2/P1.3, so the binding + pin route are chip-common and live here rather
// than per-module; move them into a `<chip>_<module>` file if a future board wires Leaf to a
// different eUSCI_B. The Stem-I²C (slave uplink) and SPI roles land here later the same way.
// Gated on `_leaf`: only dual-I²C families have the Leaf **master** bus (`hal`/`i2c`). FR2433
// (single-I²C slave-only) has none, so this whole section is absent there.
#[cfg(feature = "_leaf")]
mod leaf {
    use crate::pac::Peripherals;

    const P1_LEAF_I2C_PINS: u8 = 0x0C; // BIT2|BIT3 -> UCB0 SDA/SCL (P1.2/P1.3)

    /// Route the Leaf-I²C pins (P1.2/P1.3 → UCB0). SEL1:SEL0 = 01 selects the primary module; clear
    /// SEL1 explicitly so we never depend on reset state selecting the wrong function.
    pub fn route_leaf_i2c_pins(p: &Peripherals) {
        p.p1.p1sel1().modify(|r, w| unsafe { w.bits(r.bits() & !P1_LEAF_I2C_PINS) }); // SEL1=0
        p.p1.p1sel0().modify(|r, w| unsafe { w.bits(r.bits() | P1_LEAF_I2C_PINS) }); // SEL0=1
    }

    /// The Leaf-I²C bus as an `embedded-hal` `I2c` master — the acquisition point drivers use, so
    /// they name the role, never the raw `e_usci_b0`. Backed by `hal::EusciI2c` → `crate::i2c` (UCB0).
    /// `allow(dead_code)`: retained seam for `crates/devices` drivers + prod (LTO-stripped, ~0 ROM);
    /// the diag POST currently drives `crate::i2c` directly.
    #[allow(dead_code)]
    pub fn leaf_i2c(p: &Peripherals) -> crate::hal::EusciI2c<'_> {
        crate::hal::EusciI2c::new(p)
    }
}
#[cfg(feature = "_leaf")]
pub use leaf::*;

// ── Watchdog role (chip-common): the ONE place the WDT field-name delta lives — dual `wdt_a` vs
// fr2433 `watchdog_timer` — so callers (diag, prod) are role-based, not model-hardcoded, and the
// backstop policy + constants aren't duplicated per firmware. Pattern: HOLD during deterministic
// bring-up, ARM the ~16 s ACLK(REFO) reset-mode backstop before the serve/scheduler loop, then PET
// every pass; a hang past ~16 s resets the node instead of wedging it. ACLK=REFO is the default, so
// this works even on the clock-less silent fr24xx path.
mod wdt {
    use crate::pac::Peripherals;

    const WDTPW: u16 = 0x5A00;
    const WDTHOLD: u16 = 0x0080;
    const WDTSSEL_ACLK: u16 = 0x0020; // WDTSSEL_1: clock from ACLK (REFO, 32768 Hz)
    const WDTIS_DIV_2E19: u16 = 0x0003; // WDTIS_3: /2^19 ≈ 16 s
    const WDTCNTCL: u16 = 0x0008; // reload the counter ("pet")
    const BACKSTOP: u16 = WDTPW | WDTSSEL_ACLK | WDTIS_DIV_2E19; // reset mode (WDTTMSEL=0)

    /// The ONLY per-chip delta: the WDTCTL field name. Everything else is identical silicon.
    #[inline(always)]
    fn write(p: &Peripherals, bits: u16) {
        #[cfg(feature = "_leaf")]
        p.wdt_a.wdtctl().write(|w| unsafe { w.bits(bits) });
        #[cfg(not(feature = "_leaf"))]
        p.watchdog_timer.wdtctl().write(|w| unsafe { w.bits(bits) });
    }

    /// Stop (hold) the WDT — during deterministic bring-up, before the backstop is armed.
    pub fn wdt_hold(p: &Peripherals) {
        write(p, WDTPW | WDTHOLD);
    }
    /// Arm the ~16 s reset-mode backstop — call ONCE, after bring-up, before the serve/scheduler loop.
    pub fn wdt_arm_backstop(p: &Peripherals) {
        write(p, BACKSTOP);
    }
    /// Pet the backstop (reload its counter) — call every loop pass; a hang that stops petting resets.
    pub fn wdt_pet(p: &Peripherals) {
        write(p, BACKSTOP | WDTCNTCL);
    }
}
pub use wdt::*;

// ── Millisecond time base (role): TB0 free-running from ACLK (REFO, 32768 Hz) — the tick the
// cooperative scheduler reads. The TB0 instance is hidden HERE so callers (the diag `Clock`) are
// role-based, not model-hardcoded (they used to poke `p.tb0` directly). This role is just start +
// raw read — like `usec_*`; the ms-folding math stays in the caller. Chip-common across the dual
// families (all have TB0, identical programming); gated `_leaf` because the slave-only fr2433 has no
// scheduler and its PAC omits TB0. (`usec_*` is per-module because its instance DIFFERS — TA0 vs TB1;
// the ms base is TB0 on every dual family, so it lives here, once.)
#[cfg(feature = "_leaf")]
mod ms {
    use crate::pac::Peripherals;

    const TBSSEL_ACLK: u16 = 0x0100; // TBSSEL_1: clock TB0 from ACLK
    const MC_CONTINUOUS: u16 = 0x0020; // MC_2: count to 0xFFFF and wrap
    const TBCLR: u16 = 0x0004; // clear counter + divider on start

    /// Start TB0 free-running from ACLK (REFO must already be live — `clock_init_1mhz` routes it).
    pub fn ms_timer_start(p: &Peripherals) {
        p.tb0
            .tb0ctl()
            .write(|w| unsafe { w.bits(TBSSEL_ACLK | MC_CONTINUOUS | TBCLR) });
    }
    /// Raw 16-bit TB0 counter (32768 Hz ticks). The caller folds wrap-safe deltas into monotonic ms.
    pub fn ms_timer_ticks(p: &Peripherals) -> u16 {
        p.tb0.tb0r().read().bits()
    }
}
#[cfg(feature = "_leaf")]
pub use ms::*;
