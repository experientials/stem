#![no_std]
#![feature(asm_experimental_arch)] // core::arch::asm! for SCG0 (SR bit) during the FLL retune (board)

//! `bsp` — the shared low-level layer both **diag** and **prod** build on (STEM-DIRECTION.md:
//! "board/hal must be a shared crate"; "diag is a special case on top of the normal prod baseline").
//!
//! Contents:
//! - the **chip-select PAC alias** (`bsp::pac`) — re-exports the one selected svd2rust PAC;
//! - **`board`** — the semantic peripheral map (clock, console UART, Leaf-I2C pins + accessor,
//!   µs timer), keyed per `<chip>_<module>`;
//! - **`hal`** — the `embedded-hal` `I2c` seam the shared `crates/devices` drivers talk to;
//! - **`i2c`** — the Leaf-I2C (UCB0) driver with bounded, stuck-bus-recovering transfers.
//!
//! The chip is picked by a mutually-exclusive cargo feature that the consuming crate forwards.

#[cfg(feature = "fr2476")]
pub use msp430fr2476 as pac;
#[cfg(feature = "fr2355")]
pub use msp430fr2355 as pac;
#[cfg(feature = "fr2155")]
pub use msp430fr2155 as pac;
#[cfg(feature = "fr2433")]
pub use msp430fr2433 as pac;

// Exactly one chip feature must be on. "None" is caught here; selecting two makes `pac` a duplicate
// import (a clear "defined multiple times" error), so no verbose pairwise guard is needed.
#[cfg(not(any(
    feature = "fr2476",
    feature = "fr2355",
    feature = "fr2155",
    feature = "fr2433"
)))]
compile_error!("bsp: select ONE chip feature (fr2476|fr2355|fr2155|fr2433) via the consumer (diag/prod)");

pub mod board;
// Shared console text formatting (puts/hex/dec/fixed2) over `board::console_uart_tx` — one copy for
// diag + prod (was duplicated in both `uart.rs`). Consumers re-export it and keep only init/rx/getc.
pub mod fmt;
// Runtime silicon identity + hardware-capability map (one image per compatible family). PAC-agnostic
// (a volatile TLV read); shared by prod's boot and diag's POST/`chip` command so there is ONE detector.
pub mod model;
// The Leaf-I²C master driver + its embedded-hal seam exist only on dual-I²C families (`_leaf`);
// FR2433 (single-I²C slave-only, PAC field `usci_b0`) has no Leaf master bus, so these are absent there.
#[cfg(feature = "_leaf")]
pub mod hal;
#[cfg(feature = "_leaf")]
pub mod i2c;
