# ADR-0014 · Board-layer structure — compile-time family module + runtime model detect; LaunchPad modules serve as product-board modules

Status: Accepted
Date: 2026-10-06   Source: Henrik 2026-10-06 ("my ideal outcome is one image per product module … I prefer avoiding chip specific firmware images" · "we could just treat the LaunchPad modules as product-board modules"). Captured verbatim in STEM-DIRECTION.md ("One image per product module", "Same-boot + master-commanded mode").

Resolves the design gate behind **CONV-1** (prod `clock.rs`/`uart.rs` → `bsp::board`, [GitHub #5](https://github.com/experientials/stem/issues/5)). Builds on **ADR-0001** (one binary per compatible family) and **ADR-0012** (`stembus` shared contract).

## Context
Moving shared low-level code into `bsp` stalled on a genuine design question, not effort. Two layers
looked like they were competing to own "which chip, which pins":

1. **`bsp::board`** is **compile-time, per-`<chip>_<module>`** — the family feature selects one module
   (`fr2476_launchpad`, `fr2355_launchpad`) that owns clock trim, the console-UART instance+pins, the
   Leaf-I²C pins, the timer instances.
2. **prod's `model.rs`** is **runtime** — `detect()` reads the TLV device-id and maps role/pins/
   capabilities from the concrete part (ADR-0001's within-family detection).

And a second tension: prod targets a **product PCB**, while `bsp::board` only has **LaunchPad** modules.
Folding prod's `clock.rs`/`uart.rs` into `board` seemed to force a choice between these models, so it was
parked as "design-gated."

The parking was correct to flag but the framing was wrong: the two are not competitors.

## Decision

### 1. Compile-time family module **and** runtime model-detect are complementary axes, not alternatives
- **`bsp::board` = the compile-time FAMILY axis.** One module per PAC/wiring family, selected by the
  cargo feature. It owns what is fixed for the whole family: clock bring-up, the console-UART
  instance+pins, Leaf-I²C pins/driver, timer instances.
- **`bsp::model` = the runtime MEMBER axis.** `detect()` → `Model` → hardware-capability `pin_map`
  (+ role address via `stembus::role`). It resolves what differs *between members of one family*
  (FR2476 vs FR2475; which role address; 2nd-I²C present or not). Moved into `bsp::model` 2026-10-06.

"**One image per product module**" (STEM-DIRECTION.md, ADR-0001) therefore = **one compiled board
module per family, runtime-adjusted to the detected member**. The image is portable across a family's
members because second-sourcing (supply / errata / variant) is routine and per-chip firmware is a BOM
liability. The compile-time axis picks the PAC; the runtime axis adapts within it. No conflict.

### 2. LaunchPad modules **are** the product-board modules — for now
There is no product PCB yet. We do **not** block `bsp` convergence on an abstraction for hardware that
does not exist. The existing `<chip>_launchpad` modules serve as the product-board modules today. When a
real product PCB lands, add a **sibling** `<chip>_<product>` module beside it; the LaunchPad module stays
as the dev board. The `<chip>_<module>` naming already makes that a straight addition, not a rework.

This dissolves the "board has only LaunchPad modules vs prod targets the product PCB" half of the gate:
the LaunchPad module *is* the board the product build selects, until a product module exists to select
instead.

### 3. Consequent crate boundary (what lives where)
With the above, the layering is decided and un-ambiguous:

| layer | owns | compiled for |
|---|---|---|
| **`bsp`** | PAC alias · compile-time `board` (clock, console UART for *all* chips incl fr2433, Leaf-I²C, timers) · runtime `model` (detect + capability). **Protocol-free.** | MSP430 targets only |
| **`stembus`** | wire contract: `regmap`, `role`→addr, `model_name`, `crc`, mode codes | node **and** masters (RP2040/M7/host) |
| **`prod-core`** *(to create; name TBD — candidate `node`)* | shared node **capabilities**, individually invocable: Stem-slave service + RegFile dispatch, mode state machine, status/regfile assembly, Leaf enumerate, FRAM integrity | MSP430 targets |
| **prod bin** | the API + timer event-loop **harness** over `prod-core` | — |
| **diag bin** | the one-at-a-time **test harness** over `prod-core`, plus diag-only (menu/OLED/LED/stress/signature) | — |

This is the "prod is a layer diag builds on" end state (STEM-DIRECTION.md): emptying prod is **not** the
goal — prod bin stays as the thin event-loop harness; `prod-core` is the shared capability library both
harnesses drive.

## Consequences

**CONV-1 unblocks** (no longer design-gated — now mechanical):
- **Clock:** prod/`clock.rs::init_1mhz` is a verbatim copy of `bsp::board::clock_init_1mhz` (which
  already does per-module trim). prod calls `board`'s and the copy is deleted. Clock init is
  compile-time-per-family, so the runtime-model axis never applied here — this was always safe.
- **UART:** the text formatters (`puts`/`hex`/`dec`/`fixed2`) are byte-identical copies in prod and diag
  → one shared `fmt`. The fr2433 console (inline in prod only) becomes a `bsp::board` fr2433 console
  module; prod/`uart.rs` collapses to formatters + delegation.

**The `prod-core` bucket unblocks** (was gated on *this* decision): Stem-slave (CONV-3) splits into the
eUSCI-slave hardware half → `bsp::board` and the RegFile/dispatch half → `prod-core`; `mode`/`status`/
`post` lift into `prod-core` as individually-invocable capabilities; prod `enumerate` and diag's Leaf
scanner converge onto one shared probe core. This is what lets **diag host prod functionality** and the
**same-boot + master-commanded mode** behaviour (STEM-DIRECTION.md) fall out.

**Costs / caveats carried forward:**
- `pin_map` per-member is still **thin placeholders** (`bank_count: 5` for both dual rows). Because the
  one-image goal means the image must drive the *correct* hardware on a second-sourced part, these must
  be filled and **tested per family member** before multi-model portability is trusted in the field.
- Product-PCB `<chip>_<product>` board modules are **future work**; today LaunchPad modules stand in.
- `prod-core` crate name is not yet fixed (`node` is a candidate; it may become the "stem" library of the
  long-term rename). Naming is deferred to the extraction PR; the boundary above is what's decided.

## Status of follow-on work
- ✅ `model.rs` → `bsp::model` (2026-10-06).
- ▶ **Next (un-gated):** UART formatters → shared `fmt`; prod clock init → call `bsp::board` (delete copy).
- ▶ **Now unblocked (was gated):** `prod-core` extraction — Stem slave (CONV-3), `mode`/`status`/`post`,
  Leaf-scan convergence.
