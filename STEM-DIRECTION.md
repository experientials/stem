# Stem — architecture & direction

> **Captured verbatim from Henrik (2026-09-25).** This is the intended **leading description for a
> future version of the README**, plus the fuller Stem architecture spec. Forward-looking: the roles
> are being filled **slowly / incrementally**. This file is canonical for the Stem direction — when
> the README lead is rewritten, draw from the verbatim blocks below (polish grammar, keep the intent).
> Ties to project memory `stem-nervous-system-vision`, `prod-fw-multimodel-dualbus`,
> `msp-fw-ota-host-root-of-trust`.

## Proposed leading README description (verbatim)

The stem is composed of mcus that maintains the system cohesion even when inactive. They take on
specific roles that make up the overall functionality. A SoM/SoC is the default master on the stem
bus and the work horse for the active system.

Defined roles for mcus are:
- **Sleep supervisor**
- **Power supply**
- **Inner vision** (nfc and super low res ir ml vision)
- **Radio receiver** (ble/thread)

The msp-fw will become the **stem repo** with firmware for all stem mcus. We need proper **HAL** to
manage the exact mcu models.

## Role → MCU (current fills)

| Role | Production chip | Core | Dev board |
|---|---|---|---|
| **Sleep supervisor** (a.k.a. **Detector**) | MSP430 (FR2355 canonical; FR2433 baseline) | MSP430 | FR2476 / FR2355 LaunchPad |
| **Power supply** | **CYPM1111** | Cortex-**M0** | **CY7111** |
| **Inner vision** (NFC + super-low-res IR ML vision) | **RP2350** | — | — |
| **Radio receiver** (BLE / Thread) | **nRF52 family** | — | nRF52 dongle |

*Sleep supervisor ≡ **Detector** (Henrik, 2026-09-25): it uses sensors to determine what/how the device
needs to respond. "Detector" names the function (sense → decide response); "sleep supervisor" names the
when (active while the system sleeps).*

Each role has a **production chip** and (where applicable) a **dev board** used to develop it — the
same split as MSP430's FR2476 LaunchPad (dev) → FR2433/FR2355 (production). For the power role that's
**CY7111 (dev board) → CYPM1111 (the actual chip we would use)**.

Not starting the non-MSP430 parts yet — these are the current assignments. "Proper HAL to manage the
exact mcu models" is what lets one Stem repo hold firmware per role across these different cores/
vendors; it is a **later, incremental** step (extract it as the MCUs land, not ahead of them — the
near-term MSP430 step is FR2355 via a feature-gated PAC alias, see
[diag/FR2355-SCOPE.md](diag/FR2355-SCOPE.md)).

### MSP430 roles — FR2355 sleep-supervisor, FR2433 general I/O expander

FR2355 fills the **sleep-supervisor** role. A second MSP430 role — **"some sort of general I/O expander
on a daughterboard"** (Henrik, 2026-09-25, **not yet clearly defined**) — is an **even better fit for
the FR2433**: single-I²C, slave-only, already the **Stembus Expander** (a pure I/O-extender the SoM
drives), whereas FR2355's **2× I²C** (sensor master + SoM slave) is what suits the *autonomous*
sleep-supervisor. These map onto the prod firmware's existing behaviours — **Sensing** ≈ autonomous
supervisor (FR2355) and **Passive** ≈ pure I/O-extender/slave (FR2433 expander); see
[prod/DESIGN.md](prod/DESIGN.md). The daughterboard I/O-expander would use the semantic (topic,
topic-pin) model above. **To define:** what the daughterboard I/O-expander role adds beyond the
existing FR2433 Stembus Expander.

## Bench/dev host — RP2040 deep bus-inspection module (verbatim · Henrik 2026-09-30)

> **New Stem member, the first non-MSP430 core to land.** Distinct from the **RP2350** "Inner vision"
> product node in the role table above — this **RP2040** is a **master/host** (a dev SoM stand-in),
> not a slave node.

Verbatim (Henrik 2026-09-30):

> *"I want to add an RP2040 profile for putting a dev-time firmware on it via USB. I want it to connect
> to both Leaf and Stem I2C and possibly also SPI. This isn't the role/variant that we might put in
> products but more of a dev-time that can be used to deep inspect internal busses. It would have access
> to the full stem bus including PMIC/boot pins for UCM i.MX 8M Plus, and other internal pins. This might
> be a dev/debug module that can be slotted into products for deep tests. It will start out as something
> for bench testing MCUs and sensors."*

And the recurring structural principle (verbatim · Henrik 2026-09-30):

> *"I again want a prod base that can grow into something used in the products."*

And the purpose — an M7 stand-in for automated testing (verbatim · Henrik 2026-09-30):

> *"This new firmware will allow you to run ad-hoc and soak tests fully automated without an attached
> UCM board directly from a Mac or Linux machine duplicating what the UCM SoM M7 will be able to do
> mastering the system."*

**Derived (not verbatim), for planning — reconciles with the `prod`-baseline principle above:** even
though the RP2040's *initial use* is a dev/bench bus inspector, its firmware is structured as a proper
**prod base** (foundation) with the dev/inspection capability layered on top — the same "prod is the
normal baseline; the test/dev harness is a build axis on top" stack proven for MSP430, so it's
product-quality and can grow (including possibly shipping as an in-product slotted debug module). The
RP2040 is the **master** side of the SAME chip-agnostic contracts the MSP430 nodes serve as slaves
(`regmap`, the stembus protocol, the semantic Stem/Leaf bus roles) — so it reuses those contracts, but
needs its **own** `board`/`hal` layer (Cortex-M0+, `thumbv6m`, a Rust RP2040 HAL) since it shares no
silicon with MSP430. This is the moment "proper HAL to manage the exact mcu models" (above) starts to
be real — extract cross-core abstractions **as this second core lands**, not ahead of it.

