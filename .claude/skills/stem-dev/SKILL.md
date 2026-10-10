---
name: stem-dev
description: >-
  Orientation and workflow for developing MSP430 firmware in the stem repo — building/flashing
  via the Docker toolchain + `just`, generating/regenerating svd2rust PACs, writing and extending
  the Rust `diag` power-on self-test firmware, wiring signals through the `connections.toml`
  registry, and the product's high-level goals (multiple MSP430s as low-power supervisor /
  I/O-extender nodes; FR2433 as the default part, FR2476 where battery/rail monitoring is needed).
  Use this whenever working anywhere under `stem/` — editing `diag/`, `pac/`, `board/`,
  `examples/`, the `justfile` or `docker/Dockerfile` — or when asked "how do I build/flash/test
  this", "add a sensor/device test", "regenerate the PAC", "which MCU or PAC do we target",
  "where's the pin map", or "what is this firmware even for". Be pushy about consulting it even for
  a one-line change: the repo has strong conventions (typed PAC register access, never-hang I²C,
  pins sourced from connections.toml, build-in-container/flash-on-host) that are easy to violate.
---

# Developing firmware in stem

`stem` is the MSP430 firmware for the Thepia/bob-929 hardware. This skill is the map: what the
firmware is *for*, where things live, how to build/flash/test, and the conventions to keep. It
links out to the repo's own docs rather than duplicating them — **read the linked doc for detail**.

## High-level goals (read before designing)

The MSP430 is a **low-power supervisor + I/O extender**, and the design assumes **multiple** of
them coordinating:

- Each node stays awake to monitor rails/signals while the main board is off, and wakes the
  system on a trigger; it also exposes its GPIO as an I²C **slave** (PCA9698/PCA9555-style
  register banks) so a host can read/write pins.
- Multiple nodes coordinate over a 1-Wire-over-UART **"Stem"** bus (address-based arbitration).
- **Production part = FR2433** (cheap, well-stocked, adequate). **FR2476** is the dev board on
  hand *and* the production variant where true analog battery/rail monitoring (eCOMP) is needed.
  Firmware must not depend on FR2476-only features if it may ship on FR2433.

Source docs (the live spec, even though written earlier):
- [../../FIRMWARE-API.md](../../../FIRMWARE-API.md) · [../../I2C-API.md](../../../I2C-API.md) ·
  [../../STEM-MSG.md](../../../STEM-MSG.md) · [../../HARDWARE.md](../../../HARDWARE.md)
