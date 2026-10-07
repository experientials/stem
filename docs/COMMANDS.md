# Command system across MCUs

**Status:** DESIGN (the first implementation is Phase 2 of [DEEPSLEEP-EXTRACTION.md](DEEPSLEEP-EXTRACTION.md),
on the MSP430 `deepsleep` module). This doc is the **cross-MCU overview** — the common command interface
every MCU module (MSP430 `deepsleep`, future `lowvision`, `bluetooth`, …) exposes to the SoM/M7. Register
map + role addressing live in [`I2C-API.md`](../I2C-API.md) (the transport contract); this is the command
*model* layered on it.

## 0. Direction update (2026-10-07): ChromeOS EC protocol, not PCA9698, for Linux leverage
> (Henrik 2026-10-07) I found out that ChromeOS has already solved the Linux I2C communication protocol
> for MCUs. This means that Supporting PCA9698 on the Stem bus is no longer the best solution for
> leveraging existing Linux driver support.

The **ChromeOS EC host-command protocol (`cros_ec`)** is the Linux-standard way a host talks to an MCU
("EC") over I²C/SPI — a far richer leverage point than PCA9698's GPIO-only `gpio-pca953x`:
- **Mature Linux stack:** `drivers/mfd/cros_ec` + `cros_ec_i2c` → `/dev/cros_ec` + sub-drivers (GPIO,
  sensors via IIO, PWM, I²C-passthrough, …) + the `ectool` userland.
- **Firmware update built in:** `ectool` reads/writes EC flash over host commands while the OS runs — our
  "receive firmware patches" requirement WITH host tooling (PCA9698 offered nothing here).
- **Fits our slave:** protocol v3 = a request/response packet (`ec_host_request`: struct_version=3,
  checksum [all bytes sum to 0], command, command_version, data_len; `ec_host_response`: result, data_len,
  checksum). On I²C the **EC clock-stretches until the response is ready** — which our MSP430 eUSCI slave
  does natively (slau445i §24.3) — and returns filler `0xEC` on over-reads. A Linux host binds via
  `EC_CMD_GET_PROTOCOL_INFO` / `EC_CMD_HELLO` / `EC_CMD_GET_CMD_VERSIONS`; a reserved board-specific
  command range carries our own semantics.

**Supersedes PCA9698 as the Linux-leverage path** (§4 is now legacy/narrow). It very likely **also
supersedes the bespoke `postcard` control-bus protocol (§1)**: if every MCU is a `cros_ec` "EC", *that*
is the common command API — standard commands bind standard Linux sub-drivers for free; vendor commands
carry our semantics. `postcard` may survive only for MCU↔MCU / non-Linux links, or as a vendor-command
payload encoding — TBD.

**Open decisions (this is a protocol PIVOT, not a drop-in — for Henrik):**
1. **How much cros_ec?** (a) standard commands mapped onto our GPIO/sensor/mode semantics → maximal Linux
   sub-driver leverage; (b) cros_ec as transport + mostly vendor commands → channel + ectool + firmware,
   less sub-driver leverage; (c) keep `postcard`, use cros_ec only where Linux leverage is wanted.
2. **FR2433 fit (15 KB):** a MINIMAL cros_ec EC (v3 packet + checksum + the 3 baseline commands + a few
   of ours) — measure; the protocol core is small, the command *set* is what grows. We implement a
   subset, not the full ChromeOS EC OS.
