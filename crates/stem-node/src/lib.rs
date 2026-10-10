#![no_std]

//! `stem-node` — the shared **Stembus-node control surface** every node builds on.
//!
//! What a Stembus node does to serve its control surface and report identity/health, independent of which
//! product it is: the cros_ec serve + command dispatch (`ec`/`node`/`stem`), the boot self-test (`post`),
//! and the identity/status model (`status`). Consumed by BOTH `modules/deepsleep` (the sleep-supervisor)
//! and `modules/expander` (the I/O expander) — which is why it is named for the capability, not a product.
//!
//! It is hardware-free: dispatch is generic over the `node-hal` seam traits; the MSP430 impl (`bsp::StemHw`)
//! is wired by the bin. Deeper capabilities live in sibling crates — the wire vocabulary in `stembus`,
//! sensor drivers in `devices`, Leaf-bus enumeration in `leaf-scan`, hardware in `bsp`.
//!
//! (History: split out of the old product-named `deepsleep` crate — see `docs/DEEPSLEEP-EXTRACTION.md`.
//! `enumerate` → `leaf-scan`; the Passive/Sensing `mode` machine → the `modules/deepsleep` product.)

pub mod post;
pub mod status;

/// Stem-bus I²C-**slave transport bring-up** (`stem::init`). The command SURFACE above it is cros_ec
/// (`ec`/`node`), NOT a register file — the PCA9698 regmap slave was retired (COMMANDS.md §0: cros_ec is
/// the sole command interface). The shared wire CONSTANTS (role addr, mode codes, CRC) live in
/// `stembus::regmap`.
#[cfg(any(feature = "_dual", feature = "fr2433"))]
pub mod stem;

/// cros_ec v3 EC-slave transport glue (COMMANDS.md §0) — binds the `bsp::stem_i2c` `SlaveEvent` seam to the
/// pure `stembus::cros_ec::EcSlave` framer. The node's command interface (replaces the old regmap slave).
pub mod ec;

/// The `Node`+`Operate` adapter (`stembus::command`) over this node's live state — what the cros_ec
/// vendor+postcard path dispatches against.
pub mod node;

// ── Build identity (DEEPSLEEP-EXTRACTION.md §B) ──────────────────────────────────────────────────
// The **capability** semver is THIS crate's `CARGO_PKG_VERSION` — reported over the Stem surface
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
