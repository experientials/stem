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