3. **Who masters which bus?** (TENTATIVE, Henrik 2026-10-07): **Linux (A-core) masters the Stem/control
   bus, M7 masters the Leaf/sensor bus** — *"not certain; depends on how often Linux is suspended."* If
   that holds it's the GOOD case for cros_ec (Linux drives the command bus → leverage applies; Leaf stays
   the M7's non-Linux domain → postcard/bespoke). The real driver is **Linux suspend frequency**: if Linux
   is often down, the Stem bus may need a **when-suspended secondary master** (M7, or the PD controller —
   `I2C-API.md`'s existing "SoM-off master"), making Stem **multi-master**; cros_ec then helps only the
   Linux master, the secondary reimplements the protocol. So this is a **power-architecture decision
   upstream of the protocol choice** — keep both open together.
4. **`postcard`'s fate** (per the above).

Sources: [AP↔EC comm protocol](https://chromium.googlesource.com/chromiumos/platform/ec/+/HEAD/docs/ap-ec-comm.md) ·
[cros_ec_i2c driver](https://codebrowser.dev/linux/linux/drivers/platform/chrome/cros_ec_i2c.c.html) ·
[Chromium EC README](https://chromium.googlesource.com/chromiumos/platform/ec/+/HEAD/README.md) ·
[(Ab)using Google's Chromium EC (FOSDEM'17)](https://mfischer.github.io/fosdem17-slides/).

### Cross-platform support → ONE vocabulary, per-link ENCODING (Henrik 2026-10-07)
> It likely pushes out postcard, but it can still be relevant for sending commands between two Rust
> firmwares, unless the protocols are supported on macOS, iOS, MSP430, Windows, etc

Checked (sources below): cros_ec is **native on Linux only**; Windows/macOS are niche community tools
(CrosEC / coolstar `croseci2c`), iOS has nothing, the Rust host crate (`crosec-rs`) is WIP/unpublished,
and on the MCU (MSP430/RP2040) we **implement** the EC side ourselves regardless. `postcard` is the
inverse: native Rust↔Rust, a lib-port everywhere else. So neither is universal → **don't force one wire
format everywhere, and don't lock the choice now.**

> **Decision DEFERRED (Henrik 2026-10-07):** *"keep options open and see where implementation ends [up]."*
> cros_ec / postcard / PCA9698 are candidate wire faces, not a settled pick — implementation reveals which
> pays off per link. What keeps this cheap is the decoupling below: design the **command semantics** (the
> `stembus` vocabulary + each module's `execute`) INDEPENDENT of the wire encoding, so a face can be
> added/swapped late without touching the command set (the transport↔dispatch↔state layering,
> `references/firmware-architecture.md` §9 — the anti-lock-in move).

Working guidance (NOT locked):
- **ONE command vocabulary** (the semantic `Command` set in `stembus`) with **per-link encodings**:
  - **cros_ec host-command framing** on the **Linux control bus** — leverage the kernel stack + `ectool`
    + firmware update (only pays off if the master is Linux — the open question in §0);
  - **`postcard`** for **Rust↔Rust** MCU links (both ends ours → no ecosystem needed; cros_ec would be
    dead weight there).
- The **phone/app side (iOS/macOS/Windows)** is almost certainly a **separate channel — BLE via the
  bluetooth module** with its own (GATT) protocol — so NEITHER cros_ec nor postcard is turn-key there and
  it does **not** drive this choice. (Confirm the app channel.)

Extra sources: [crosec-rs (Rust)](https://github.com/FyraLabs/crosec-rs) · [framework-ec / ectool ports](https://github.com/DHowett/framework-ec).

### Constraint: the Linux side MUST be Yocto-buildable (Henrik 2026-10-07)
The i.MX8 SoM image is a **Yocto/OpenEmbedded** build, so whatever Linux-side command stack we pick must
express as **kernel config + device-tree + a userland recipe** — no out-of-tree hacks. How the candidates
map (another reason to keep the choice open until we try it):
- **cros_ec — YES, available on Yocto (verified 2026-10-07), the lightest integration:**
  - **Kernel:** the drivers are **mainline** (`CONFIG_CHROME_PLATFORMS`, `CONFIG_MFD_CROS_EC_DEV`,
    `CONFIG_CROS_EC_I2C`, + IIO/GPIO sub-drivers) → just a **kernel-config fragment** in the `linux-*`
    recipe + a **DT node** on the SoM I²C bus (`compatible = "google,cros-ec-i2c"`). **No custom driver,
    no special layer** for the kernel side.
  - **Userland:** the community **`meta-chromebook`** layer ships an **`ectool-chromium`** recipe (ectool +
    EC modules) — a ready path; or build `crosec-rs` via `cargo.bbclass`. (fwupd also has ChromeOS-EC
    support if we want managed firmware update.)
  - **Age/version:** cros_ec has been **mainline since Linux 3.10** (2013; in `drivers/mfd/`), **moved to
    `drivers/platform/chrome/` in 4.19** (2018). **Yocto Dunfell 3.1 = kernel 5.4** (Apr 2020), so a
    **~5-year-old Yocto already has the driver** — no Yocto migration needed *just* for cros_ec; it's a
    kconfig fragment + DT node on that kernel.
  - **⚠️ Verify on OUR kernel:** the i.MX8 build usually rides NXP's **`linux-imx`** (meta-imx), which can
    trail mainline — confirm that version carries `drivers/platform/chrome` + `CONFIG_CROS_EC_I2C` + the DT
    binding (present-but-disabled is the likely state → enable via fragment). The **userland recipe**
    (`meta-chromebook`/`ectool-chromium`) may need porting to an old release — vet/vendor it. Note a
    5-yr-old Yocto (Dunfell) is **community-EOL** (~2024) — migrating may be worth it for security/support
    *regardless* of cros_ec, but that's a separate decision, not forced by this.
- **postcard / bespoke on Linux — needs a userland recipe + the Yocto Rust toolchain** (`cargo.bbclass`,
  `meta-rust`-style), and a raw-I²C userland path (`i2c-dev`), with NO kernel sub-driver leverage (we'd
  write the host client). Heavier to own, lighter on kernel.
- **Either way:** confirm the SoM I²C controller + the MCU slave address are in the DT, and that the build
  layer has Rust support if a Rust userland tool is chosen. Prove a minimal recipe early (hello-EC over
  `/dev/cros_ec`, or a raw-`i2c-dev` postcard ping) so the packaging path is de-risked before committing.

**Everything below (§1–§5) predates this pivot** — kept for the problem framing (two buses, framing,
firmware-patch streaming, per-module execute), but the *wire protocol* is now cros_ec-leaning, not
PCA9698 and likely not bespoke-postcard. Revise once the open decisions land.

## 1. The model — one command vocabulary, two transports, per-module `execute`
- **One `Command` vocabulary** in the shared **`stembus`** crate (the codec + the command set the SoM and
  every MCU compile against — [ADR-0012](adr/0012-stembus-shared-contract-crate.md)). Serialized with
  **`postcard`** (compact `serde` binary, `no_std`, no-alloc).
- **Each MCU module implements its own `execute`** over that vocabulary — the command set is common; the
  *handling* is per-module. The command/dispatch layer is separate from the bus transport and from the
  device state (transport ↔ dispatch ↔ state; see `references/firmware-architecture.md` §9).
- **Two transports, differing only in FRAMING:**

| | I²C (control bus) | UART |
|---|---|---|
| Message boundary | **free** — `START…STOP` delimits one message; the slave buffers RX bytes to `Stop`, then deserializes | **none** — a raw stream, so COBS or a length prefix (`postcard` provides COBS: `to_slice_cobs`/`CobsAccumulator`) |
| Direction | R/W bit: **write** = command (master→MCU); **read** = response (MCU→master) | full-duplex |
| Request→response | write command → read response (or combined repeated-START), reply pre-staged / clock-stretched | interleaved |

- **Build order (CANONICAL — verbatim, Henrik 2026-10-06):**
  > We will redesign the command system anyway starting with commands over I2C and then commands over UART

  → **I²C commands first, UART second**, and it's a deliberate redesign (not a retrofit of the current
  regmap menu). Corollary: the existing **polled console `getc`/`rx_ready` menu is throwaway** — the UART
  command path is replaced, so don't invest in extending it.

## 2. Buses & addressing — the SoM masters TWO buses (CANONICAL — verbatim, Henrik 2026-10-06)
> We have two I2C busses for the SoM M7 to master while it runs. One has sensors and MSP deepsleep
> modules in passive, the other is meant for controlling MCUs. The MCUs would be controlled using
> commands with postcard data and receive firmware patches. Whether MCUs would all support PCA9698 style
> API is doubtful, and it would make sense to have a common API to all MCUs.

- **Control bus** = the Stem bus — the MCU uplink: commands + firmware patches, every MCU a **slave**,
  addressed by **role** (see [`I2C-API.md`](../I2C-API.md) "Stem address is ROLE-BASED": `0x20` supervisor,
  `0x21` dev stand-in, `0x22` I/O-expander, `0x23` PD controller).
- **Sensor bus** = Leaf — sensors **and** the MSP as a passive slave (the SoM masters it directly while the
  MSP is Passive — the "reachable on both" refinement; see DEEPSLEEP-EXTRACTION.md behaviour model). This
  bus is nice-to-have for the MSP's own Sensing role; the command system rides the **control** bus.
  *(Bus mapping is my read — confirm against the behaviour-model block, which is the authority.)*

## 3. Firmware update — IN-FIELD, SoM-driven, ROW-BASED (CANONICAL, Henrik 2026-10-07)
> The SoM will field update MCU firmware. It has UART and Stem I2C available. I expect we will follow the
> row based firmware update available for the PD Controller.

So **flashing IS a bus-command category** (not dev-only): the SoM delivers firmware to each MCU in the
field, over **UART or Stem I²C**, following the **row-based** scheme the **PD controller** uses (Cypress/
Infineon EZ-PD PMG1 bootloader — flash written a **row/page at a time** with verify; confirm the exact
protocol when that firmware is developed). The common update flow ≈ **enter-update → write-row(index,
bytes) → verify (CRC/row-check) → activate** — a *sequence of bounded rows*, not one opaque blob (fits
the §1 streamed framing; each row is one transaction). **Per-MCU apply differs** behind the same command
shape: MSP430 FRAM can't A/B self-update → host-driven BSL/STEM reflash (memory
`msp-fw-ota-host-root-of-trust`); RP2040/RP2350 (deepsleep? lowvision) via its own flash; the PD
controller's own row bootloader is the template.

## 4. Coexistence with PCA9698-emulation (SUPERSEDED — see §0)
> **Superseded 2026-10-07:** PCA9698 is no longer the Linux-leverage path — `cros_ec` (§0) covers GPIO
> *and* sensors *and* firmware update *and* a generic channel with a richer Linux stack. Keep this section
> only as a narrow legacy option (a lone PCA9698 GPIO view) if a specific consumer ever needs exactly that.

PCA9698-emulation is an **optional per-module compatibility shim, NOT the common denominator** — not all
MCUs will support it ("whether MCUs would all support PCA9698 style API is doubtful"). Where a module opts
in (e.g. MSP430 as a Linux-gpiochip I/O-expander via the stock `gpio-pca953x` driver), the command system
coexists with no collision:
- Keep the PCA9698 command-pointer + auto-increment envelope on the GPIO banks (`0x00–0x1F`) so the stock
  driver is untouched.
- Put a reserved, **non-incrementing stream port** in the semantic window PCA9698 leaves free (0x30–0x3F;
  taken: `0x30` iface, `0x3B–0x3D` ver, `0x3E` MODE_CTRL, `0x3F` CRC — so e.g. **`0x37` CMD / `0x38` RESP**):
  writes APPEND a postcard command buffer, `Stop` triggers decode+dispatch; RESP streams the reply.
- Caveats (settle when building): CMD port = AI-off accumulate (vs PCA9698 overwrite/increment); one
  command per transaction; buffer bounded to the MCU's RAM; never point the stock driver at the stream
  port; SMBus masters (not raw I²C) cap blocks at 32 B.

## 5. Phase 2 discovery — MCU inventory × command taxonomy
**The Phase 2 kickoff: enumerate the product's MCUs/boards, what each does, and the commands each should
support — across four categories, over two access paths** (Henrik 2026-10-07). The vocabulary is common
(`stembus`); each module registers the subset it implements.

### 5.0 Source of commands — provenance (CANONICAL, verbatim, Henrik 2026-10-07)
> Production-functionality commands would come from the SoM or the PD Controller that both may be masters
> of the Stem Bus. PD controller would only be during near full shutdown of the device.
> Introspection/Inspection commands can come from Developer over USB via stem-probe(Pico firmware role) or
> PD controller. Both would be where you connect USB to debug the device.
> Additional commands would be directly to stem-probe or equivalent Yocto/M7 firmware for servicing hwd
> agent and Developer working on product features.
> Bob-929 is a prototyping/learning product, so it will have full support for Developer commands by
> default. Talki is a consumer product, so it will have much less.

Derivation — **a command exists ONLY with a named caller + purpose; else it is "capability-gold-plating
dressed up as an API." DEFER everything until truly needed.**
- **Callers by category:**
  - **Operation (production):** the **SoM** (normal), or the **PD controller** (CYPM1111) *only during near
    full shutdown*. Both are Stem-bus masters in their regime.
  - **Introspection/inspection:** a **Developer over USB via the stem-probe** (Pico firmware role), or the
    PD controller — the "USB-connected to debug the device" context.
  - **`hwd` is an external CALLER, not a separate command layer** (Henrik 2026-10-07): "hwd agent" =
    **`thepia hwd` driven by an AI agent** assisting firmware/hardware work, connecting to bench or product
    over the **single USB-C cable** (through a router, or handled by a single on-device controller board).
    So it isn't its own vocabulary — it's a caller that reaches any node over USB-C.
  - **The gateway/host nodes are themselves command TARGETS** (SoM, M7/CPU-core, the on-device controller):
    the gateway is **both a router** (forwards MCU-node commands onward) **and directly commandable**. So
    "commands to the SoM/M7/controller" are just another part of the ONE vocabulary (a section), NOT a
    scoped-out surface. (Supersedes my earlier "scope it out" — Henrik: unify, with sections.)
- **Product tier sets the default surface, via COMPILE-TIME feature gating** (Henrik 2026-10-07: "most
  likely, unless it runs a debug build") — the **same mechanism as `console`/`debug`**: dev/debug commands
  register only when their feature is on, so a production image literally doesn't contain them (zero cost,
  genuinely lean). **Bob-929** defaults to the **full** set; **Talki** defaults **lean**; but a **debug
  build of ANY product** includes the dev/debug set (Talki-debug ≠ Talki-production). Gate = build variant,
  not hard-wired to the product.
- **Doc roles:** DESIGN.md = high-level; **I2C-API.md = HOW to send commands** (transport/framing/
  addressing), *not* the command list; **COMMANDS.md = the actual commands — ONE UNIFIED vocabulary**
  (with sections), covering **what is sent internally between MCU / SoM / CPU-core (M7), or from outside
  to one of them** (Henrik 2026-10-07). Nodes (MCUs, SoM, M7, on-device controller) are all targets;
  callers are the SoM, PD controller, M7, and external agents (AI-driven `thepia hwd`, developer).

### 5.1 Command categories
| Category | What | Primary consumer |
|---|---|---|
| **Operation** | runtime product behaviour: mode (Passive/Sensing/…), power states, sensor/GPIO I/O, presence/wake, actuation | SoM (dev for test) |
| **Introspection** | identity (model/role/build-id/version), capabilities, status/health, FRAM CRC, enumeration ("what's present") — read-mostly, always available | SoM **and** dev |
| **Debugging** | logs, register dumps, self-tests, fault injection, the diag suites | dev (gate off production) |
| **Flashing** | firmware update / patch delivery — per-MCU mechanism | dev (SBW/USB) + SoM (over the protocol) |

### 5.2 Access model — ONE external USB-C port + a power-state gateway that routes (CANONICAL, Henrik 2026-10-07)
> The product ends up with a single USB-C connection that either goes to the SoM (Yocto and likely msg
> routing to M7) or Pico/PD Controller if SoM is off. So commands will need to go through that channel
> when sent from a person or outside agent.

So external access (a **person or outside agent** — dev laptop, `hwd`, a remote/CI agent over the mesh) is
a **single USB-C port**, and what it lands on depends on power state:
- **SoM ON → gateway = SoM** (Yocto; also **routes/relays to the M7** and to the MCUs over the Stem bus).
- **SoM OFF → gateway = Pico / PD controller** (which then masters the Stem bus) — i.e. the productised
  **stem-probe** role.

**Two hops, one vocabulary:** external `USB-C ↔ gateway` (USB-CDC) then `gateway ↔ target MCU`
(Stem I²C / UART). The gateway bridges framing (per-link encoding, §0) but the command *semantics* are
preserved end-to-end. Internal transports unchanged: Stem I²C control bus + per-MCU UART; the bench's
SBW/USB-DFU flashing is the same USB-C port reaching whichever gateway owns it.

**Consequence — commands need a TARGET/route:** since one port fans out to many MCUs (+ M7), an external
command must say *which node* it's for so the gateway forwards it. Open fork (shapes the protocol — §5.4):
is the gateway a **tunnel/router** (external addresses a specific MCU by role; gateway forwards the raw
command over Stem — thin, MCU owns the API) or a **translator** (external speaks the *gateway's* API;
gateway interprets and issues its own Stem commands — richer, gateway owns the API)? cros_ec leans
tunnel-ish on the SoM (each MCU a `/dev/cros_ec` device the agent targets); the Pico/PD gateway is
naturally a tunnel (forward to the addressed node). Decide when we design routing.

**Bench ≠ product — the bench is OPEN (Henrik 2026-10-07):** *"benches are much more open than any
product, so individual modules/boards can be reached over USB from the outside."* On the bench each board
has its **own direct USB** to the dev machine (MSP430 eZ-FET SBW+console, Pico USB-CDC, …) — **no gateway,
no routing**; `hwd` drives each board directly. So a board must be reachable+commandable **both** ways:
**direct over its own USB (bench)** AND **routed over the Stem bus via the gateway (product)**. Same
command vocabulary, two topologies — which maps onto §1's two faces: the **UART/console face ≈ bench-direct
per-board**, the **I²C/Stem face ≈ product-routed**. The gateway/routing model above is the PRODUCT
topology only; the bench bypasses it.

### 5.3 MCU/board inventory — PROVISIONAL (seeded from the repo + memory; **Henrik to confirm/correct**)
| MCU/board | Role | Stem addr | Flashing | Source |
|---|---|---|---|---|
| MSP430 **FR2355/FR2155** | deepsleep supervisor (production) | `0x20` | BSL (host-reflash) | I2C-API.md |
| MSP430 **FR2476/FR2475** | supervisor — DEV stand-in (same fw) | `0x21` | BSL / SBW | I2C-API.md |
| MSP430 **FR2433** | I/O-expander / passive (slave-only) | `0x22` | BSL / SBW | I2C-API.md |
| **CYPM1111** (EZ-PD PMG1-S1) | PD / power controller; Stem node; SoM-off master | `0x23` | row-based (PMG1 bootloader) | **future — fw not started, not on any bench** (Henrik 2026-10-07); its row-update is the §3 reference |
| **RP2040** | dev-bench stem-probe (SoM stand-in) | master | UF2 / picotool | MCU-BENCH.md |
| **RP2350** | product node (future) | ? | UF2 / picotool | ADR-0013 |
| **nRF52** (nRF52840 / Raytac MDBT50Q) | BLE / bluetooth networking module | ? | DFU / SWD | datasheets; "bluetooth" module |
| **i.MX8 SoM** | host: Linux A-core (Stem master) + M7 (Leaf master, tentative) | master | — | this session |
| **"lowvision"** | vision/sensing module **on a Pico** (RP2040/RP2350); keeps a history | ? | UF2 / picotool | Henrik 2026-10-07 |

### 5.4 Next (drive from the confirmed inventory, not this draft)
1. **Confirm/correct §5.3** — the authoritative MCU list + roles is Henrik's; this is a repo-seeded draft.
2. **Pick the first MCU × category** to define concretely — likely **MSP430 `deepsleep` × introspection +
   operation** (the proven node; introspection is the always-available baseline every MCU needs).
3. Enumerate that MCU's exact `Command` set (name, params, response, access-path gating), then generalise
   the shared vocabulary from the real cases — not top-down.

### 5.5 Derived command set v0 — from NAMED callers (Henrik 2026-10-07)
First real operations, each with a caller + purpose (verbatim source):
> SoM wakes up: Asks all MCUs did you wake me? what are recent events?
> SoM asks PD Controller for a power state or history report
> SoM asks lowvision(Pico) for a recent history report
> SoM updates thresholds for PD controller, lowvision, deepsleep sensors/triggers

Derived vocabulary (note: these are **cross-MCU COMMON** commands — validates one shared `Command` set):

| Command | Caller → target | Purpose | Category |
|---|---|---|---|
| **WakeAttribution** ("did you wake me?") | SoM → **all MCUs** (on wake) | which node triggered the wake | operation/introspection |
| **EventHistory** ("recent events" / "history report") | SoM → **all MCUs** (deepsleep, PD, lowvision) | read the node's recent-event log | operation/introspection |
| **PowerStateReport** | SoM → **PD controller** | current power state (+ history) | operation |
| **SetThreshold** | SoM → **PD, lowvision, deepsleep** | update a sensor/trigger threshold | operation |

**Common capabilities these REQUIRE of every participating MCU** (the real design implications):
- a **recent-event ring** — wake triggers, threshold crossings, mode changes — with **wake-source
  attribution** (so "did you wake me?" + "recent events" are answerable). Bounded (small ring; MSP430 RAM).
- **settable thresholds** for its sensors/triggers (deepsleep already has VSOM/CHARGE voltage thresholds
  in I2C-API.md §2E; PD + lowvision have their own).
- the **SoM wake-flow**: wake → poll every MCU (`WakeAttribution` + `EventHistory`) → read PD power state →
  (re)set thresholds. This sequence is the anchor the vocabulary is derived from.

**lowvision = a Pico** (RP2040/RP2350) running vision/sensing + its own history — update §5.3.

**Firmware update is IN scope** (row-based, §3) — a real category, SoM-driven.

**PD controller commands DEFERRED (Henrik 2026-10-07):** "the PD Controller isn't yet part of any bench,
so I have not started developing the firmware." So `PowerStateReport`, its shutdown-master role, and its
row-update protocol are a **future target** — but its **row-based update is the design reference** for §3.

Still DEFERRED (no named caller yet): per-pin GPIO, voltage *read* beyond thresholds, sensor proxying.

> **Implemented (2026-10-07) in `crates/stembus/src/command.rs`** (pure, wire-agnostic — the §0
> decoupling, no codec): the §5.6 data model (`WakeReason`, `EventKind`, `Event`, `EventRing<16>`), the
> §5.7 `Command` vocabulary, the `Response`/`Identity`/`StatusData` types, and the **per-module seam** — a
> `Node` trait (what a module provides: identity/status/crc/drain_events/set_mode/set_threshold) + a shared
> `execute(node, cmd, scratch) -> Response` dispatch. 9/9 host unit tests (ring, overflow, wake round-trip,
> and `execute` dispatch via a mock `Node`); compiles in budget for msp430/cortex-m. **Next:** `deepsleep`
> implements `Node` over its RegFile/Status/mode/EventRing (thin adapter), then the wire codec when §0 lands.

### 5.6 Data model — APPROVED v0 (Henrik 2026-10-07)
- **`WakeReason` = a byte enum, hardcoded in `stembus` source** (shared SoM↔MCU). The *reason* an MCU woke
  the SoM: `None=0`, then per real trigger — e.g. `SoundTrigger`, `ThresholdCross`, `GpioEdge`, `Timer`,
  `Fault`, … (fill from each MCU's actual triggers). **`WakeAttribution` = read this byte** from each node;
  the SoM scans all nodes and the non-`None` one is the culprit (+ *why*, for free).
- **Event record = `{ type, id, flag, seq }`**, common schema. `type` from a shared enum (trigger /
  threshold-cross / mode-change / fault — reuses the `WakeReason` space for trigger events); `id` = which
  trigger/threshold; `flag` = direction/aux; `seq` = **sequence number, NO clock/RTC**. Ordering =
  **drain-on-read** (SoM reads → gets all since last read → MCU clears).
- **Event ring = 16 deep**, drop-oldest + an `overflowed` bit (so the SoM knows it missed some while asleep).
- **`SetThreshold` = uniform `(threshold_id: u8, value)`**; each MCU owns its threshold-id catalog.

### 5.7 `deepsleep` command set v0 — WORKED EXAMPLE (the proven node; generalise from here)
Mostly **formalises existing regmap** (I2C-API.md) into the vocabulary + adds `EventHistory`/`WakeReason`/
`SetThreshold`. Tiers: `all` = every image; `debug` = Bob-929 default / Talki-debug build only (§5.0).

| Command | Dir | Params | Response | Category | Tier |
|---|---|---|---|---|---|
| `Identify` | R | — | iface magic `0xD0`, model id, role, build-id, fw version | introspection | all |
| `Status` | R | — | status flags, fault, dev-count, current mode, **`WakeReason`** | introspection | all |
| `FramCrc` | R (windowed) | sector idx | per-sector CRC16 (ADR-0010) | introspection | all |
| `EventHistory` | R (drain) | — | ≤16 × `{type,id,flag,seq}` + `overflowed`; clears ring | introspection/operation | all |
| `SetMode` | W | mode (Passive/Sensing) | ack + current mode | operation | all |
| `SetThreshold` | W | `threshold_id`, value | ack | operation | all (deepsleep ids: VSOM, CHARGE, … from I2C-API §2E) |
| `Scan` / `Dump` / `SelfTest` | R/action | — | enumerate / reg dump / POST result | debugging | debug |
| `FwUpdate` | W (rows) | enter → `write_row(idx,bytes)` → verify → activate | per-row ack/CRC | flashing | SoM + dev |

**Open specifics to fill as we implement (not blocking the shape):** the concrete `WakeReason` enum values
(from deepsleep's real triggers — eCOMP sound/analog, GPIO edges, the voltage thresholds), the
`SetThreshold` value width (I2C-API §2E has 16-bit voltage thresholds), and the `FwUpdate` row size/verify
(follows the PD/PMG1 row scheme, §3).

## References
- [`I2C-API.md`](../I2C-API.md) — PCA9698 register map, extended/debug window, **role addressing** (transport contract).
- [DEEPSLEEP-EXTRACTION.md](DEEPSLEEP-EXTRACTION.md) — the MSP430 `deepsleep` implementation + the `next_event` seam (Phase 2).
- [ADR-0012](adr/0012-stembus-shared-contract-crate.md) — `stembus` as the one shared wire contract.
- `references/firmware-architecture.md` — transport↔dispatch↔state layering; I²C stream facts (UM10204).
