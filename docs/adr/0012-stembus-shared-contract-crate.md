# ADR-0012 · `stembus` — one shared crate for the Stem-bus contract + codecs

Status: Accepted
Date: 2026-10-04   Source: Henrik 2026-10-04 ("a library usable by M7, common functionality between M7 and stem-probe; CRC first")

## Context
The Stem-bus interface was defined **twice**: the server's `prod::regmap` (the MSP430 node) and the
master's hardcoded consts in `stem-probe` (`DBG_IFACE=0x30`, `MODE_CTRL`, model ids, role addresses…).
Two copies silently drift. The **i.MX8 M7** now needs the same interface (it masters + health-monitors
the nodes) plus shared logic — starting with the **FRAM-CRC integrity** management (ADR-0010): the M7
computes golden per-sector CRCs, reads the node's reported CRCs over I²C, and reflashes mismatches.

## Decision
A **pure-logic `no_std` crate `crates/stembus`** — no hardware deps, so it compiles for every
participant's target (`msp430-none-elf`, `thumbv6m`, `aarch64`/std). Transport (I²C) is injected via
closures; stembus never touches a peripheral. Modules:
- **`regmap`** — the register-map CONSTANTS (addresses, codes, role addresses, model device-ids, the
  PCA9698 command model, the debug window, `MODE_CTRL`, the `DBG_CRC` 0x3F window). The single source.
- **`crc`** — FRAM integrity: `crc16_ccitt`, `sector_crcs` (golden / live-FRAM), `read_sector_crcs` (the
  0x3F windowed-readout protocol), `mismatched_sectors` (the M7's judge).

Consumers: **`prod`** (`prod::regmap` does `pub use stembus::regmap::*`; `post::fram_crcs` uses
`stembus::crc::sector_crcs`), **`stem-probe`** (imports the regmap consts; its hardcoded copies deleted),
and the **M7** (depends on this repo as a git dependency).

## Consequences
- **No drift**: server and masters share ONE contract; a register change is a one-line edit in one place.
- **CRC golden == reported by construction**: the node and the M7 run the identical `sector_crcs`, so a
  CRC mismatch means real corruption, not an algorithm difference (makes ADR-0010 trustworthy).
- Server-internal decode (`RegSel`/`decode`/`next_ai`/`ShadowState`) stays in `prod::regmap` — not part
  of the wire contract, not every consumer needs it.
- Unit-tested on host (`crc16_ccitt("123456789") == 0x29B1`, the standard check value). `cargo test` works
  via `#![cfg_attr(not(test), no_std)]`.
- Builds verified across the whole matrix: prod lean/silent/`--debug`/fr24xx + stem-probe. The `std`
  feature is reserved for future M7-host helpers (e.g. golden-CRC from an ELF); the core needs none.
- Modules now present: `regmap` (contract constants), `crc` (FRAM integrity), `role` (model→role→addr
  + inverse labelings), `mode` (`MODE_CTRL` code↔Mode+name), `flags` (DBG_STATUS/DBG_FAULT bits), `msg`
  (STEM-MSG byte-frame layer — typed per-message builders deferred until STEM-MSG.md is finalized; the
  doc still has conflicts, e.g. `0x8` vs `0x40` for VSOM threshold). Wired: `prod` re-exports
  `regmap`+`flags`, delegates `stem_addr`/`model_name` to `role`, and uses `crc`; `stem-probe` imports
  `regmap` + delegates `role_name` to `role`. Host-tested (crc check-vector + msg frame roundtrip).
