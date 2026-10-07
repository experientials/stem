#![no_std]

//! `deepsleep` — the **MSP430 deepsleep-supervisor module** capability crate.
//!
//! Both **prod** (the API/event-loop harness) and **diag** (the test harness) build on this crate, so
//! diag can host and exercise the real capability ("diag is a layer on prod"). It is **module-specific
//! only** — hardware lives in `bsp`, the wire vocabulary in `stembus`, sensor drivers in `devices`;
//! anything a sibling module (lowvision/bluetooth) would reuse belongs in a shared layer, not here.
//!
//! Extracted incrementally from the prod binary — see `docs/DEEPSLEEP-EXTRACTION.md` (derives from
//! ADR-0014). **Phase 0a:** the I²C-slave register-map decode (`regmap`).

/// I²C-slave register-map decode — re-exports the shared `stembus::regmap` wire contract and adds the
/// server-internal decode (`RegSel`/`decode`/`next_ai`) + `ShadowState`. (PCA9698-shaped layout kept so
/// the optional Linux-userland adapter stays implementable; see DEEPSLEEP-EXTRACTION.md §2.)
pub mod regmap;

/// Sensor-bus enumeration (Leaf-I²C master) + the async scan task. **Dual-I²C families only** — fr2433
/// is slave-only (no sensor bus), so this is gated `_dual`. Phase 0a (DEEPSLEEP-EXTRACTION.md).
#[cfg(feature = "_dual")]
pub mod enumerate;

/// Operating-mode machine (Passive ↔ Sensing) + the sensor-bus acquire/release handoff. Dual-I²C only
/// (fr2433 is Passive-only, no sensor bus to master) → gated `_dual`.
#[cfg(feature = "_dual")]
pub mod mode;
pub mod post;
pub mod status;

/// Stem-bus I²C-**slave** dispatch — the register-file/command-pointer layer that maps a master's reads
/// and writes onto [`regmap`], consuming the `bsp::stem_i2c` [`SlaveEvent`](bsp::stem_i2c::SlaveEvent)
/// transport seam. Present on EVERY node (the slave control surface is every chip's whole/partial job):
/// dual families serve it on eUSCI_B1, fr2433 on its one eUSCI_B0. Phase 1 (DEEPSLEEP-EXTRACTION.md §A).
#[cfg(any(feature = "_dual", feature = "fr2433"))]
pub mod stem;

/// cros_ec v3 EC-slave transport glue (COMMANDS.md §0 HELLO milestone) — binds the `bsp::stem_i2c`
/// [`SlaveEvent`](bsp::stem_i2c::SlaveEvent) seam to the pure [`stembus::cros_ec::EcSlave`] framer. Opt-in
/// via `cros_ec`; replaces the regmap slave on the FR2433 milestone build.
#[cfg(feature = "cros_ec")]
pub mod ec;

// ── Build identity (DEEPSLEEP-EXTRACTION.md §B) ──────────────────────────────────────────────────
// The **capability** semver is THIS crate's `CARGO_PKG_VERSION` — reported over the Stem regmap
// (DBG_FW_VER_*) + the boot log. The per-image **build stamp** is NOT here: it's crate-scoped
// `cargo:rustc-env` in each bin's build.rs, so the lib can't `env!` it — the bin injects it as a
// `&'static str` (e.g. `Status::booted(model, build)` / `post::boot_log(.., build)`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const VER_MAJOR: u8 = parse_u8(env!("CARGO_PKG_VERSION_MAJOR"));
pub const VER_MINOR: u8 = parse_u8(env!("CARGO_PKG_VERSION_MINOR"));
pub const VER_PATCH: u8 = parse_u8(env!("CARGO_PKG_VERSION_PATCH"));

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
