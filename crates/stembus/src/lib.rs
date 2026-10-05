#![cfg_attr(not(test), no_std)] // no_std for real targets; `cargo test` links std for the test harness
//! # stembus — the Stem-bus wire contract + shared codecs
//!
//! One definition of the Stem-bus interface, shared by every participant so they agree **by
//! construction** (no more drift between the server's regmap and each master's hardcoded copy):
//! - **MSP430 node firmware** (`prod`) — the *server* that exposes the register map.
//! - **RP2040 bench host** (`stem-probe`) — a *master* (the SoM/M7 stand-in).
//! - **i.MX8 M7** — a *master* + health-monitor; depends on this repo as a git dependency.
//!
//! Pure logic, **no hardware dependencies**, so it compiles for `msp430-none-elf`, `thumbv6m-none-eabi`,
//! and `aarch64`/std alike. Transport (I²C) is injected by the caller via closures — stembus never
//! touches a peripheral. The `std` feature is reserved for future M7-host conveniences; the core is no_std.
//!
//! Modules:
//! - [`regmap`] — the register-map constants (the contract): role addresses, the PCA9698 command model,
//!   the debug/identity window, mode control, and the CRC window.
//! - [`crc`] — FRAM integrity (ADR-0010): the CRC algorithm + golden/readout/verify helpers.
//! - [`role`] — model → role → address + the inverse labelings (what a responder *is*).
//! - [`mode`] — `MODE_CTRL` code ↔ `Mode` + name.
//! - [`flags`] — `DBG_STATUS` / `DBG_FAULT` bit definitions.
//! - [`msg`] — STEM-MSG 1-wire notification framing (byte-frame layer).

pub mod crc;
pub mod flags;
pub mod mode;
pub mod msg;
pub mod regmap;
pub mod role;