**The defining purpose = a UCM SoM M7 stand-in for host-driven automated testing.** The RP2040 masters
the stem system exactly as the i.MX 8M Plus's real-time **M7** core will, so **fully-automated ad-hoc +
soak tests run from a Mac/Linux machine (and CI) with NO UCM attached** — and, because it duplicates
the M7's mastering role, the test behaviour transfers to the real M7. This makes the stem system
testable in CI without a UCM in the loop (ties to the Big Bob bench / CI-hardware objective). Design
consequence: the host's capability set (`host-core`) should mirror the **M7's intended stembus-master
API**, so the same test harness can target the RP2040 (bench/CI) or the real M7 (on a UCM). See the
design doc [docs/RP2040-BENCH-HOST.md](docs/RP2040-BENCH-HOST.md).

### Pico as a product MCU + in-system probe (verbatim · Henrik 2026-10-07)

**Standing context (Henrik 2026-10-07): the Pico (RP2040/RP2350) has been a possible MCU on all
products so far — just not sufficiently spelled out.** So the point below is NOT specific to one named
module: it is a property of *the Pico wherever it lands in a product*. `lowvision` is a **recent name**
Henrik introduced (likely the **"Inner vision" RP2350 role** in the role table above — confirm the
relationship), and it is the current concrete example, not the sole vehicle.

**What `lowvision` is (role meaning, Henrik 2026-10-07):** a **(super-low-res) IR camera** — in the
**mouth of Talki**, or hidden elsewhere — that **runs while the device sleeps** to provide **monitoring
beyond the Bosch IR-range sensor**. So it is a **richer sensing tier above the always-on range sensor**:
the sleep-supervisor/Detector (FR2355) watches at µA with cheap presence/range sensors (incl. the
IR-range ToF sensor), and `lowvision` adds low-res IR *vision* that can also run during sleep — monitoring
the range sensor alone can't do. (This is the "super-low-res IR ML vision" of the role table's **"Inner
vision" RP2350** entry — confirm whether `lowvision` renames that whole role or just its vision half.)

**Optional observer, not base (Henrik 2026-10-07):** lowvision is *"an optional observer on some products,
not a required base functionality."* It is therefore **not bound by the µA sleep budget** that governs the
required base (the FR2355 Detector watching for weeks in a drawer): an IR camera + ML running during sleep
costs mA–tens-of-mA, and that cost is accepted by the products that *choose* to include it — not forced on
every product. Architecturally this = a **feature-gated / per-product build axis** (LTO-strippable, opt-in),
the same opt-in model as the `devices` drivers and the probe/programmer capability. Any
escalation-vs-duty-cycle power strategy for *when* the observer runs is a **per-product concern**, not a
base-architecture decision.

> **Range sensor = the VL53L0X** ([crates/devices/src/vl53l0x.rs](crates/devices/src/vl53l0x.rs), Henrik
> pointed here 2026-10-07). NOTE/CORRECTION: Henrik called it "the Bosch IR-range sensor", but the
> VL53L0X is an **STMicroelectronics** ToF IR ranging part (I²C 0x29, MODEL_ID 0xEE) — vendor to confirm
> (ToF ranging is not a Bosch Sensortec line; likely a label mix-up). Also currently **identity-only** in
> the devices crate — ranging is a tracked TODO (needs ST's init sequence, not in `datasheets/`).

This is the concrete realization of the "dev/debug module that can be slotted into products for deep
tests" + "prod base that can grow into something used in the products" above — the slotted-debug-module
is not a separate SKU, it rides a Pico that is already in the product:

> lowvision runs on a Pico and is connected to the Stem bus. It has an USB connection, so it would be
> possible to expose it on T-USB for Bob 929. It should have space in flash for additional
> diag/programming/JTAG-access to MCUs and the SoM. That is essentially a stem-probe extension on top of
> the prod firmware

**Implications (derived, for planning — confirm):**
- A **Pico in a product** is a Stem-bus node (RP2350 product node, COMMANDS.md §5.3) running its
  product role (e.g. vision/sensing + history), AND can carry the host/probe capability (`stem-probe`)
  layered on **as a feature on top of its prod firmware** — same "prod base; dev/inspection as a build
  axis on top" stack as the MSP430 nodes.
- Because the Pico has **USB**, that probe surface is reachable in-product over **T-USB on Bob 929** —
  the bench `stem-probe` console/command path becomes a product-time deep-inspection + programming path.
- "diag/programming/JTAG-access to MCUs and the SoM" = the Pico additionally acts as an **in-system
  programmer/debug probe** for its peers: SBW/BSL to the MSP430 nodes (cf. `msp-fw-ota-host-root-of-trust`
  — this makes the Pico a second host root-of-trust alongside the M7) and JTAG/SWD to the SoM. This is
  **new** capability not in today's stem-probe (bus inspect only); needs flash headroom budgeted on the
  Pico, and the SBW/JTAG lines physically routed to it (not just Stem I²C).
- This resolves the "enabled in prod" question for the shared host/probe layer: the probe logic belongs
  in **shared crates** consumed by (a) the bench `stem-probe` binary and (b) any **Pico product firmware**
  — so the in-product probe and the bench probe are one implementation. See the host/probe shared-layer
  proposal (pending ADR).

### Structural direction across MCUs (verbatim · Henrik 2026-09-30)

> 1. *"special debug firmware makes sense in a special location. prod crates do not"*
> 2. *"I start with RP2040, but will surely use RP2350 in the future"*
> 3. *"I will likely use Arducam TinyML board with a camera. This could be a dev base for something we
>    put in a product"*
> 4. *"Common crates should be common. We don't want duplication of crates that can be common. We don't
>    want forced commons that is full of if statements"*
> 5. *"You want to pick toolchain for building carefully so we can also add in nRF52 modules in the
>    future"*

**Derived synthesis (not verbatim) — the structure these imply:**
- **The shared surface is THIN, and consistency comes from COMMON TESTS, not shared code (4; Henrik
  2026-09-30: *"I can imagine the shared surface between the major families to be quite limited"* ·
  *"organise it as common tests across islands but no shared code. The bit that is common could be just
  duplicated, but kept in check with common tests"*).** The common bit — the **protocol/contract**
  (regmap + stembus line semantics) — is **small** and crosses language/runtime boundaries (Rust-sync
  MSP430, Rust-async RP/nRF, **C** CYPM1111), so a shared Rust crate can't span it anyway (the C island
  would port/duplicate regardless). Chosen model:
  - **One canonical spec** = [I2C-API.md](I2C-API.md) + the PCA9698 tables (the single source of truth;
    `regmap.rs` is already *derived* from it — keep it that way).
  - **Each island implements it itself** (its own language/runtime) — **duplication by design**, not a
    forced common crate.
  - **A common conformance suite keeps them honest** — and we already have the tester: the **RP2040
    host** masters every node, so the "common tests across islands" ARE the host's ad-hoc/soak suite
    driving each island's slave surface against the spec, plus shared **test vectors** (data, not code)
    an island's own unit tests can run. CI gates on conformance. (Tradeoff: duplication drifts if the
    suite misses a case — so "passes the conformance suite" *is* the definition of conformant.)
  - **Same-language device-driver overlap** (e.g. a sensor the Detector and the host both read) MAY
    still be a shared Rust crate if a real overlap lands, but default to the same duplicate-plus-test
    discipline; extract-on-proven-overlap still governs, just with a higher bar for shared *code*.

  Everything else is **per family**: runtime/framework (embassy async vs MSP430 sync-poll vs CYPM1111
  C), HAL, board/pins, feature logic. Even `crates/sched` is **island-side** (MSP430) — embassy nodes
  use embassy's executor. **Principle: the spec + the conformance suite are the common denominator;
  shared code is the exception, never the mechanism for consistency.**

