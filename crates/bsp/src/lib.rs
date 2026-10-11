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
// The COMBINED image (`fr2xxx`): `pac` is the dual PAC (FR2476 register-block shape; the FR2355/FR2155
// delta is handled at RUNTIME, not by a distinct PAC). Its typed peripherals are correct for FR2476 and
// for every FR2355-family peripheral that shares FR2476's address; the three divergent peripherals
// (eUSCI_B1, console eUSCI_A, CSCTL1 trim) are reached via the runtime `_dynb1` / combined-board paths.
#[cfg(feature = "fr2xxx")]
pub use msp430fr2xxx_dual as pac;

// Exactly one chip feature must be on. "None" is caught here; selecting two makes `pac` a duplicate
// import (a clear "defined multiple times" error), so no verbose pairwise guard is needed.
#[cfg(not(any(
    feature = "fr2476",
    feature = "fr2355",
    feature = "fr2155",
    feature = "fr2433",
    feature = "fr2xxx"
)))]
compile_error!(
    "bsp: select ONE chip feature (fr2476|fr2355|fr2155|fr2433|fr2xxx) via the consumer (diag/prod)"
);

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
// ADC board-health (internal rail-mV + die-temp channels) — shared capability re-homed from diag/adc.rs
// for the node's `--diag` extended POST. Dual-only (diag's scope); FR2355/FR2155 cal values to-confirm.
#[cfg(feature = "_leaf")]
pub mod adc;
// The Stem-bus I²C **slave** transport — sibling to the Leaf-master `i2c` above. Present on EVERY node
// (the slave control surface is every chip's job): dual families run it on eUSCI_B1, FR2433 on its one
// eUSCI_B0. The protocol/register-file dispatch lives above this, in `deepsleep` (DEEPSLEEP-EXTRACTION.md §A).
pub mod stem_i2c;
// The MSP430 newtype wrapper (`StemHw`) that impls the host-clean `node-hal` hardware-seam traits
// (StemSlave/GpioBanks/LeafMaster) so `deepsleep` can be generic over the hardware instead of naming the
// PAC `Peripherals` (DEEPSLEEP-EXTRACTION.md Phase 6a). Holds the fr2433 PCA9698 bank→port backing.
pub mod node_hw;
pub use node_hw::StemHw;
