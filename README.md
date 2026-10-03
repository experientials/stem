# stem

MSP430 firmware for the Thepia **bob-929 / ziloo** hardware — a low-power **supervisor + I/O
extender** that monitors rails/signals while the main board sleeps and exposes its GPIO to a host
over I²C.

Two firmware crates:

- [`diag/`](diag/) — the bring-up ROM: a Rust power-on self-test (POST) that scans the I²C sensor
  bus and exercises each known device. Runs on the **FR2476** dev part (32 KB FRAM, 8 KB SRAM).
- [`prod/`](prod/) — the shipping **Stembus Expander** firmware for the product-baseline **FR2433**
  (15 KB program FRAM, 4 KB SRAM). An I²C **slave** exposing local GPIO/ADC to the SoM — *not* the
  central supervisor (that's the FR2476's 2× I²C role; see
  [ziloo/…/STEM-EXPANDER.md](../../ziloo/Hardware/stem/STEM-EXPANDER.md)). Currently a scaffold;
  `just prod build` enforces the 15 KB budget so it can never silently outgrow the part.

Production baseline part is the **FR2433**; the **FR2476** is the dev board and the
battery/rail-monitoring supervisor part. Both builds are footprint-gated in CI
([scripts/size-check.sh](scripts/size-check.sh)).

> New here? Read [TOOLCHAIN.md](TOOLCHAIN.md) (tooling) and [diag/DESIGN.md](diag/DESIGN.md) (the
> firmware model). Pin assignments live in [crates/bsp/connections.toml](crates/bsp/connections.toml).

## Prerequisites

- **`just`** — the command runner (one entry point: `just build <chip>`).
- **Docker** — only for the **MSP430** build (TI's `msp430-elf-gcc` linker lives in the pinned image;
  amd64, emulated on Apple Silicon). The **RP2040** build needs no container — host `cargo` + `rust-lld`.
- **Flashing (host, USB — never through Docker):** MSP430 needs the one-time `mspdebug` setup (x86_64
  mspdebug + signed `libmsp430.dylib`) from the **msp430-macos-dev** skill; RP2040 needs **picotool**
  (`brew install picotool` on macOS).

## Build

One **chip-primary** command builds any board's firmware — it routes to the right firmware class and
link toolchain automatically:

```sh
just build fr2355          # FR2355 product (prod)          → Docker toolchain
just build fr2355 --diag   # FR2355 POST/self-test (diag)   → Docker toolchain
just build rp2040          # RP2040 bench host (stem-probe) → host cargo (no container)
just build-all             # every default image (MSP430 products + the RP2040 probe)

just bootstrap             # once: build the MSP430 toolchain image (Docker path only)
just check deps            # verify the toolchain
```

Chips: `fr2355 | fr2476 | fr2433 | fr2155 | rp2040`. Default class = **prod**; `--diag` / `--debug`
pick the POST or debug image instead. **Two link toolchains, one command:** MSP430 needs TI's
`msp430-elf-gcc` → the **Docker** image (amd64, emulated on Apple Silicon); RP2040 links with
`rust-lld` (ships with Rust) → plain **host `cargo`**, no container, Linux + macOS alike. Full detail:
[TOOLCHAIN.md](TOOLCHAIN.md#one-just-build-chip-two-link-toolchains).

Under the hood `just build <chip>` delegates to the per-firmware modules — `just prod build <family>`,
`just diag build <part>`, `just probe build` — which you can also call directly.

**Faster MSP430 local loop:** one-time `bash scripts/setup-native-macos.sh` installs a segregated
native toolchain (never your system `rustc`; check with `just diag doctor`). After that the MSP430
build is native (no emulation) and **`just diag dev`** is the sub-minute build+flash inner loop.
Detail in [TOOLCHAIN.md](TOOLCHAIN.md#native-macos-build-optional-segregated).

## Flash

```sh
just diag run         # MSP430: build + flash + VERIFY the firmware reports the stamp  ← everyday bring-up
just prod flash fr247x # MSP430: flash a built product image (host; mspdebug / thepia hwd)
just probe flash      # RP2040: build + flash the bench host (picotool, USB BOOTSEL)
just monitor          # watch the 9600 8N1 backchannel UART (MSP430; auto-detects the port)
just usb status       # diagnose the eZ-FET USB if it drops off (hub/latch/short)
```

`just diag run` closes the "did my flash actually take?" gap — it stamps the build, flashes, then
reads the UART and refuses to succeed until it sees that exact stamp come back.

## Versioning

**There is no release system yet — every build is a dev build.** Firmware identity is a stamp
(`DIAG_BUILD`) baked in by [diag/build.rs](diag/build.rs) and printed on every POST banner, so the
board announces exactly what it's running:

```
[<target>/]<year>.<release>-<short-hash>[-dirty][.<secs>]
```

- **Today** (single FR2476 dev build): `2026.dev-59ad941-dirty.1725540000`.
- **`<target>`** is the real axis of variation — the chip/config variant (FR2433 vs FR2476,
  role/placement), kept to a few targets and FRAM-size-constrained, with the firmware autodetecting
  finer placement at boot. Set via `DIAG_TARGET` once the board crate builds per chip.
- **`<release>`** is `dev` until a release process sets a number (`DIAG_RELEASE=1` → `2026.1-59ad941`).
- Not semver — the crate `version` in Cargo.toml is unused by the firmware.

CI ([.github/workflows/hil.yml](.github/workflows/hil.yml)) builds the `diag` ELF in the toolchain
image (the canonical build; native is iteration-only), then a self-hosted runner with a board runs
`just diag hil`: it flashes and asserts over UART that the POST reports the stamp, all expected
devices are present, and nothing is FAULTY. Reproduce locally:

```sh
just diag run                                     # build + flash + confirm the stamp
EXPECT_STAMP=$(git rev-parse HEAD) just diag hil  # full HIL assertion against the board
```

The toolchain image is published to GHCR by
[.github/workflows/toolchain-image.yml](.github/workflows/toolchain-image.yml); set
`MSP430_IMAGE=ghcr.io/experientials/msp430-toolchain:latest` to skip `just bootstrap`.

## Layout

| Path | What |
|---|---|
| [`diag/`](diag/) | Rust POST / diagnostic firmware (current focus) |
| [`pac/`](pac/) | Vendored svd2rust PACs (`msp430fr2433`, `msp430fr2476`) |
| [`crates/bsp/connections.toml`](crates/bsp/connections.toml) | Single source of truth for pins/signals |
| [`examples/`](examples/) | `hello-c`, `hello-rust` — toolchain smoke tests |
| [`justfile`](justfile) + `*.just` | Command runner (`diag`, `pac`, `usb`, `example`, `check`) |
| [`docker/Dockerfile`](docker/Dockerfile) | The one amd64 toolchain image |
| [`scripts/`](scripts/) | Build/setup/PAC logic (recipes stay thin) |