### Public master-side client — the `stembus` client for the M7 (Zephyr) & the host (verbatim · Henrik 2026-09-30)

> *"I would also imagine that stem repo would include a public crate/lib that can be used by the repo
> building M7 Zephyr based firmware."*

**Derived — this sharpens the master/slave split (the ONE place shared code is warranted):**
- **Slave side (the nodes):** many, constrained, divergent (MSP430 · CYPM1111 · …) → **duplicate +
  conformance-test** (above). No shared slave code.
- **Master side (the SoM/**M7** · the RP2040 stand-in · the Mac/Linux CI harness):** few, capable, and
  they *want* parity — so a **shared, public master client** earns its keep. The stem repo **publishes**
  a stable, versioned **`stembus` client** (scan nodes, read/write regmap, sequence control lines) — the
  portable master protocol logic, abstracted over a **transport**: local I²C for the M7/host;
  remote-over-USB for the Mac/Linux harness (talking *through* the RP2040 host).
- **Bindings (depends on the M7 repo's language, decided when it starts):** a **Rust crate** (RP2040
  host + CI harness + any Rust-on-Zephyr) and a **C library / Zephyr module** for mainstream C Zephyr.
  Both derive from the canonical spec and are kept honest by the **same conformance suite** — so this
  shared code does NOT reintroduce drift.
- **Payoff + discipline:** the RP2040 host's `host-core` **is** this client (concrete transport = its
  own I²C); the M7 uses the same client → "the RP2040 duplicates what the M7 does" becomes *literal*
  and the ad-hoc/soak tests transfer to the real M7. This is the one crate with **public-API
  discipline** (semver, docs, stability) — an **external repo depends on it**, unlike internal firmware.
- **Per-arch HAL/board crates stay SEPARATE, never cfg-merged across unrelated silicon (2,4):** MSP430
  `bsp` (msp430-hal) · a `rp-bsp` covering **RP2040 + RP2350** (a chip axis *within* the RP family,
  the same idea as the MSP430 family axis) · a future `nrf-bsp`. Each exposes the semantic bus roles
  (`stem_i2c`/`leaf_i2c`/`console`/`spi`) + embedded-hal, so the common layer stays chip-agnostic. This
  is exactly the "common is common; forced if-riddled commons are not" line: share the contract, keep
  the silicon separate.
- **Toolchain/framework = `embassy` for the parts it supports (2,5):** the one async framework
  spanning **RP2040 / RP2350 / nRF52** (embassy-rp, embassy-nrf), unified by embedded-hal(-async) —
  what (5) "add nRF52 later" + (4) "don't duplicate" jointly point to (one framework, not per-chip
  ecosystems). Targets: `thumbv6m` (RP2040), `thumbv8m.main-eabihf` (RP2350-ARM), `thumbv7em-eabihf`
  (nRF52). Async is a real tradeoff vs the MSP430 polled style, accepted for the ARM-side unification.
- **TWO non-embassy islands — MSP430 and the CYPM1111 (Power role) (2,4):** `embassy` is NOT a
  universal target. **MSP430** (msp430-hal) is one island; the **CYPM1111** (Power-supply role;
  Cortex-**M0**, `thumbv6m`, dev board CY7111) is another — a Cypress/Infineon part (PMG1/PSoC-class,
  ModusToolbox/C-first) that embassy does not cover (whether it has any Rust HAL at all = **verify when
  the Power role starts**; may be C-only). Consequence: **the sharing boundary sits BELOW embassy** —
  on `embedded-hal` + the **pure contract crate** — so the common crates (`crates/stembus`,
  `crates/devices`) **must not depend on embassy**, letting both islands share them. embassy is a
  framework choice for RP/nRF, never the common denominator.
- **The CYPM1111 is a planned stem NODE, and it forces the contract to be language-neutral (2,4):**
  the Power role is a node on the stembus — it serves the **same `regmap`/stembus contract as a slave**
  (the triplet: `stem_i2c` slave, its own `leaf_i2c`, `console`), so the RP2040 host + the shared
  contract reach it exactly like an MSP430 node. But because it's likely **C (ModusToolbox)**, the
  contract **cannot be Rust-only**: its **canonical source stays language-neutral** — the register map
  is already *derived* from [I2C-API.md](I2C-API.md) + the PCA9698 spec (see `prod/src/regmap.rs`'s
  header), so `crates/stembus` (Rust) and a generated **C header** are two *bindings* of the one spec.
  Sharing is therefore at the **protocol level** across all four groups: embassy-Rust nodes (RP/nRF),
  the MSP430 island, and the CYPM1111 C island. (This also anchors the field-update scheme — the
  CYPM1111's **row-based flash** is the granule, see §"Field-updatable firmware".)
- **Debug firmware lives in a SPECIAL location; prod crates do not (1):** the RP bench/bus-inspector
  (M7-stand-in) **binary** is segregated (e.g. a top-level `debug/` or `bench/`), built **on** the
  normal prod + common crates — but prod/product-capable crates never live in the debug area.
- **Boards incl. Arducam (3):** `rp-bsp` models boards as **(chip, module)** — a Pico-class bench host
  **and** the **Arducam TinyML + camera** board, the latter a dev base for a product (the **Inner
  vision** role, RP2350 in the table above). Same (chip, module, node-role) granularity as MSP430.

## Field-updatable firmware & integrity — FUTURE MILESTONE (verbatim)

> **Deferred.** Henrik (2026-09-25): *"I will return to this topic in a future milestone."* Not now —
> captured so it's not lost.

The firmware build includes **checksums** and may be **aligned to row boundaries** to allow partial
updates. When booting the firmware calculates the checksums and compare with expected. If it fails
the loading is **restricted to known working parts**.

Goal: **Stem firmware field-updatable with check-summed incremental updates.** The **CYPM1111 has row
logic** (row-based flash) that we would use for the **incremental field updates** — this is why images
align to **row boundaries** (a row is the update granule). This applies across the Stem MCUs, not just
CYPM1111.

**Decision (Henrik, 2026-09-25): A/B is NOT the approach** — common, but concluded against it. *"We
will make a custom scheme if need be, but attempt to ground any logic in what has been done before."*
So: a **custom scheme is acceptable** where warranted; ground it in **proven OSS solutions and
principles** as much as possible (checksum/signature verification, verify-then-boot, known-good
fallback) **without adopting A/B**. Note the model here is **row-granular incremental** updates (patch
rows, via the CYPM1111 row logic) — inherently different from A/B's whole-image swap.

## The full stembus (verbatim)

The full stembus for the som to communicate with mcus, is **i2c, uart, reset, bootsel, wakeup,
sleepy**.

It allows the som to connect to a specific mcu and **program or monitor it over uart or swd**. Some
lines are guarded by a **directional level shifter that can be switched on/off via the i2c**.

## Stem & Leaf I²C buses (verbatim · Henrik 2026-09-29)

There are **two I²C buses**, named by role, not by silicon instance:

- **Stem I²C** — the SoM↔MCU interconnect (part of the stembus above). The MCU is a **slave/node**
  on it; the SoM is master.
- **Leaf I²C** — the bus carrying **leaf nodes**. Henrik: *"Leaf is a better name for the bus with
  sensors as it can also have LCD and other non-MCU nodes."* So it is **not** just "sensor I²C":
  sensors, LCDs, and any other **non-MCU** peripheral hang off it. The MCU is **master** here.

**The allocation of Stem vs Leaf to a physical eUSCI instance + pins is chip/PAC-specific.** So code
must not reference a raw peripheral like `e_usci_b0`; the **semantic bus (Stem/Leaf) is defined per
chip in the `board` layer** and exposed by role (e.g. `board::leaf_i2c()` / `board::stem_i2c()`),
exactly like the backchannel UART instance. The generated svd2rust PAC stays pure silicon
(`e_usci_b0`/`b1`); the *role→instance* mapping lives in `board`. On a 2-eUSCI-B part (e.g. FR2355:
UCB0 + UCB1) one becomes Leaf-master, one Stem-slave; on a slave-only part (FR2433) there is only the
Stem side. See [[per-sensor-module-convention]], [[prod-fw-multimodel-dualbus]].

### Granularity of `board` (decided · Henrik 2026-09-29)

**`board` = the module / PCB** — not the MCU (below it) and not the assembled product (above it).
Rationale, verbatim: *"We generally don't have more than one MCU per module. If we were to have two,
it would most likely be very different ones."* So a module carries **one MCU**, and `board` = that
module. Vocabulary: *"Modules are boards of sorts, or built around one. A sub-module can also be
thought of as a daughter board."* — module ≈ board; a **sub-module = a daughterboard**; all one layer.

Three layers: **chip/MCU** (silicon: PAC, clock trim, timer, RAM, eUSCI existence + pin options) →
**board/module** (which pins are wired; the semantic peripheral map: `leaf_i2c`/`stem_i2c`/
`console_uart`/`spi`) → **product/assembly** (which modules + MCU roles compose the device — the stem
topology). Because a module is ~1:1 with its MCU, the chip and board facts are **fused in one
`board::<chip>_<module>` file** (e.g. `fr2355_launchpad`) rather than split into separate `chip`/
`board` layers — the split is only forced if one MCU ever lands on ≥2 modules. The cargo feature names
the **chip** (the PAC axis); the module is named in the file. See [[stem-nervous-system-vision]].

**This generalizes to the whole node (Henrik 2026-09-29): `board` is the semantic peripheral map —
2× I²C, UART, SPI are all semantic.** Every bus is named by role and bound per-chip to its instance +
pins + mode: Stem-I²C (slave), Leaf-I²C (master), the backchannel/Stem UART, and SPI (e.g. an LCD or
non-MCU node). On MSP430 the allocation is constrained by silicon: **eUSCI_A = UART or SPI**,
**eUSCI_B = SPI or I²C** (SLAU445/SLAU144), and A/B counts + pinouts differ per part — which is exactly
why the role→instance map must be per-chip in `board`, and the layers above (`hal`/drivers) stay
role-agnostic.

**Every MCU node carries the same triplet (Henrik 2026-09-29):** `stem_i2c` (uplink to the SoM — the
MCU as **slave**), `leaf_i2c` (its **own** sensor/LCD/non-MCU subtree — the MCU as **master**), and
`console_uart` (debug/programming link). `stem_i2c` is needed **in addition to** `leaf_i2c`, not
instead of it.

**Multi-MCU boards need the node role as a disambiguator.** If a module has >1 MCU, `console_uart`
and `stem_i2c` exist **once per MCU** (both connect to the SoM), so the bus role alone is not unique —
you must qualify by **which MCU**, i.e. its **system role** (Detector, Expander, …). So the full
identity is `<node-role>::<bus-role>` (e.g. `detector.stem_i2c`, `expander.console_uart`), and the
**`board` granularity refines to (chip, module, node-role)** — the role is implicit on a single-MCU
module, explicit when >1. This is the same node identity the stembus uses (source chip / uid, above).
Physical consequence: **Stem-I²C is one shared bus with a distinct slave address per node**;
**console_uart is a separate link per node**; **leaf_i2c is a separate master subtree per node** (never
two masters on one bus). From a node's own firmware the accessors (`board::stem_i2c()` etc.) always
mean *this* node — the role qualifier lives in the system/topology layer and the SoM's addressing.

## diag and prod are variants, not separate codebases (verbatim · Henrik 2026-09-29)

*"I need both diag and prod to follow the same patterns and use the same abstractions. They are not
conceptually different code bases. Rather they should be variants in a dimension. Features of prod are
features of diag. Features in prod are API centric with occasional timer triggered activity. Features
in diag are tested/used one at a time to test presence, correctness, load/stress."*

So **diag and prod are two variants along one dimension**, over a **shared** feature set + abstractions,
not two codebases:
- A **feature** (a sensor/device, a bus, a subsystem) is implemented **once**, on the shared
  abstractions (`board` semantic peripheral map, `hal`, `crates/devices`). See
  [[per-sensor-module-convention]] — one module per sensor, both faces, no fork.
- **prod** = the **API-centric** variant: features driven by the stembus **API** (the SoM as master)
  with **occasional timer-triggered** activity. An event/API loop.
- **diag** = the **test** variant: the **same** features exercised **one at a time** to check
  **presence, correctness, load/stress**. A sequential test harness.
- **Feature containment:** every prod feature is a diag feature (prod ⊆ diag); diag additionally holds
  test-only modes (stress/soak). The *implementations* are shared; only the **harness** differs
  (API+timer vs one-at-a-time test) — so diag/prod become thin harnesses over shared crates
  (`board`, `hal`, `devices`, feature modules), not parallel trees.

Implication for the current work: the `board`/`hal` abstractions being built in `diag/` must land in a
**shared crate** both consume — this is exactly what `hal.rs` already anticipates ("when the board
crate lands, this type moves there"). The dual-target (chip) dimension and this diag/prod (harness)
dimension are orthogonal axes over the same shared code.

**prod is a LAYER diag builds ON (Henrik 2026-09-29):** *"I would also include prod code in diag so it
can be treated as a layer to build on. We would want to test the maximum amount of prod code in the
diag logic."* So the relationship is a **stack, not two siblings**: prod's real logic (features, API
handlers, timer behaviour) is factored into a **prod-core library**; the **prod binary** is a thin main
(the API+timer event loop) over it; and **diag depends on prod-core** and drives its features
one-at-a-time (+ test-only stress/soak). The point is **coverage**: diag exercises the **actual prod
code**, not a parallel reimplementation, so the maximum prod surface is under test.
- Stack: `bsp` (board/hal) + `devices` → **prod-core** (feature/API/timer logic) → { **prod bin** =
  thin event loop · **diag** = test harness driving prod-core }.
- **Requirement this imposes:** prod-core features/API-handlers must be **individually invocable** (a
  callable capability), not buried in the event loop, so diag can drive one at a time and so prod can
  call them from API/timer. (Same "expose the capability, not a forced `run()`" point as above.)
- **Size:** the FR2433 15 KB gate applies to the **prod binary**; diag (32 KB budget) carries prod-core
  + test harness and is expected to be larger — no conflict.

### MSP430 is not special; prod/diag/bench are chip-neutral (verbatim · Henrik 2026-10-07)

Sharpens the "orthogonal to the chip axis" line above — the top-level trees are **roles/variants**, not
chips, and **MSP430 is one island of several**, not a privileged base:

> MSP430 should not have a special status in the repo, it is one of a handful MCUs we compile Stem
> firmware for

> debug/ -> bench/ is ok, but why would diag/ not also be bench/? Why should we consider diag/ to be
> MSP430 only? prod/ should not be MSP430 only

> We might have to slot it on top of Zephyr in order to support existing codebases, but M7 is part of the
> product stem, so it belongs here

**Derived synthesis (not verbatim) — the chip-neutral layout these imply:**
- `prod/` and `diag/` are the **variant pair** (prod ⊆ diag; prod-core → {prod bin, diag}), **per node
  chip**, NOT MSP430-only. MSP430 is just the only chip whose node roles are *filled* today; RP2350
  (lowvision), nRF52 (bluetooth), CYPM1111 (power) are node chips too. The firmware trees must be
  organised so no chip is the implicit default — each node chip is a fill under the same variant axis.
- `debug/` → **`bench/`** (rename accepted): the host / M7-stand-in binary.
- **`diag/` ≠ `bench/` because of ROLE, not chip:** `diag` is **node (device-under-test) self-test
  firmware**, a build variant of what ships on each node chip (coupled to `prod` via prod-core). `bench/`
  is the **host/master** side — the opposite end of the bus (the thing that *drives* the DUTs). Both are
  "test", but node-DUT-firmware vs test-driver are different roles (the §142 master/slave split). *(If
  Henrik instead wants a single diagnostics umbrella spanning both ends, that's a deliberate merge to
  decide — flagged, not assumed.)*
- **The M7 belongs in this repo.** It is product stem (the real master), so the **shared master client**
  (the `stembus` client / host-core, §142) is its home — possibly **slotted on top of Zephyr** to reuse
  existing codebases. That client is the one consumed by M7 (product) + the RP probe + Mac/Linux CI alike.

### stem-probe = host-side diagnostics, not a "bench" (verbatim · Henrik 2026-10-07)

Refines the "debug firmware → special location" framing (§"Public master-side client", point 1) and the
`bench/` idea above — the probe is **not inherently a bench tool**:

> I still don't understand why bench/stem-probe is so special. What if I create a product module with
> stem-probe firmware. That is no longer a bench, but a module to plug into a product. It is used for
> diagnosis, just like diag firmware loaded on an MCU to give extra diagnostic functions

**Derived synthesis (not verbatim) — the 2×2 this implies (role × variant):**

| | node (slave) | host (master) |
|---|---|---|
| **prod** | operational node fw (deepsleep…) | operational mastering (the product M7 master) |
| **diag** | node self-test (today's `diag/`) | deep inspect / sniff / probe / JTAG (today's `stem-probe`) |

- **stem-probe = the host/master-role analog of node `diag`** — extra diagnostic functions, from the
  master end. `prod ⊆ diag` holds on BOTH sides; shared cores = feature-crates (node) / `stembus-client`
  (host).
- **"Bench" is a DEPLOYMENT CONTEXT (dev board / CI), not a code identity.** The same host-diag firmware
  can ship as a **product diagnostic module** on a product Pico (reachable over T-USB). So there is **no
  special `bench/` tree** for the firmware.
- **The "module to plug into a product" IS a crate:** the probe/inspection capability is a **pluggable
  diagnostic module** built on `stembus-client`; the standalone probe binary AND a product image both
  *compose* it.
- **Only genuinely bench-only artifact:** the **PC-side (std) CLI / soak harness** that drives the probe
  remotely over USB from a Mac/Linux/CI host. That is dev *tooling*, not shippable firmware — it alone
  wants a `tools/`-type home.
- **Net on §142:** the "special debug location" narrows to genuinely-debug-only artifacts (the PC
  harness); its crate-vs-binary rule still holds, but stem-probe's *firmware* is reclassified out of
  "debug" into prod-capable **host-diag**.

**Named + scoped since (2026-10-06):** this "prod-core library" is the **`deepsleep` crate** — the MSP430
deepsleep-supervisor module (per-product-module crates, siblings `lowvision`/`bluetooth` on their own
MCUs; *not* a generic `prod-core`/`node`). The slave **driver → `bsp/src/stem_i2c.rs`**, dispatch +
semantic command/state → `deepsleep`; execution is **interrupt-driven + LPM** via the async `sched`
executor. Full phased plan: **[docs/DEEPSLEEP-EXTRACTION.md](docs/DEEPSLEEP-EXTRACTION.md)**.

**Sharper still (Henrik 2026-09-29):** *"I would not treat any/much of the prod code as a distinct
codebase but rather diag as the special case on top of 'normal'."* · *"prod should just be seen as a
build target/axis."* So **prod is the "normal" baseline**, and **diag is a special-case build axis on
top of it** — prod/diag is a **build axis** (like the chip axis), NOT two separate codebases. End
state: **one codebase, two orthogonal build axes** (chip × prod/diag), over shared crates. The
foundation is `bsp` (board/hal — `crates/bsp`, already scaffolded) + `devices`; "normal" (prod)
behaviour is the baseline; diag layers the test harness on top. **Immediate blocker:** `board`/`hal`
wrongly live inside `diag/` — they move into `bsp` first, and both prod and diag build on it (de-forking
prod's parallel `hal.rs`/`i2c.rs`).

**Same-boot + master-commanded mode (verbatim · Henrik 2026-10-06):** *"It is conceptually built on
top. I understand that prod is the root dir for the prod images and diag for the diag images. The
implementation should be using common crates, so the functionality present in prod firmware is also
running in diag, or can be enabled to run by flipping a register. We should be able to install diag
firmware and test prod functionality with some diag capability run on top. Obviously diag has extended
the POST and tests, but it would probably be good if diag essentially boots the same and waits a second
before running the diag suites automatically. This would allow the Pico/M7 to detect that it rebooted
and instruct what it should do via Stem I2C or UART."*

Derived requirements (from the verbatim above):
- **Common crates, prod functionality live in diag:** prod's functionality runs in diag too (shared
  crates), or **can be enabled at runtime by flipping a register** — not a compile-time fork. Installing
  the **diag** image must let you **test prod functionality**, with diag capability layered on top.
- **Identical boot:** diag **boots the same as prod** (same init sequence, same early Stem-slave
  presence), then **waits ~1 s before auto-running the diag suites**. The default-after-timeout is the
  diag suite run (for the diag image); the grace window is the hook.
- **Master-commanded mode:** during that window the **Pico/M7 master detects the reboot** (the node
  reappears on the bus) and **instructs what to do via Stem I²C or UART** — i.e. the "flip a register"
  control is the Stem regmap mode/control register the master writes, before the auto-run fires.
- **Commands over I²C *and* UART (verbatim · Henrik 2026-10-06):** *"It would make sense to support
  commands routed over I2C as well as UART. I2C could be a pattern supported on prod."* So the command
  surface is **transport-agnostic**: the same command set is reachable over the **Stem I²C** regmap and
  over the **console UART**. The **I²C command pattern** is itself a **prod** capability (not diag-only)
  — prod already owns the Stem slave + regmap, so command-over-I²C belongs in the shared prod-core and
  diag inherits it; UART command routing (diag has it today) becomes the second transport over the same
  dispatch.

## Repo layout — `modules/<product>` + the build matrix (verbatim + derived · Henrik 2026-10-08)

**SUPERSEDES the "prod is the root dir / diag for the diag images" framing above (§462, 2026-10-06).**
That earlier line is retained for history, but the directory model below replaces it: there is **no
top-level `prod/` or `diag/` tree**. prod/diag is a build-axis FLAG per product module, never a folder.

**Verbatim (Henrik 2026-10-08):**
> *"I do not like the current directory structure at all. It looks like a toy project made for a single
> MCU family. I need something for multiple MCUs and variations in multiple dimensions beyond a
> directory tree."*
>
> *"diag can be seen as an extended POST on top of a prod codebase. --log is just extra console log
> statement. --debug is just debug info."*
>
> *"I would start with root Cargo.toml, and ask what would give it a natural structure."*
>
> *"Build matrix sounds like a good center."*
>
> On the transition case: *"we change the hardware for the PD Controller, but want to keep all the
> logic. We want to develop the firmware for two different chipsets during a transition period. Or
> deepsleep is migrated from MSP430 to the newer TI M0 low power architecture and maintained in parallel
> during transition … each existing/planned product module firmware is a thin crate that depends on
> feature/util crates. It could also be a crate per role, where the actual board is feature flags (but
> that sounds like a big if statement in main)."*
>
> *"prod/ should be prod/<product-name>/ … We would have prod/pico-probe, prod/msp-deepsleep/ and
> prod/msp-expander/."* (naming since refined to `modules/` — below)
>
> *"pico-probe is a product, it is a board or particular bench assembly that can be thought of as a
> product."*
>
> *"I think modules/ as you suggest is a good name."*

**Derived model (agreed in dialogue 2026-10-08) — the structure these imply:**

1. **Organise the TREE by dependency altitude (what a crate may depend on) — the one axis Cargo
   enforces.** Every other axis (chip, role, prod/diag, log, debug) is a **feature or target triple**, a
   *matrix* dimension, NOT a directory. Tiers, each defined by its dependency ceiling:
   - `contract/` — pure spec (regmap/stembus), no HAL/embassy/chip; builds on host + every target + C
     header gen. (= today's `crates/stembus`, held to language-neutral contract discipline.)
   - `capabilities/` — feature/device logic generic over `embedded-hal` traits + `contract`; **host-
     testable**; no embassy, no bsp, no PAC. (= `crates/devices`, `deepsleep-core`, `pd-core`, plus the
     two harness runtimes `prod-runtime`/`diag-runtime`.)
   - `bsp/` — **per-arch**, never cfg-merged across unrelated silicon: `bsp-msp430`, `rp-bsp`
     (RP2040+RP2350), future `ti-m0-bsp`, `nrf-bsp`. Each implements the capability traits + exposes
     semantic bus roles.
   - `modules/<product>/[<chip>/]` — **thin binary crate per product module** (see 2–4).
   - `stembus-client` — the one public master-side crate (M7/host/CI), transport-abstracted.
   - `xtask/` — the build-matrix driver. `target/` is ONE shared Cargo-owned dir.

2. **`modules/<product>/` holds a thin binary per product module; prod/diag/log/debug are FLAGS on it,
   not trees.** diag = prod + extended POST/test harness (`prod ⊆ diag`); `--log`/`--debug` are additive
   features. The only `cfg` allowed in a module `main` is **harness selection**
   (`prod-runtime` vs `diag-runtime` over the shared `*-core`) — never a chip if-ladder.

3. **Chip/arch is bound by CRATE COMPOSITION, not feature flags.** Incompatible chips (different
   arch/HAL/triple, or a transition needing BOTH live) → **separate thin binary crates** sharing one
   `*-core`, chip bound via a trait seam ([[purity-seam-trait]]). Feature-flag-selects-the-board is
   rejected for this axis: it degrades to a big `cfg` in main AND cannot span two target triples in one
   build — so it literally cannot express a parallel-chip transition. Feature flags survive ONLY for
   **compatible within-family** variation (the runtime model-detect of §"One image per product module").
   Transition siblings group by role with chip as the leaf, so they read as obvious pairs:
   ```
   modules/deepsleep/{fr247x, fr235x, fr215x, ti-m0}/ → capabilities/deepsleep-core  (dual SUPERVISOR: FR2476/FR2355/FR2155; ti-m0 = the migration)
   modules/expander/{fr24xx}/           → capabilities/expander-core   (FR2433 programmable I/O EXPANDER: generic pin IDs)
   modules/pd/{cypm1111, <next>}/       → capabilities/pd-core           (PD-controller swap)
   modules/pico-probe/                  → stembus-client  (host/master; a product in its own right)
   ```
   Delete a leaf when a transition ends; the `*-core` never moves. **This settles that firmware binaries
   are separate workspace members** (a transition requires two to coexist on different triples).

4. **The build MATRIX is the real artifact, not the tree.** Flag/chip/variant combinations are **matrix
   rows** the `xtask` enumerates; outputs are **named artifacts in `dist/`** (NOT per-flag `target/` dirs
   — that fights Cargo's one-target-dir + fingerprint model and loses cross-crate caching). Example:
   `dist/deepsleep-msp430-fr2433-prod.elf`, `dist/deepsleep-msp430-fr2433-diag-log.elf`,
   `dist/pico-probe-rp2040-prod.elf`, `dist/pico-probe-rp2040-diag.elf`. Every product module (incl.
   pico-probe) emits BOTH a prod and a diag artifact — no module is privileged; prod/diag is purely the
   flag.

5. **Root `Cargo.toml` = virtual workspace** (`resolver = "2"`), `[workspace.dependencies]` pins shared
   versions once, `default-members` = only the **host-buildable** crates (contract, capabilities,
   stembus-client, xtask) so `cargo test` on the dev host Just Works and exercises max shared surface.
   **No bare `cargo build` builds all firmware** — it can't (feature unification would collide two
   chips' `compile_error!`; multiple triples can't share one invocation). Firmware is built per-target
   by the matrix driver.

**Precision note (per `quality-bar`):** items 2–5 are *derived synthesis agreed in dialogue*, not
Henrik verbatim, EXCEPT the quoted block. Exemplars to verify before templating the matrix driver:
**Tock** (`tock/tock`, per-board binary crates — the precedent for item 3), **embassy**
(`embassy-rs/embassy`, how far features+codegen take the chip axis), **ZMK** (`zmkfirmware/zmk`,
`build.yaml`→CI matrix — the precedent for item 4). These are described from recollection (cutoff
2026-01) and are a **verification debt** — read the live repos + cache per `save-sources-locally`
before adopting any as a template.

### Build frontend — `just` now; `xtask` DEFERRED (adopt only on these conditions · 2026-10-08)

**Decision (Henrik 2026-10-08):** keep `just` as the build frontend for now; hold `xtask` as an **open
option to adopt if** one of the conditions below is hit. Henrik is **neutral** on xtask (noted lack of
detailed knowledge), so this is a deliberate *defer-with-triggers*, not a rejection. Do NOT migrate to
xtask on aesthetics/idiom alone.

**Why deferred (the balance today):** xtask's headline win — "one toolchain, no external installs" — is
**undercut in stem**: the MSP430 build *requires* the Docker/msp430-gcc image and RP needs picotool
(`stem-build-paths`), so xtask would still shell out to them; it changes *where* orchestration lives
(Rust vs `just`), not the toolchain you must install. Meanwhile `just` already works and has real
investment (`justfile` + `bsp.just`/`diag.just`/`prod.just`/`pac.just`/`probe.just`/`usb.just`/…). So
the cost/benefit only flips when the **matrix logic outgrows what `just` can sanely express**.

**Adopt xtask (for the MATRIX/LOGIC layer only) when ANY of these becomes true:**
1. **The matrix stops being a clean cartesian product** — legal `(module, chip, variant, log, debug)`
   rows need filtering/exclusions/special-cases/derived values, i.e. you catch yourself writing a
   bash/python script *to generate the matrix*. That script should be xtask (typed, shares `contract`).
2. **`dist/` naming or build inputs must share types with the firmware** — chip list, role addresses,
   feature sets start getting **duplicated** in the justfile and drift from `contract`/`bsp` constants
   becomes a real bug risk.
3. **A `just` recipe exceeds ~15–20 lines of shell with branching/loops** (loops over targets,
   conditionals on chip family, retries/error handling) — the readability/testability smell.
4. **The matrix needs to be testable** because a wrong/missing combo has actually shipped or left a CI
   gap — "unit-test the expansion" stops being theoretical.
5. **A new frontend need `just` can't model** — generating a CI job matrix from the same source (the
   ZMK `build.yaml`→GitHub-Actions pattern), parallel matrix execution with dependency ordering, or
   staleness-aware skipping beyond cargo's own.
6. **Cross-platform / multi-contributor friction** — Windows support needed, or recipes break across
   zsh/bash/CI shells.

**Shape WHEN adopted (keep it cheap to exercise):** xtask takes **only** the matrix/`dist` expansion;
`just` stays the human **verb layer** (`just build`/`flash`/`dev` call `cargo xtask …` underneath).
Keep the matrix itself **declarative data** (`modules.toml` or `[workspace.metadata]`, à la ZMK
`build.yaml`) with xtask as the small typed **expander**. Never move the simple verb aliases into xtask.

**Anti-trigger (do NOT adopt for these):** "it's more idiomatic", "it'd be cleaner", or wanting a
single toolchain — the Docker/picotool deps mean the single-toolchain argument never fully applies here.

## One image per product module — avoid chip-specific firmware (verbatim · Henrik 2026-10-06)

*"my ideal outcome is one image per product module. In real world products it's common to swap specific
chip models due to supplies/bugs/new-features. This is why I prefer avoiding chip specific firmware
images."*

**Decision:** the artifact is **per product module**, not per chip model. The image **runtime-detects
the concrete model and adjusts** (the `model.rs` seam: `detect()` → `pin_map()` hardware-capability
flags + `stem_addr()` role). This commits to **true multi-model family images** (not model-specific
builds) — because second-sourcing (supply / errata / new variant) is routine and per-chip firmware is a
BOM liability.

**Boundary condition (not negotiable, physics):** "one image" holds only across chips that share a PAC
/ register-/memory-map-compatible **family**. Within a family (e.g. FR2476 ⇄ FR2475) it is literally one
binary; a swap to a **different** family (e.g. FR235x ⇄ FR24xx) is the **same source, a different
family build**, module-bundle-selected — not one binary. So each product module's **substitution set**
should be chosen within one family where possible; cross-family second-sources cost a build axis, not a
rebuild of logic.

**Cost this imposes:** because the image must drive the *correct* hardware on a swapped chip,
`pin_map()`/capability detection must be **real and tested per family member** — the current thin
placeholders (`bank_count: 5` for both dual-I²C rows) must be filled from the BOM/`connections.toml`
before multi-model portability is trusted in the field.

## Expanders & semantic pin identity (verbatim)

Expanders keep a **current state** on connected pins. An expander also keeps **topical identity** of
each connected pin. This allows a cluster of expanders to provide **semantic control of pins
regardless of how they connect**. Using stembus it becomes easier to iterate through hardware designs,
changing the location and connection of chips on the way.

Connected pins are identified by multiple IDs:
- **uid** — global known pin ID
- **Source chip**
- **Source chip pin**
- **Topic**
- **Topic pin**

### Topics
- T-USB 3.0 alt modes
- T-USB 2.0 alt modes
- M.2 Key B SSD
- M.2 Key E WiFi
- SoM NPU wakeful module
- Left Cam Module
- Right Cam Module
- PD Controller
- Battery charger

For each pin on the expander it defines the **topic no** and **topic pin index** that it connects to.
This allows other MCUs to **request state change without knowing the pin number** — the semantic
(topic, topic-pin) addressing rather than a physical pin number. This is the direction the pin
registry ([crates/bsp/connections.toml](crates/bsp/connections.toml)) evolves toward.
