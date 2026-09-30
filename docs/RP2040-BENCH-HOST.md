# RP2040 bench/dev host — loading, wiring & structure (design)

Status: **design / planning** (no firmware yet). Companion to the verbatim brief in
[STEM-DIRECTION.md](../STEM-DIRECTION.md) ("Bench/dev host — RP2040 deep bus-inspection module").
Scope of THIS doc (per Henrik 2026-09-30): **hardware wiring planning + conceptual firmware loading +
firmware structure**. The `thepia hwd` support to drive the load/update cycle is a **separate task**
(spec'd in §2.4 so it can be lifted out); the host firmware itself is developed separately.

## 0. What this is

An **RP2040** acting as a **master/host on the internal busses** — a dev SoM stand-in / deep bus
inspector. It masters **Stem I²C** (to read the MSP430 nodes' slave/regmap surface), **Leaf I²C**
(sensors/LCD), and optionally **SPI**, plus the **full stembus** control lines (reset, bootsel,
wakeup, sleepy — STEM-DIRECTION §"full stembus") and the **i.MX 8M Plus (UCM) PMIC/boot pins**. It
starts as a **bench tester for MCUs + sensors** and may later slot into products as a debug module.

Distinct from the **RP2350** "Inner vision" *product* node — this RP2040 is a **dev host (master)**.

It is the **master side of the same chip-agnostic contracts the MSP430 serves as slave** (`regmap` /
PCA9698 register map, the stembus protocol, the semantic Stem/Leaf bus roles). So it **reuses those
contracts** and needs only its own `board`/`hal` for RP2040 silicon. Built as a **prod base** (the
"normal" foundation) with the dev/inspection capability as a layer on top — the same prod⊕harness
stack proven for MSP430 (STEM-DIRECTION §"diag and prod are variants").

---

## 1. Firmware loading & auto-update — THE FIRST CHALLENGE

### 1.1 Why the RP2040 makes this easy (vs MSP430)

The MSP430 needs an external programmer (eZ-FET/SBW). The RP2040 does **not**: it has an
**in-silicon, unbrickable mask-ROM USB bootloader** ("BOOTSEL mode"). No JTAG probe required for
production loading — the programmer is the chip's own ROM + USB. This changes the whole loading model.

In BOOTSEL mode the RP2040 exposes **two** USB interfaces:
- **USB Mass Storage (MSC)** — mounts as the `RPI-RP2` drive; drag/copy a `.uf2` → it writes QSPI
  flash and reboots. Human-friendly, but filesystem mounting is awkward to automate.
- **PICOBOOT** — a raw vendor USB interface that **`picotool`** drives. **This is the automation
  path** — no mount, fully scriptable.

**Tooling — `picotool` (verified 2026-09-30, [github.com/raspberrypi/picotool](https://github.com/raspberrypi/picotool)):**
the official Raspberry Pi CLI for RP2040/RP2350, **macOS + Linux** (prebuilt binaries via
pico-sdk-tools, Homebrew on macOS, or build from source). Subcommands: `load` / `verify` / `save` /
`erase` / `info` / `reboot` / `uf2` (+ RP2350 `otp`/`seal`/`encrypt`/`partition`). Programs over USB
BOOTSEL **with no external debug probe**. Note: `picotool` handles **programming** (flash/verify/
reboot); **run-time control** (the ad-hoc/soak test commands) is a **separate USB-serial (CDC)**
channel the firmware exposes — two channels, not one.

**Installing `picotool` (canonical, verified against the official
[BUILDING.md](https://github.com/raspberrypi/picotool/blob/master/BUILDING.md) / README, 2026-09-30):**

- **Prebuilt (simplest, no SDK):** download the Windows/macOS/Linux binary from the
  [pico-sdk-tools releases](https://github.com/raspberrypi/pico-sdk-tools/releases). Good for CI.
- **macOS convenience:** Homebrew provides a `picotool` formula — `brew install picotool`. (Homebrew,
  not an RPi-official channel; the RPi-documented routes are prebuilt + source.)
- **Build from source (the RPi-canonical route):**
  ```sh
  # deps — Linux (Ubuntu/Debian):
  sudo apt install build-essential pkg-config libusb-1.0-0-dev cmake
  # deps — macOS: your package tool for libusb + pkg-config + cmake (e.g. brew install libusb pkg-config cmake)
  export PICO_SDK_PATH=/path/to/pico-sdk        # required (or pass -DPICO_SDK_PATH=…)
  git clone https://github.com/raspberrypi/picotool && cd picotool
  mkdir build && cd build && cmake .. && make
  sudo cmake --install .        # do NOT just copy the binary into PATH — the SDK must be able to locate it
  ```
- **Linux non-root USB (required for unattended/CI automation):**
  ```sh
  sudo cp udev/60-picotool.rules /etc/udev/rules.d/    # from the picotool repo; lets hwd/CI run picotool without sudo
  ```

> For the **`thepia hwd` task** and the **Mac/Linux test harness**: depend on `picotool` (prebuilt or
> brew for macOS; prebuilt + the udev rule for Linux/CI) — no Pico-SDK build needed just to *use* the
> CLI. The SDK is only needed to build picotool from source.

Entry into BOOTSEL mode, two ways:
- **Hardware:** hold **BOOTSEL** low while **RUN** is released from reset (also entered on a blank /
  invalid flash). This is the **cold-recovery** path — works even on a bricked/empty chip.
- **Software:** the running firmware calls the ROM `reset_usb_boot()` and re-enumerates as PICOBOOT.
  **This is the normal update path** — no button, no extra wiring, triggered over USB.

### 1.2 Recommended loading model

**Initial (bootstrap) load:** once, via BOOTSEL (hardware) → `picotool load firmware.uf2`. On the
bench this can be a manual BOOTSEL the very first time, or automated immediately if BOOTSEL/RUN are
wired to a bench-controllable source (§3).

**Automatic updates (steady state):** software-triggered, zero extra hardware once firmware runs:
1. `thepia hwd` sends **"enter bootloader"** over the host's **USB CDC** control channel (a defined
   command), OR asserts the hardware BOOTSEL+RUN lines for cold recovery.
2. Firmware calls `reset_usb_boot()` → RP2040 re-enumerates as **PICOBOOT** (`2e8a:0003`).
3. `thepia hwd` runs `picotool load -x firmware.uf2` (writes flash, `-x` executes) — or a raw
   PICOBOOT write.
4. New firmware boots; `thepia hwd` confirms by reading a **build stamp** over USB CDC (the RP2040
   analogue of the MSP430 `--verify-stamp`).

**Unbrickable fallback:** if the firmware is wedged/blank and won't accept the CDC command, the
hardware **BOOTSEL + RUN** lines force BOOTSEL regardless → same PICOBOOT flash. So wiring BOOTSEL +
RUN to a bench-controllable source (§3.2) guarantees recovery. This is the key reason to wire them.

### 1.3 Board identity (for `hwd` addressing, like the MSP430 serial)

**Bench board (identified 2026-09-30, `picotool` v2.3.1 via Homebrew):** the dev-bench RP2040 is an
**RP2040 rev B2**, currently **blank** (no program), enumerating in **BOOTSEL** as `RP2 Boot` with
**serial `E0C9125B0D9B`** — the `picotool --ser` key and the future `hwd` board id. This is the board
v0 targets.

The RP2040 has no fixed serial, but the **QSPI flash chip's 64-bit unique id** is readable
(`picotool info` / the ROM `flash_get_unique_id`) and the running firmware should **also** expose it
over USB CDC + as the USB iSerial string. That id is the stable per-board handle `thepia hwd` uses
(the counterpart to the eZ-FET serial). VID:PIDs: `2e8a:0003` = bootloader/PICOBOOT, and a chosen
app PID (e.g. `2e8a:000a`-style CDC) when running.

### 1.4 `thepia hwd` support spec (SEPARATE TASK — hand this to the hwd work)

**Model: `picotool` is to `hwd` what `mspdebug` is** — `hwd` shells out to it for program/verify/
reset while keeping its **own** console, board-addressing, and verify-stamp layers (exactly as for
MSP430). **This repo does not implement it** — spec only. All flags below are verified against the
picotool README/BUILDING.md (2026-09-30).

**Board key = the RP2040 serial.** Firmware sets USB `iSerial` = the **flash unique id** (ROM
`flash_get_unique_id`); `picotool` targets it with **`--ser <uid>`** — the direct analogue of
`mspdebug`/`hwd`'s `--board <ezfet-serial>`. Works in both states (the PICOBOOT device carries the
same serial). `hwd` gives it an `@alias` like the MSP430 boards; state = `running` (app VID:PID +
CDC) vs `bootloader` (`2e8a:0003` PICOBOOT).

**Command mapping** (an `hwd rp2` family mirroring `hwd msp`):

| hwd concept | MSP430 (mspdebug) | RP2040 (picotool) |
|---|---|---|
| select board | `--board <serial>` | `--ser <flash-uid>` (or `--bus/--address`, `--vid/--pid`) |
| program + byte-verify | mspdebug program+verify | `picotool load -x -v fw.uf2 --ser <s>` (`-v` byte-verify, `-x` run) |
| enter prog mode | SBW (always) | `picotool reboot -f -u --ser <s>` (force running→BOOTSEL) **or** HW BOOTSEL+RUN |
| standalone verify | `mspdebug verify` | `picotool verify fw.uf2 --ser <s>` |
| reset | SBW reset | `picotool reboot -a --ser <s>` (or HW RUN) |
| read memory | `msp read <addr>` | `picotool save -r <a> <b> out.bin --ser <s>` |
| identify | eZ-FET serial + JTAG id | `picotool info -a --ser <s>` + USB descriptors |
| console | backchannel UART tty | **USB CDC tty** (same console layer + verify-stamp watch) |

**`hwd rp2 flash fw.uf2 --board <ser> [--verify-stamp <h>]`:**
1. **Ensure BOOTSEL:** if running, `picotool reboot -f -u --ser <ser>` → wait for PICOBOOT
   (`2e8a:0003`, same serial). If wedged/blank → fall back to the **hardware BOOTSEL+RUN** lines
   (§3.2) — the one recovery USB can't do itself.
2. `picotool load -x -v fw.uf2 --ser <ser>` — write + **byte-verify** (`-v`) + execute (`-x`).
3. **Verify-by-stamp:** watch the **CDC** console for the build stamp within a timeout (hwd's
   existing verify-stamp, over CDC). Report `programmed + verified + confirmed running`.

**Console:** treat the host's **USB CDC** as the console — `hwd console tail/send` over the CDC tty
(`/dev/cu.usbmodem*` on macOS, `/dev/ttyACM*` on Linux), same read-only-capture discipline.

**Control lines (optional, cold recovery + power sequencing):** if the bench exposes BOOTSEL and RUN
as `hwd`-addressable outputs (§3.2), add `hwd rp2 bootsel` / `hwd rp2 reset`. Needed only when the
CDC/`reboot -f` path can't be used (blank/wedged silicon).

**Two firmware requirements this imposes** (in the host firmware, not hwd):
- Link the **picotool-compatible USB reset interface** (Pico-SDK `stdio_usb`/reset; in Rust,
  embassy-rp USB or a `usbd-picotool-reset`-style class) — else `reboot -f` can't force BOOTSEL and
  only the hardware lines work.
- Set **`iSerial` = flash unique id** so `--ser` + hwd addressing are stable.

**Dependencies for hwd/CI:** just the `picotool` binary (prebuilt or Homebrew; Linux CI also needs
the udev rule, §1.1) + USB access. **No external programmer HW** (unlike MSP430's eZ-FET).

> Deliverable for the hwd task: an `rp2` command family (`identify`/`flash`/`verify`/`console`/
> `reset`/`bootsel`) wrapping `picotool` over USB PICOBOOT + a CDC console, keyed by the flash-uid
> serial (`--ser`), with a build-stamp verify — structurally the same adapter as the mspdebug one.

---

## 2. Hardware wiring / pin plan (RP2040 → the bench & internal busses)

RP2040 resources to allocate: USB1.1, **2× I²C**, **2× SPI**, **2× UART**, **2× PIO (8 SM)**, ADC
(3ch), ~26–30 GPIO, RUN, BOOTSEL(QSPI CS), SWD. Start on a **Pico-class board** for the bench; the
"slotted debug module" is a later custom PCB with the same logical map. GPIO numbers below are a
**provisional** allocation (confirm against the chosen board / final module BOM).

### 2.1 Bus/function allocation (provisional)

| Function | RP2040 resource | Provisional pins | Notes |
|---|---|---|---|
| Host USB | USB D+/D- | fixed | Flash (PICOBOOT/UF2) **and** the CDC console/control channel |
| **Stem I²C** (master) | I2C0 | GP4 SDA / GP5 SCL | Reads MSP430 node slave/`regmap` surfaces; multi-drop, addr per node |
| **Leaf I²C** (master) | I2C1 | GP6 SDA / GP7 SCL | Sensors / LCD / non-MCU leaf nodes |
| **SPI** | SPI0 | GP18 SCK / GP19 TX / GP16 RX / GP17 CSn | LCD or other SPI node (optional) |
| Stem **UART** | UART0 | GP0 TX / GP1 RX | "program or monitor a node over uart" (STEM-DIRECTION full stembus) |
| **Control lines** | GPIO | (assign) | reset, bootsel, wakeup, sleepy per node (full stembus) |
| **Level-shifter enables** | GPIO or I²C-GPIO | (assign) | The "directional level shifter on/off via i2c" — see §2.3 |
| **i.MX8 PMIC/boot** | GPIO | (assign) | ONOFF / PMIC_ON_REQ / POR_B / BOOT_MODE straps — see §2.4 |
| Rail monitoring | ADC (GP26–28) | GP26/27/28 | VSOM / charge / bench rails (through a divider) |
| **Deep bus capture** | **PIO** (2×, 8 SM) | flexible | Logic-analyser capture, odd/extra bus protocols, glitch/inject — see §2.5 |
| Firmware-dev debug | SWD | SWCLK/SWDIO | Optional — for live-debugging host firmware, separate from the USB-load path |

### 2.2 The loading/control pins (for §1)

- **RUN** (active-low reset) → a bench-controllable output (§3.2). Pulse to reset; hold to keep off.
- **BOOTSEL** (sampled on QSPI CS at boot) → a bench-controllable output. Hold low across a RUN
  release → forces BOOTSEL/PICOBOOT. This is the unbrickable cold-recovery path.
- **USB** → bench host (the primary load + console channel).

### 2.3 Level shifters & bus isolation

STEM-DIRECTION (full stembus, verbatim): *"Some lines are guarded by a directional level shifter that
can be switched on/off via the i2c."* The RP2040 host is 3.3 V; internal busses may sit at other
levels or must be isolated until intentionally probed. So each guarded line/bus goes through a
**directional level translator with an OE (output-enable)**; the host asserts OE only when it means
to drive/inspect that bus (default = isolated, so slotting the module into a live product is safe).
Whether OE is driven by a host GPIO or an I²C-GPIO expander is the "via the i2c" option — **decision
in §4**. This isolation is what makes "slotted into products for deep tests" safe.

### 2.4 i.MX 8M Plus (UCM) PMIC/boot access

To let the host power-sequence and boot-control the UCM-iMX8M-Plus (e.g. force USB-SDP recovery,
power-cycle, hold in reset), route through level shifters (§2.3) to the i.MX8/PMIC control class:
**boot-mode straps** (serial-download vs normal boot), **ONOFF**, **PMIC_ON_REQ**, **POR_B**, and
**PMIC_STBY_REQ / WDOG_B**. **Exact pin names/locations come from the Compulab UCM-iMX8M-Plus +
i.MX8MP datasheets — do NOT hardcode from memory** (see the `ucm-dev` skill / Compulab docs). This
section lists the *categories*; the concrete net list is filled when the UCM interface is pinned.

### 2.5 Why RP2040 (the inspection superpower)

**PIO** (2 blocks / 8 state machines) is what makes the RP2040 a real *bus inspector*: it can
capture busses at speed (logic-analyser), implement extra/unusual bus protocols beyond the 2 hard
I²C + 2 SPI, and do precise-timing stimulus — far beyond a fixed-peripheral MCU. Dual-core + 264 KB
SRAM give room for capture buffers + a USB command loop concurrently.

### 2.6 Pin allocation — per-board profiles & the Arducam Pico4ML constraint

The §2.1 layout assumes a **plain Pico** (free pins). The **Arducam Pico4ML** (the Inner-vision dev
base, STEM-DIRECTION point 3) is NOT free — its camera + LCD + mic hardwire most low GPIOs, and
crucially **`I2C0` (GP4/GP5) is the camera SCCB bus** — a direct clash with §2.1's Stem-I²C. So there
is no single fixed pin map; `rp-bsp` carries a **pin profile per (chip, module)**, and the semantic bus
roles (`stem_i2c`/`leaf_i2c`/…) bind to different GPIOs per board. Source: ArduCAM RPI-Pico-Cam +
Arducam Pico4ML docs (2026-09-30); **confirm the exact camera data-bus width/pins against the Arducam
schematic / RPI-Pico-Cam driver** before finalising (the HM01B0 data lines beyond GP6 overlap the LCD,
so the width is reduced/PIO-multiplexed — not yet pinned here).

**Arducam Pico4ML fixed usage (do not reassign):**

| Function | Pins | Notes |
|---|---|---|
| Camera **SCCB (I2C0)** | GP4 SDA / GP5 SCL | the camera control bus — **this is why I2C0 is not free** |
| Camera control | GP2 reset · GP3 XCLK · GP16 VSYNC · GP6 data-base (+ TBD data/PCLK/HSYNC) | parallel capture via PIO |
| **LCD** ST7735 (SPI1) | GP7 RST · GP9 DC · GP13 CS · GP10 CLK · GP11 MOSI | |
| **PDM mic** | 2 pins (TBD) | confirm from schematic |
| likely free | GP0, GP1, GP14, GP15, GP17–22, GP26–28(ADC), GP8/GP12(if not cam-data) | the bus budget on this board |

**Two board profiles in `rp-bsp`:**

- **`pico-bench` (plain Pico, the v0 host):** the §2.1 layout — Stem=I2C0 GP4/5, Leaf=I2C1 GP6/7, SPI0,
  UART0, etc. Full bus-mastering; no camera.
- **`arducam-vision` (Pico4ML, Inner-vision dev base):** the camera/LCD/mic own their pins; the
  bus roles remap onto free pins. Clean reconciliation using the semantic model:
  - **Leaf-I²C = I2C0 (GP4/5)** — the camera *is* a leaf node (a sensor), so it sits on the Leaf bus with
    any other leaf sensors. No change to the camera wiring.
  - **Stem-I²C = I2C1 on free pins** (e.g. GP14 SDA / GP15 SCL, or GP26/27) — the SoM/M7 uplink.
  - **SPI** is taken by the LCD (SPI1); a second SPI peripheral (SPI0) can use free pins if needed.
  - This board's *role* is vision, so it need not expose the full bench control-line set — it's the
    product/Inner-vision profile, not the bench inspector.

**Consequence / recommendation.** The bench v0 uses `pico-bench` (free pins, §2.1). For a **future
custom product module** (not a devkit) we *choose* the pins, so design that PCB to a **canonical stem
pin map** so firmware ports cleanly. The Arducam is the exception that proves the rule — its fixed
devkit wiring is why the role→pin binding must live per-profile in `board`, never hard-coded.

#### Don't re-map RP2040 → RP2350 (Henrik 2026-09-30)

Good news: **Arducam kept the same camera pin convention across RP2040 and RP2350** — HM0360 for
Pico/Pico 2 uses SDA=GP4, SCL=GP5, VSYNC=GP16, HREF=GP15, PCLK=GP14, D0=GP6 ([Arducam HM0360 for
Pico 2](https://www.arducam.com/blog/product/arducam-hm0360-vga-spi-camera-module-for-raspberry-pi-pico-2/)),
and **RP2350A is pin-compatible with RP2040** (Pico 2 = Pico form factor). So if we **align our
`arducam-vision` profile to Arducam's camera convention**, the RP2040 Pico4ML and the RP2350 ML boards
share one map — **no re-map on the chip bump**. Two hard constraints to design around:

- **Camera data = 8 *consecutive* GPIOs** (a PIO requirement): D0=GP6 ⇒ **GP6–GP13 reserved** for an
  8-bit camera. (The combined camera+LCD Pico4ML dodges this by running the camera at reduced width;
  a standalone HM0360 module uses the full GP6–GP13.) So on a camera board, treat **GP4–GP16 as the
  camera/SCCB block** and place everything else above it.
- **RP I²C pinmux: SDA on even, SCL on odd**, port-specific pairs. So the **Stem-I²C uplink** lands on a
  high free I2C1 pair clear of the camera block — e.g. **I2C1 GP18(SDA)/GP19(SCL)** or GP26/GP27.

**Recommendation for the product: use RP2350B (48 GPIO).** The camera block (GP4–16) + LCD + mic leave
almost no room on a 30-GPIO RP2040/RP2350A for the bench control-line set (Stem/Leaf, reset/bootsel/
wakeup/sleepy, level-shifter OE, i.MX8 PMIC/boot). **RP2350B's 48 GPIO** ends the squeeze — the camera
keeps its Arducam-canonical pins (no re-map) and the stem/control functions get the upper pins
(GP30–47). So the canonical map is: **low pins = Arducam camera block (fixed), upper pins = stem +
control**, identical RP2040(bench, subset) → RP2350A(vision) → RP2350B(product, full set).

---

## 3. Firmware structure

### 3.1 Placement & reuse — common / per-arch / debug (honours the 5 directional points)

The common denominator is a **spec + a conformance suite, NOT shared code** (Henrik 2026-09-30 —
STEM-DIRECTION §"Structural direction"). Shared *code* is the exception (same-runtime, proven overlap).

```
COMMON DENOMINATOR = SPEC + CONFORMANCE  (not a shared-code crate)
  I2C-API.md + PCA9698 tables   the ONE source of truth for the regmap/stembus contract
  conformance suite             the RP2040 HOST drives each node's slave surface against the spec
                                (+ shared test VECTORS an island's own unit tests can run); CI gates on it
  → each island implements the contract ITSELF (Rust or C), DUPLICATED by design, kept honest by tests

OPTIONAL SHARED CODE  (the exception — same-runtime only, on proven overlap)
  crates/devices   portable drivers over embedded-hal — shared ONLY where ≥2 Rust nodes genuinely touch
                   the same part; else duplicate + conformance-test
  crates/sched     MSP430/island-side (embassy has its own executor) — NOT cross-family

PER-ARCH HAL/BOARD  (separate crates — NOT cfg-merged across unrelated silicon)
  crates/bsp       MSP430 board/hal (msp430-hal)                 — cfg axis WITHIN MSP430 families
  crates/rp-bsp    NEW · RP2040 + RP2350 board/hal (embassy-rp)  — cfg axis WITHIN the RP family
  crates/nrf-bsp   future · nRF52 (embassy-nrf)
     each exposes the semantic roles: stem_i2c / leaf_i2c / console / spi  (+ embedded-hal)
     boards modelled as (chip, module): pico-class · arducam-tinyml+camera · …

PROD / PRODUCT-CAPABLE firmware  (normal location)
  prod/            MSP430 prod (exists)
  <rp product nodes, future>    e.g. Inner-vision on the Arducam board — reuse rp-bsp + common

SPECIAL DEBUG firmware  (SPECIAL location — e.g. debug/ or bench/; prod crates never live here — point 1)
  debug/<bench-host>   the RP2040/RP2350 bench bus-inspector / M7-stand-in BINARY:
     host-core (lib = the "prod base": scan_stem / read_node_regmap / scan_leaf / set_line / imx8_boot /
                rails — individually invocable) + a thin bin = the USB-CDC command loop over it (+ reset→BOOTSEL)
```

- **The contract is a SPEC kept in check by tests, not a shared crate (4):** the host reads a node
  using its *own* implementation of the regmap, and the **conformance suite** proves it matches what
  the node serves — so each island (MSP430 Rust, RP/nRF Rust, **CYPM1111 C**) can implement the
  contract independently (duplicated) without a shared-code dependency that couldn't span C anyway. The
  canonical form stays language-neutral (`I2C-API.md` + PCA9698; `regmap.rs` is *derived* from it). The
  host being the M7-stand-in tester means the consistency mechanism and the test harness are the **same
  artifact** — the anti-drift cost is ~zero.
- **host-core is the prod base AND the public `stembus` client:** capabilities as callable functions,
  not buried in a loop, so the bench harness drives them one-at-a-time now and a product/API loop — or
  the real **M7** — can call them later. This is the **published master-side client** (semver, docs)
  the M7 Zephyr repo consumes (Rust crate and/or C/Zephyr binding), concrete transport = the host's own
  I²C; the Mac/Linux harness uses the remote-over-USB transport. See STEM-DIRECTION §"Public master-side
  client". (The "expose the capability, not a forced `run()`" rule.)
- **The debug binary is segregated (1):** the bench bus-inspector lives in a **special location** and
  is built ON the normal crates; product firmware never lives beside it. Its command REPL over USB CDC:
  `scan`, `read <addr> <reg>`, `leaf`, `line <name> <0/1>`, `imx8 sdp`, `stamp`, `bootloader`.

### 3.2 Loading/control source on the bench

Something must drive **RUN + BOOTSEL** for cold recovery + own the USB. Options (decision §4):
(a) the bench host PC directly (USB for load; a USB-GPIO/FT232 for RUN/BOOTSEL); (b) a companion
controller; (c) rely on software `reset_usb_boot` for updates and only wire RUN/BOOTSEL to a header
for manual recovery. Recommendation: **(c) for v0** (software updates need no extra HW), add (a) when
`hwd` gets the control-line commands.

### 3.3 Toolchain & build

- **Framework = `embassy` — DECIDED (Henrik delegated the call, 2026-09-30).** One async framework
  across **RP2040 / RP2350 / nRF52** (`embassy-rp`, `embassy-nrf`), unified by embedded-hal(-async).
  Rationale of record (weighed against `rp-hal` + `nrf-hal`, sync): (1) only embassy satisfies points
  4 + 5 together — rp-hal/nrf-hal are separate ecosystems = the duplication (4) forbids; (2) the host
  is inherently **concurrent** (USB CDC ⊕ Stem/Leaf I²C ⊕ SPI ⊕ control lines ⊕ soak ⊕ PIO) — async's
  home turf; (3) embassy's **async USB stack** gives the CDC console **and** the picotool-reset
  interface (`reboot -f` → BOOTSEL) first-class; (4) nRF52 (5) is far nicer in embassy. **The one
  objection — async ≠ MSP430's polled style — is moot:** the thin shared surface makes the ARM nodes a
  separate island from MSP430, so async costs zero cross-family consistency. embassy sits **above** the
  sharing boundary; the common crates stay on pure + embedded-hal(-async) and never depend on it.
- **embassy is NOT the common denominator — TWO non-embassy islands:** **MSP430** (`msp430-hal`) and
  the **CYPM1111** Power-supply role (Cortex-**M0**, `thumbv6m`, dev board CY7111 — a Cypress/Infineon
  PMG1/PSoC-class part, ModusToolbox/C-first; embassy doesn't cover it, and whether it has any Rust HAL
  = verify when the Power role starts). The common denominator is the **spec + conformance suite** (§3.1),
  not embassy — and any *optional* shared code (`crates/devices`) stays on `embedded-hal`, below embassy,
  so both islands can use it. embassy is the framework for RP/nRF only.
- Targets: `thumbv6m-none-eabi` (RP2040), `thumbv8m.main-none-eabihf` (RP2350-ARM),
  `thumbv7em-none-eabihf` (nRF52) — same Rust toolchain, per-chip `--target`; pinned `rust-toolchain.toml`.
- Build → `.uf2` (`elf2uf2-rs` / embassy build). A new `just` module mirrors `prod.just`: `build` →
  `.uf2`; `flash` shells to `picotool` now, to `thepia hwd rp2 flash` once that lands (§1.4). `picotool`
  loads RP2350 too, so the load/verify flow generalizes RP2040 → RP2350 unchanged.

### 3.4 First increment (smallest end-to-end proof)

1. **Loading loop first (the stated first challenge):** RP2040 host bin = USB CDC printing a **build
   stamp** + **flash unique id** (= `iSerial`), accepting `bootloader` → reset→BOOTSEL. Proves the
   **load + auto-update + identity + verify** loop (§1) end-to-end. No shared crate — the host
   implements only what it uses.
2. **First conformance test:** add the **Stem I²C master** + a host-side copy of the contract (derived
   from `I2C-API.md`); `read <addr> <reg>` reads a live MSP430 node's regmap and **asserts it against
   the spec** (e.g. FR2433 `DBG_IFACE=0xD0`, model `0x8240`). This is the common conformance suite in
   miniature — the host's and the node's independent implementations agreed, with **no shared code**.
3. Then Leaf I²C scan, control lines, i.MX8 boot control, PIO capture — and grow the conformance/soak
   suite into the standing "common tests across islands".

---

## 3.5 Product angle — deep introspection with the SoM off (RESEARCH TODO)

> Verbatim (Henrik 2026-09-30): *"research how RP Pico might be used in a product to provide deep
> introspection and debugging over USB 2.0 while UCM SoM is switched off."*

**Finding: this is a "service processor" / BMC pattern, and it's feasible.** Researched 2026-09-30.

- **The pattern is proven at small scale.** RP2040/RP2350 already ship as on-board debug processors:
  the **RP2350B dev board uses an RP2040 running Picoprobe-style firmware** to debug the main RP2350B
  over SWD+UART ([Elektor](https://www.elektormagazine.com/news/rp2350b-development-board);
  [RP Debug Probe](https://www.raspberrypi.com/documentation/microcontrollers/debug-probe.html);
  non-invasive SWD register peek [PicoReg2](https://iosoft.blog/2025/03/30/picoreg2/)). Generalising
  that on-board debugger into a **management controller** for the i.MX8 = exactly a **BMC**: "a service
  processor that runs whenever the system is energised and handles power on/off/reboot"
  ([parisc BMC](https://parisc.docs.kernel.org/en/latest/bmc.html)). Frame the in-product RP that way.
- **USB-2.0-while-the-SoM-is-off works because the RP's USB is independent.** The RP2040/RP2350 has its
  **own USB 2.0 full-speed device controller**, unrelated to the i.MX8's USB. Wire a product USB port to
  the RP and it enumerates (picotool + CDC) regardless of SoM state. Better: the RP can be **bus-powered
  from VBUS**, so *plugging in a host powers the RP up* even if the product is otherwise off — the
  service processor is reachable on demand without an always-on rail. (An always-on/standby rail is the
  alternative when it must run untethered; the RP's dormant draw is low enough for that — confirm the
  exact standby budget against the RP datasheet when the rail is designed.)
- **Introspection surface with the SoM off** (from §2): the RP masters Stem/Leaf I²C, the control lines,
  rails/ADC, and the i.MX8 PMIC/boot pins — so it can read every node's regmap, dump voltages, sequence
  power, and **force the i.MX8 into USB-SDP recovery**. **SDP** is the i.MX boot-ROM's USB-HID protocol,
  the fallback when the boot media is empty/unbootable; it downloads images *and* can read/write
  registers ([NXP: enter SDP from SW](https://community.nxp.com/t5/i-MX-Processors/Enter-USB-Serial-Download-from-Software/m-p/1715466);
  [u-boot SDP README](https://github.com/ARM-software/u-boot/blob/master/doc/README.sdp)). It's selected
  by the **boot-mode straps** the RP drives (via level shifters). **Data-path caveat:** the SDP transfer
  runs over the *i.MX8's own* USB to a host — so the RP's job is to **sequence the SoM into SDP + power**
  (straps + ONOFF/POR_B), while the actual reflash streams over the i.MX8 USB (RP2040 USB-host is
  PIO-only and weak; don't plan to bridge the SDP payload through it). Exact i.MX8MP pin names
  (ONOFF / PMIC_ON_REQ / POR_B / BOOT_MODE) come from the i.MX8MP RM + Compulab UCM docs (`ucm-dev`).
- **Isolation makes it safe in a live product** (§2.3): directional level shifters with OE, tri-stated
  by default, so the RP only drives the internal busses when it deliberately introspects — otherwise it
  is invisible to normal SoM operation.
- **Security is THE decisive product concern.** An always-reachable debug master on a USB port is a
  powerful attack surface (power-control, bus access, SoM reflash). Mitigations to design in: ship the
  product variant on **RP2350** (secure boot + OTP so only signed RP firmware runs), **gate
  introspection behind an authenticated/signed command** (or a fuse/strap/physical enable), and treat
  the USB port as untrusted. Decide **DFT-only population** (don't fit the RP on production units) vs a
  **locked-down shipped** debug controller — this is the main go/no-go for shipping it.
- **Chip:** bench = **RP2040** (on hand); the in-product debug/service processor should be **RP2350**
  for the security features above. Same `host-core`/firmware, different power+enable+trust context — so
  this is the same code as the M7-stand-in (§0), just deployed on a standby/VBUS rail behind auth.

**Open (next research / design):** the exact standby rail + budget; the i.MX8MP strap/power net list
(from the RM/UCM); the auth/enable scheme; whether SDP reflash is host-driven over the i.MX8 USB or
needs the RP in any part of the path; and RP2350 secure-boot/OTP provisioning flow.

## 4. Decisions

**Resolved (from the directional points, STEM-DIRECTION §"Structural direction across MCUs"):**
- **Framework: `embassy`** (not rp-hal / nrf-hal) — unifies RP2040/RP2350/nRF52 (points 4 + 5). §3.3.
- **Structure: common crates shared by trait, per-arch bsp crates separate, no cross-silicon cfg**
  (point 4). §3.1.
- **Debug firmware in a special location; prod crates stay in normal locations** (point 1). §3.1.
- **RP as a chip axis (RP2040 now, RP2350 next)** in one `rp-bsp` (point 2). **Arducam TinyML+camera**
  is a board/module → dev base for the Inner-vision product (point 3). §3.1.

**Still open:**
1. **Crate/role name + the special debug dir** (e.g. `debug/`, `bench/`; binary `probe`/`benchhost`/
   `stemhost`). Used for the dir + `just` module.
2. **v0 target board:** a stock Pico/Pico-class bench host first, then the **Arducam TinyML** board
   with the vision role? (Recommend: plain Pico for v0 loading/bus bring-up; Arducam with Inner-vision.)
3. **BOOTSEL/RUN control source** on the bench (§3.2) — drives what `hwd` needs for cold recovery.
4. **Level-shifter OE control:** host GPIO vs an I²C-GPIO expander ("via the i2c").
5. **i.MX8/PMIC net list:** pin it from the Compulab UCM + i.MX8MP datasheets (`ucm-dev`) when ready.
```
