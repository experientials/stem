# prod — bob-929 Stembus node firmware (DUAL-I²C family axis)

> **S2a reorg (STEM-DIRECTION "Repo layout"):** the single-I²C **FR2433** slave-only node was EXTRACTED
> from this crate into its own binary — [`modules/expander/`](../modules/expander/) (cros_ec-only). Build
> it with `just build fr2433`. **prod is now DUAL-I²C only.** (The DESIGN.md sections below still describe
> the pre-split FR2433 design in places — a doc pass is pending; the authority for FR2433 is now the
> expander crate.)

The **shipping** MSP430 firmware for the bob-929 **dual-I²C** Stembus nodes. One source, a **compile-time
family axis** — `fr247x` (default) / `fr215x` / `fr235x` — selects the PAC + memory map + size-gate budget.
See [DESIGN.md](DESIGN.md) (requirements + strategy) and
[../docs/MCU_SELECTION.md](../docs/MCU_SELECTION.md) (part eval).

| Family feature | Parts | Role | I²C | Budget (gate) |
|---|---|---|---|---|
| **`fr247x`** (default) | FR2476 / FR2475 | **dual-I²C** node: Stem SLAVE + local sensor MASTER | 2× eUSCI_B | 32 KB / 8 KB |
| `fr215x` / `fr235x` | FR2155 / FR2355 | **dual-I²C** node (production baseline / dev superset) | 2× eUSCI_B | 32 KB / 4 KB |

`fr247x` is the **primary dev target** (the on-hand FR2476 dev boards). The production dual-I²C part is
the **FR2155** (`fr215x`); `fr235x`/FR2355 is its strict-superset dev LaunchPad (the FR2476 can't exercise
the 2nd eCOMP). The cheap FR2433 slave-only node lives in `modules/expander/` now (see note above).

> **Status.** `fr247x` now has its first working function: at boot it inits clock + UART + the sensor
> I²C master, prints a banner (`PROD_BUILD` + detected model), **enumerates the I²C bus**, and reports
> present devices — labeling known ones via the shared `crates/devices` registry. It then **reports on
> demand** over the backchannel UART: send `s` to re-scan, `r` to re-report. `fr24xx` (FR2433) is still
> the idle scaffold (bring-up lands with an FR2433 board). Remaining node duties in
> [`src/main.rs`](src/main.rs) (I²C-slave surface, per-sensor `measure()`, LPM3) are TODO.

Enumeration + reporting ride on the shared **[`crates/devices`](../crates/devices)** library
(`present`/`KNOWN`, generic over `embedded-hal` I²C) through prod's FR247x bus impl
([`src/hal.rs`](src/hal.rs)) — the same drivers diag will consume. Verify on hardware:
`just prod build fr247x && just prod flash`, then `just monitor` to see the boot banner + scan (use a
bidirectional terminal, e.g. `screen /dev/cu.usbmodem* 9600`, to send `s`/`r` for on-demand reports).

## Build & flash

```sh
just prod build            # build fr247x (default) + gate 32 KB/8 KB (FR2476/FR2475)
just prod build fr235x     # build FR2355 dual-I²C + gate 32 KB/4 KB
just build fr2433          # build the FR2433 expander (modules/expander) — NOT prod
just prod build fr247x fast  # no-LTO quick build (may exceed budget — release is what ships)
just prod flash            # program the connected board (match the family you built!)
```

The gate ([`scripts/size-check.sh`](../scripts/size-check.sh)) **fails the build** if the image exceeds
the family's program-FRAM window, static SRAM, or the stack floor. Footprint model (MSP430 FRAM part):
`FRAM = text + data`, `SRAM = data + bss`; stack floor = the RAM the build must leave free.

## How the axis works

- **PAC:** optional deps `msp430fr2476` / `msp430fr2433`; the active family feature pulls exactly one
  (a `compile_error!` guards zero/both). `src/main.rs` `#[cfg]`-selects the import and the one
  differing peripheral field (fr247x `wdt_a` vs fr24xx `watchdog_timer`).
- **Memory map:** [`build.rs`](build.rs) copies `memory-<chip>.x` → `OUT_DIR/memory.x` and adds it to
  the linker search path (`msp430-rt`'s `link.x` does `INCLUDE memory.x`). The per-chip maps are
  **GENERATED from TI's `.ld`** by [`../scripts/gen-memory-x.sh`](../scripts/gen-memory-x.sh) and validated
  by [`../scripts/lint-memory-x.sh`](../scripts/lint-memory-x.sh) (`just memory gen` / `just memory lint`):
  [memory-fr2476.x](memory-fr2476.x) (32 KB window @ `0x8000`, 8 KB SRAM),
  [memory-fr2355.x](memory-fr2355.x) / [memory-fr2155.x](memory-fr2155.x) (32 KB @ `0x8000`, 4 KB SRAM —
  byte-identical, enforced by the lint), [memory-fr2433.x](memory-fr2433.x) (15 KB @ `0xC400`, 4 KB SRAM).
  `VECTORS` is an `msp430-rt`/PAC property (link-ASSERTed to end at `0x10000`), not taken from the `.ld`.
- **Model detect:** [`src/model.rs`](src/model.rs) reads the TLV Device ID (0x1A04) at boot — FR2476
  `0x832A`, FR2475 `0x832B`, FR2433 `0x8240` — and `matches_build_family()` catches a wrong-family flash.
- **Budgets:** selected by family in [`../prod.just`](../prod.just).

## Identity stamp

[build.rs](build.rs) bakes in `PROD_BUILD` (`<target>/<year>.<release>-<hash>[-dirty][.<secs>]`, target
defaults from the family feature — `fr2476` / `fr2433`), same scheme as diag's `DIAG_BUILD`. No release
system yet — every build is a dev build.

## TODO (derive from the objectives, don't freelance)

Per [.claude/rules/objectives-tracking.md], the node's duties come from the canonical docs
([STEM-EXPANDER.md](../../ziloo/Hardware/stem/STEM-EXPANDER.md) / [I2C-API.md](../I2C-API.md) /
[STEM-MSG.md](../STEM-MSG.md)) + DESIGN.md, not from memory. Fill `src/main.rs`'s stubs: clock, UART
banner, pin map ([crates/bsp/connections.toml](../crates/bsp/connections.toml)), eUSCI_B I²C-**slave**
register map ([regmap.rs](src/regmap.rs)), sensor-bus **master** + enumeration (dual-I²C families),
local ADC/GPIO sensing, LPM3 idle. Feature-gate heavy blocks to fit the smaller (`fr24xx`) budget.
