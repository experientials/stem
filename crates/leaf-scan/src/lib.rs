#![no_std]

//! `leaf-scan` — Leaf-I²C (sensor bus) enumeration as a reusable capability.
//!
//! Split out of the old `deepsleep` crate (which was a product-named grab-bag): discovering what sits on
//! a node's local sensor bus is a *capability*, not deepsleep-specific — any `_dual` node that masters a
//! Leaf bus reuses it. It is generic over the `node_hal::LeafBus` seam (PAC-free) and probes through the
//! shared `devices` drivers, with bus-health detection so a wedged bus reads as a fault, not 112 phantoms.
//!
//! Public surface: [`Scan`]/[`Presence`] (the result the control surface reports), [`scan`]/[`scan_async`]
//! (sync + async executor task), [`leaf_scanner`] (the `sched` task), and the `console`-gated [`report`].

pub mod enumerate;

// Re-export the result types at the crate root so the control surface reads `leaf_scan::Scan` without
// naming the module (the `stem_node::status` scan-report field consumes this).
pub use enumerate::{Presence, Scan};
