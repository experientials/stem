# Command system across MCUs

**Status:** DESIGN (the first implementation is Phase 2 of [DEEPSLEEP-EXTRACTION.md](DEEPSLEEP-EXTRACTION.md),
on the MSP430 `deepsleep` module). This doc is the **cross-MCU overview** — the common command interface
every MCU module (MSP430 `deepsleep`, future `lowvision`, `bluetooth`, …) exposes to the SoM/M7. Register
map + role addressing live in [`I2C-API.md`](../I2C-API.md) (the transport contract); this is the command
*model* layered on it.

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

## 3. Firmware patches ride the control bus
A firmware patch is a large **streamed** payload (§1 I²C framing; I²C allows an unrestricted byte count
per transfer — `references/firmware-architecture.md` / NXP UM10204 §3.1.4). The command vocabulary models
"deliver firmware patch (streamed)"; **each module decides how to apply it**: MSP430 FRAM can't A/B
self-update, so its update command hands the image to a host-driven BSL/STEM reflash (memory
`msp-fw-ota-host-root-of-trust`); other MCUs may self-apply.

## 4. Coexistence with PCA9698-emulation (optional, per-module)
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

## 5. Per-module command sets
The vocabulary is common; each module registers the subset it implements. To be defined in Phase 2:
- **`deepsleep` (MSP430):** mode (Passive/Sensing), status/identity, sleep/presence, firmware-patch; rides
  the proven `bsp::stem_i2c::next_event()` seam (accumulate `Rx` → `postcard`-decode on `Stop`) +
  `deepsleep`'s `execute`. See DEEPSLEEP-EXTRACTION.md §2/§A.
- **`lowvision`, `bluetooth` (future):** their own `execute` over the same vocabulary.

## References
- [`I2C-API.md`](../I2C-API.md) — PCA9698 register map, extended/debug window, **role addressing** (transport contract).
- [DEEPSLEEP-EXTRACTION.md](DEEPSLEEP-EXTRACTION.md) — the MSP430 `deepsleep` implementation + the `next_event` seam (Phase 2).
- [ADR-0012](adr/0012-stembus-shared-contract-crate.md) — `stembus` as the one shared wire contract.
- `references/firmware-architecture.md` — transport↔dispatch↔state layering; I²C stream facts (UM10204).