- MCU choice + the FR2433-vs-FR2476 "port caveat":
  [bob-929/docs/MCU_SELECTION.md](https://github.com/experientials/bob-929/blob/main/docs/MCU_SELECTION.md).

> Note the two roles: **`diag` is an I²C master** that probes devices (bring-up tool); the
> **product firmware is largely an I²C slave** to a host. Don't conflate them.

## Related repos (siblings in the same workspace)

`stem` is the **shared MSP430 firmware**; the products that use it live in sibling repos next to
it. Reference them by path (they're separate repos), not relative link.

- **`bob-929`** — the "Bob" device (stereo MIPI-CSI2 vision + I²C sensors that recognises objects
  and behaviour). Primary consumer of this firmware and home of the hardware/bench docs:
  - `bob-929/Hardware/202/` — the **202 Combi** camera module that carries the MSP430 supervisor
    (TCA9534 I/O expander, micro-SD, TXS0104 level shifters); `bob-929/Hardware/929/`.
  - [docs/MCU_SELECTION.md](https://github.com/experientials/bob-929/blob/main/docs/MCU_SELECTION.md)
    (FR2422/2433/2476 trade-off + the ship-on-2433 port caveat),
    [docs/DEV_BOARDS.md](https://github.com/experientials/bob-929/blob/main/docs/DEV_BOARDS.md)
    (bench inventory + I²C addresses), `docs/IS31FL3730_EB.md`.
  - [.claude/skills/msp430-macos-dev](https://github.com/experientials/bob-929/tree/main/.claude/skills/msp430-macos-dev)
    — the **flashing/toolchain skill** (mspdebug setup, PAC generation, host-toolchain and USB
    gotchas). Consult it for anything host-side.
- **`ziloo`** — the "Ziloo" device (sibling product, same concept, Kickstarter-era). Shares hardware
  modules and the sensor mix; `ziloo/Hardware/` defines modules (101/201/202/701/801/909/919…) and
  `ziloo/Hardware/testing` has extra bench boards (e.g. 6DOF-IMU Click). Use it for cross-device
  hardware/pinout context and additional sensors the firmware may need to support.

Hardware modules (e.g. **202**) recur across bob-929 and ziloo — when a pinout or sensor is unclear,
check both products before assuming.

## Repo map

| Path | What |
|---|---|
| `diag/` | Rust POST/diagnostic firmware (the current focus). See its [DESIGN.md](../../../diag/DESIGN.md) + [README.md](../../../diag/README.md). |
| `pac/msp430fr2433/`, `pac/msp430fr2476/` | Vendored svd2rust PACs (typed register access). [pac/README.md](../../../pac/README.md). |
| `crates/bsp/connections.toml` | **Single source of truth** for every MCU pin/signal. [connections.toml](../../../crates/bsp/connections.toml). |
| `examples/` | `hello-c`, `hello-rust` (toolchain smoke tests), plus WIP device examples. |
| `justfile` + `*.just` | Command runner: `diag`, `pac`, `usb`, `example`, `check` modules. |
| `docker/Dockerfile` | The one amd64 toolchain image (msp430-gcc + pinned Rust nightly + just). |
| `scripts/gen-pac.sh` | Reproducible PAC regeneration. |
| `datasheets/` | Local PDFs (MCU + peripheral parts). |
| `TOOLCHAIN.md` | How the whole build/flash/CI setup works — [read this first for tooling](../../../TOOLCHAIN.md). |

## Build / flash / test

**Model:**
- **Build** targets a **Linux** toolchain: it runs **natively on Linux** — on CI (a Linux node)
  and on a **Raspberry Pi** — and on **macOS/Windows** it runs in a **Docker** container that
  simply provides that same Linux toolchain (amd64, under Rosetta on Apple Silicon).
- **USB comms (flash + `monitor`) always runs on the host OS** — Docker has no USB passthrough.
  On macOS this needs the one-time [msp430-macos-dev](https://github.com/experientials/bob-929/tree/main/.claude/skills/msp430-macos-dev)
  setup (x86_64 mspdebug + signed `libmsp430.dylib`). **`just usb`** checks and advises on USB
  drops/latches (the eZ-FET dropping off a hub is common).

CI runs the same `just` recipes. Full detail: [TOOLCHAIN.md](../../../TOOLCHAIN.md).

```sh
just bootstrap          # one-time: build the toolchain image
just check deps         # verify toolchain (--deep to actually compile; --flash for host tools)
just diag build         # build diag/ in the container
just diag run           # build + flash to the connected board
just monitor            # watch the 9600 8N1 backchannel (auto-detects the usbmodem port)
just usb status|recover # diagnose/recover the eZ-FET USB when it drops (hub/short/latch)
just example build hello-rust

# One chip-primary command builds any board's firmware; routes to prod/diag/probe + the right toolchain.
just build fr2355            # FR2355 product (prod)        → Docker toolchain
just build fr2355 --diag     # FR2355 POST/self-test (diag) → Docker toolchain
just build rp2040            # RP2040 bench host (stem-probe) → HOST cargo (no container; Linux+macOS)
just build-all               # every default image: MSP430 products (Docker) + the RP2040 probe
```

**Which build path (automatic via `just build <chip>`):** MSP430 (`prod`/`diag`, `msp430-none-elf`)
needs TI's external `msp430-elf-gcc` → **Docker**. The RP2040 (`debug/stem-probe`, thumbv6m) links with
**rust-lld** (in Rust) → **host cargo**, no container, Linux+macOS alike. Future RP2350/nRF52/PD = a
sibling firmware module + a `build` case arm. **Never** run raw `cargo build` for MSP430 and read the
missing-linker error as "toolchain missing" — it means you bypassed `just build`. See
[TOOLCHAIN.md](../../../TOOLCHAIN.md) "One `just build <chip>`, two link toolchains".

Flashing gotchas (USB drops, "No unused FET found", `cargo install svd2rust` hangs, the old
`cargo 1.64` on PATH) are all documented in the **msp430-macos-dev** skill — consult it whenever
`mspdebug`, the USB device, or the host Rust toolchain misbehaves.

## Adding / regenerating a PAC

PACs are **generated and vendored** (nothing pulls from crates.io — the published FR2476 crate is
yanked). Regenerate with `just pac gen [device]`; compile-test with `just pac check <device>`.
The pipeline (prebuilt `svd2rust` binary — do **not** `cargo install` it — + absolute-path stable
toolchain) and the reasons are in [pac/README.md](../../../pac/README.md) and the msp430-macos-dev
skill's `references/rust-pac.md`.

## Multi-model single image (combined / "dynamic" PAC) — HW-PROVEN 2026-10-10

ONE firmware binary CAN run on register-*incompatible* MSP430 models (e.g. FR2476 **and**
FR2355/FR2155) when the differences are few. This is now a **real, shipped prod target**, not a spike:
build `prod` with `--features fr2xxx` (→ `bsp/fr2xxx` → combined PAC `pac/msp430fr2xxx-dual`), artifact
`prod-fr2xxx-silent-cros_ec`. **HW-proven 2026-10-10:** the SAME byte-verified binary served its cros_ec
Stem slave at the chip-correct role addr on BOTH @fr2476 (0x21) and @fr2355 (0x20) — see
`docs/spikes/combined-image.md` "Productized (S2b)". The spike `spike/combined-mech/` de-risked the
mechanism first. The method:

- **Find EVERY divergence first — don't assume it's one peripheral.** Diff the generated PACs'
  `pub type … = crate::Periph<…::RegisterBlock, 0xBASE>` lines **and** grep each board file for which
  *instances* it uses (`grep -oE 'p\.[a-z_0-9]+'`). For FR2476 vs FR2355 the firmware-relevant set is:
  eUSCI_B1 base (0x0580/0x05C0), **console eUSCI_A** (A0@0x0500 / A1@0x0580 — different instance), **CSCTL1
  clock trim value** (manual vs factory), and the µs timer (TA0 vs TB1 — but **prod doesn't use it**, so it
  drops out). Missing one = a combined image that boots on one chip and hangs on the other.
- **Same-block / two-base peripherals → runtime overlay.** When the `RegisterBlock` layout is identical but
  the base differs, overlay the typed block on a model-selected base: `&*(base as *const RegisterBlock)`
  with `base` from `bsp::model::stem_i2c_base()` (or equivalent). Used for eUSCI_B1 (`stem_i2c` `_dynb1`
  path) and the console eUSCI_A (`board/fr2xxx_combined.rs`). No `static`/init-ordering needed — resolve
  per call from the TLV device-id (a cheap volatile read; valid even inside the wake ISR).
- **Same register, different VALUE → runtime select the value.** CSCTL1 clock trim: FR2476 writes an
  explicit manual DCO trim, FR2355 writes range-select only (factory trim; the FR2476 values won't lock the
  FR2355 FLL). `if model.is_fr235x() { factory } else { manual }`. Use the **bounded** FLL lock-wait (safe
  for both).
- **Dual-slot vector table — let the combined PAC emit it.** The combined PAC (`pac/msp430fr2xxx-dual`,
  feature `rt`) emits its OWN `__INTERRUPTS` with the SAME EUSCI_B1 handler at BOTH idx 45 (0xFFDA) and 47
  (0xFFDE) + a real `device.x`. Consumers (`prod fr2xxx`) enable `bsp/rt` + `msp430-rt/device` as usual —
  no hand-rolled table. Safe **only if no other firmware-used IRQ** maps to those slots (we use only
  eUSCI_B1). objdump the real artifact to confirm: handler addr at both slots, reset @ 0xFFFE.
- **Runtime pins + RAM floor + family guard.** Route per-model pins at runtime (ports share addresses);
  `memory-fr2xxx.x` RAM = the smallest model (intersection, 4 KB) with the 0xFF80 superset vector region;
  `model::matches_build_family()` must accept EVERY member the combined image covers (else it flags a false
  mismatch on boot).

**Gotchas:**
- The generated PAC's `Vector` union has **private fields** → a table can't be built from another crate.
  The combined PAC must **emit its own `__INTERRUPTS`** (the spike worked around this with its own identical
  `union Vector`; the productized PAC emits the real table).
- Console-OFF builds flush out latent bugs: `deepsleep::enumerate` gated the `Console` *import* but not its
  (unconditional) trait bound → every silent/combined build failed until the import was made unconditional.
  **Build the console-OFF / silent variant** when validating, not just the default console-ON one.
- Post-flash the Stem bus is briefly perturbed by the board reset — the **first `scan` after a flash can
  come back empty**; re-scan after a short settle before concluding a node is dead.

This is the "combined dynamic PAC = a **3rd build target**" idea, now real: `prod --features fr2xxx` is ONE
FR2355+FR2476+FR2155 image. Next: fold `prod` → `modules/deepsleep`. Dual-chip proof: [[prove-firmware-on-hardware]].

## Working on `diag` (the POST firmware)

`diag` boots, inits once, then loops a self-contained POST forever. **Read
[diag/DESIGN.md](../../../diag/DESIGN.md)** for the boot→cycle model and the extension contract before
adding anything. To add a device test: write a `src/<chip>.rs` driver (mirror `is31.rs`), then add
one line to the `TESTS` registry in `diag.rs`. Shape every test **probe → identify → exercise**,
report a clear PASS/FAIL, and use the bounded I²C helpers so a dead device fails instead of hanging.

## The connections registry

`crates/bsp/connections.toml` is the canonical map of signal → MCU pin → module/function → status. Add a
connection there (status `planned` → `active` only after verifying pin/function against the
datasheet) **before** wiring it in code, and don't scatter magic pin numbers across modules. It is
intended to drive both the C `board.h` and the Rust board crate.

## Conventions & invariants (why they matter)

- **Typed PAC register access**, not raw pointers — `p.e_usci_b0.ucb0ctlw0().modify(...)`, with bit
  values from the device header. Raw-pointer pokes (like the legacy `hello-rust`) lose the PAC's
  safety and are only for pre-PAC bring-up.
- **Never hang.** Every I²C wait is bounded (`SPIN`); a stuck bus reports/`recover()`s, it doesn't
  freeze the loop. A diagnostic that hangs is worse than useless.
- **Pins from `connections.toml`**, addresses/registers near their driver — one source of truth.
- **Verify pins & config bits from the authoritative doc, never from memory/silk.** Board wiring →
  LaunchPad user's guide (SLAU802) schematics; silicon config (ADC channels, PMM/reference bits,
  TLV, BSL) → the device datasheet. Guessing a pin (e.g. the S1 button) burns a 2–3 min build+flash
  cycle each miss. Known-good facts are collected in [references/hardware-notes.md](references/hardware-notes.md).
- **Thin `just` recipes** — real logic goes in `scripts/` (sh, or python via `uv`), not the justfile.
- **Chip-portable** — prefer the common FR2433/FR2476 peripheral subset; a feature-gated **board
  crate** (chip = cargo feature, role = FRAM config) is the planned layer that makes one source
  build per-chip. There is **no HAL** — firmware talks to the PAC directly.
- **Portable references** — cite files by **relative path** (in-repo) or **GitHub URL** (cross-repo,
  under `github.com/experientials/`), never absolute local paths (`/Volumes/...`, `/Users/...`), so
  guidance and commands stay valid across machines, CI, and the Pi.

## Where to read next

| You're doing… | Go to |
|---|---|
| Anything tooling/build/flash/CI | [TOOLCHAIN.md](../../../TOOLCHAIN.md) |
| Extending / understanding `diag` | [diag/DESIGN.md](../../../diag/DESIGN.md) |
| Regenerating or consuming a PAC | [pac/README.md](../../../pac/README.md) + macos-dev `rust-pac.md` |
| Flashing / mspdebug / host toolchain / USB trouble | [msp430-macos-dev skill](https://github.com/experientials/bob-929/tree/main/.claude/skills/msp430-macos-dev) + `just usb` |
| Pin assignments | [crates/bsp/connections.toml](../../../crates/bsp/connections.toml) |
| **Board/silicon gotchas** (button pins, ADC/temp config, TLV IDs, BSL, device quirks) | [references/hardware-notes.md](references/hardware-notes.md) — **check before guessing any pin or ADC/PMM bit** |
| Product intent / protocol | FIRMWARE-API / I2C-API / STEM-MSG / HARDWARE (repo root) |
| Which MCU / the port caveat | [bob-929/docs/MCU_SELECTION.md](https://github.com/experientials/bob-929/blob/main/docs/MCU_SELECTION.md) |
