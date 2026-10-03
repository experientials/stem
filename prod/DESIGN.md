# prod — design & requirements (Stembus node firmware)

This is the authoritative design doc for the `prod/` firmware. The block below is the product
owner's requirements captured **verbatim** — it is the anchor; everything else on this page is a
*derivation* of it. If a derived section disagrees with the canonical block, the canonical block wins.

---

## CANONICAL (verbatim) — requirements

> The prod firmware needs to have some basic boot logic for enumerating the sensors on I2C and other
> detection of what other devices can be sensed. It must use one I2C bus for sensors and another for
> being a slave on the I2C MCU bus. On FR2433 the I2C would just be on the MCU bus. The firmware must
> detect what exact model it is loaded on and correctly map the pins. We would support multiple
> MSP430 models with the same binary firmware image

> If we get to the limit of firmware size, we can use feature macros to leave out blocks of
> functionality from the built firmware

> I want the least number of firmware permutations in releases. I don't yet know the exact MSP430
> models used on different destinations. Each product potentially has multiple chips. Different
> products potentially has different models. I want to limit permutations and try to determine at
> runtime if possible. Worst case the UCM SoM determines what firmware to load out of a firmware bundle

> Add a plan for modes in the firmware.
> 1) Passive - the SoM will use it as an I/O extender, it doesn't monitor sensors on I2C bus or
>    attempt to master
> 2) Sensing - the SoM is suspended and doesn't master the I2C sensor bus. MSP attempts to master the
>    sensor bus. It monitors the state of sensors and will under particular conditions raise signals to
>    PMIC to resume SoM operation.
> 3) Other modes not yet defined
>
> Prod builds can include support for specific modes or leave them out

> FR2433 is passive only. We would not include sensing mode when building for it. Yes indeed
> mode-sensing is a gating of existing prod behavior, but also an attempt to become master of the bus
> when switching to it. The modes are a runtime switch assuming they are included in the build

_(captured 2026-09-21; modes added 2026-09-24)_

---

## Derived design (subordinate to the canonical block)

### Roles & buses
- The firmware is a **Stembus node** with up to **two I²C buses**:
  1. **MCU-bus SLAVE** — the control surface the SoM masters; register map in [`src/regmap.rs`](src/regmap.rs)
     (PCA9698 emulation + Thepia extensions, per [../I2C-API.md](../I2C-API.md)).
  2. **Sensor-bus MASTER** — enumerates/monitors attached I²C sensors; result proxied to the SoM.
- **On the FR2433 (1× I²C): only the MCU-bus slave exists — no sensor master.** A part with a 2nd
  I²C (e.g. FR2476, 2× eUSCI_B) runs both roles. This matches the tracked split
  ([[msp-fw-fit-fr2433]]): one I²C can be slave *or* master, not both.

### Operating modes (derives from the 4th canonical statement)

A **mode** is the MSP's operating posture. The modes are defined by three things: **(a) who masters
the sensor I²C bus**, **(b) whether the MSP monitors sensors**, and **(c) whether the MSP signals the
PMIC to wake the SoM**. The MCU-bus SLAVE surface (`src/stem.rs`, eUSCI_B1) is the SoM's control/IO
channel and is active in **every** mode — modes differ on the *sensor-bus* behaviour, not the slave.

| Aspect | **Passive** | **Sensing** | Mode 3 (TBD) |
|---|---|---|---|
| SoM state | active (masters its buses) | **suspended** | — |
| Sensor bus (eUSCI_B0) master | **not the MSP** (SoM owns it, or bus idle) | **the MSP** | — |
| MSP monitors sensors | no | yes (enumerate + `measure`) | — |
| MCU-bus slave surface (B1) | yes — SoM uses MSP as an **I/O extender** | yes (SoM reads captured state on resume) | — |
| Raises PMIC wake signal | no | **yes**, under defined conditions | — |
| MSP role | pure I/O-extender / slave | autonomous **supervisor** | — |

**The safety-critical invariant: exactly one master on the sensor bus at any instant.** Two masters
on one I²C bus corrupts it. So the modes are, at heart, a **bus-mastership handoff**:
- **Passive** → the MSP must **not** master B0 (it releases / never drives the sensor-bus pins), so the
  SoM (or nobody) owns that bus. MSP just serves the register/GPIO interface on B1.
- **Sensing** → the SoM is suspended and off the sensor bus, so the MSP **safely takes** B0, monitors,
  and wakes the SoM. This is the low-power "watch for attention while the SoM sleeps" role — the
  VL53L0X approach classifier, APDS proximity, and RCWL motion are the "particular conditions" that
  raise the PMIC wake.
- A **mode transition is a sequenced handoff** (e.g. SoM suspends → commands Sensing → MSP takes B0;
  MSP raises wake → SoM resumes → commands Passive → MSP releases B0). Getting the ordering wrong =
  two masters. This sequencing is an OPEN QUESTION (below).

**Build-time inclusion (per the canonical "include or leave out").** Each mode is a **Cargo feature**
(`mode-passive`, `mode-sensing`), orthogonal to the family axis and `console`. A build compiles in only
the modes it needs — smaller image, and a passive-only node can't be tricked into mastering the bus.
At least one mode feature is required (a `compile_error!` guard, like the family axis).

**Modes are a RUNTIME switch** among whatever the build compiled in. The build decides *which modes
exist* in the image; at runtime the active mode is selected/changed — the SoM writes a mode register
(candidate: `MODE` 0x2A, or a Thepia-ext byte; the boot default may persist via `INIT_CODE` 0x2B in
FRAM). A build that includes only one mode simply stays there (mode writes to an unbuilt mode are
rejected). This keeps release permutations low (canonical statement 3): one multi-mode image serves
several destinations, switched at runtime.

**Switching a mode is an ACTION, not just a flag** (canonical: "an attempt to become master of the bus
when switching to it"):
- **→ Sensing:** the MSP **attempts to acquire the sensor bus** — bring up eUSCI_B0 as master
  (`i2c::init`, with the stuck-bus recovery), then run `enumerate`/`measure`. "Attempt" because the bus
  must be free (the SoM must already be off it); if B0 can't be mastered (SDA held), it stays out and
  reports a fault rather than fighting a second master.
- **→ Passive:** the MSP **releases the sensor bus** — stop mastering, tri-state / hand back B0's pins —
  so the SoM (or nobody) owns it. MSP then serves only the B1 slave (I/O-extender).
- Either way `stem.rs` (B1 slave) keeps running — the SoM issues the switch *through* it.

**Mapping to the code today.** The current unconditional B0 `enumerate` + sensor `measure` **is the
Sensing behaviour**; `stem.rs` (B1 slave) is the always-on part. Implementing modes = moving the
sensor-*master* bring-up/activity behind `mode-sensing` and driving it from the mode transition
(acquire on →Sensing, release on →Passive), while `stem` stays. No mode logic is written yet — this is
the plan.

**Family interaction (CONFIRMED).** Sensing needs a *second* I²C to master the sensor bus, so it is a
**dual-I²C (fr247x / future fr215x) capability**. The single-I²C **FR2433 (fr24xx) is Passive-only** —
its one bus is the MCU-bus slave; it has no separate sensor-master bus ([[msp-fw-fit-fr2433]]).
**`mode-sensing` is simply not built for `fr24xx`** (a build-time exclusion, enforced by a
`compile_error!` when `fr24xx` + `mode-sensing` are combined) — so an FR2433 image is Passive-only and
can never attempt to master the bus.

### How the wake is hooked up (PMIC path — designed from the NXP source of truth)

**The MSP *is* the I/O expander — it replaces the old discrete expanders.** In the Stem architecture
the MSP (this firmware, emulating a PCA9698 — `src/regmap.rs`) *is* the system's I/O expander. The
production **FR2155** / dev **FR2476** running this firmware **replace the `PCA9555` + Faceboard `FR2032`
expanders** of the old designs (`ziloo/Hardware/pinouts/I2C_EXPANDER_*.md`, `.../929/Faceboard/*`) — same
job (I²C GPIO expansion + PMIC/wake control), now one smart supervisor MCU instead of dumb expanders +
a helper MCU. So the SoM's PMIC control lines are the MSP's own GPIO pins, exposed as **OP-bank output
bits** in the register map, and "wake the SoM" = **the MSP drives its `PMIC_ON_REQ` OP bit high** — no
external expander in the path. Treat the old carrier's discrete-expander PMIC wiring as **outdated
reference only**, not the target design.

**PMIC = NXP PCA9450.** Design facts taken from the source of truth, `ziloo/Hardware/datasheets/i.MX8/
PCA9450DS.pdf` (rev 2.2, 2021-09-15) — not the carrier netlist:

- **`PMIC_ON_REQ` (pin 39, digital input):** *"PMIC ON input from Application processor. When it is
  asserted high, the device starts power on sequence."* (Table 3.) → assert HIGH to power on / resume.
- **It is a LEVEL, held — not a pulse.** §7.3.4 PWRUP: power-up starts *"when `PMIC_ON_REQ` is asserted
  high **for longer than debounce time, `tON_DEB`, which is programmable in the `PWR_CTRL` reg**."* So
  the MSP must drive it high **and hold it** past `tON_DEB`; keeping the SoM on = keep it high.
- **Masked until `RTC_RESET_B` is released** in SNVS mode (§7.3.3): after cold VSYS, the PMIC needs
  ~`tSNVS_PU` 20 ms + `tRTC_RST` 20 ms before it will accept `PMIC_ON_REQ` — the MSP can't wake it
  instantly at first power. Full SNVS timing in Table 5.
- **Standby / reset companions:** `PMIC_STBY_REQ` (pin 40, DI) high → STANDBY; `SYS_RST_PMIC` low →
  reset; `POR_B` is the PMIC's power-on-reset **output** (→ an MSP **IP**-bank input bit).
- **Electrical — the real design constraint:** `PMIC_ON_REQ` lives in the **1.8 V SNVS (always-on)
  domain** (LDO1 `NVCC_SNVS` = 1.8 V). The MSP's I/O rail is ~3.3 V, so driving it directly would
  over-drive a 1.8 V input. Design it as **open-drain to `NVCC_SNVS` (1.8 V) with a pull-up, or a
  level translator** — the MSP OP bit pulls the line, the 1.8 V rail sets the high level. Confirm
  `V_IH`, `tON_DEB` default, and the pull against `PCA9450DS.pdf` before choosing the MSP pin/drive.

**Register-map integration (the firmware hook).** Reserve named OP-bank bits for the PMIC lines
(`PMIC_ON_REQ`, `PMIC_STBY_REQ`, `SYS_RST_PMIC`) and an IP bit for `POR_B`, in the emulated-expander
space. Then: waking = set the `PMIC_ON_REQ` OP bit (held); the SoM, once up, can read the same bits over
the Stem I²C slave. This folds the whole power state machine into the register interface already built.

**The handshake (how the MSP knows when to act).** `SOUND_INT` (SoM `GPIO1_IO00` → carrier **P20.12**)
is the SoM→MSP "**M7 active / suspended**" cue — the SoM has slept, so the MSP takes the sensor bus
(enters Sensing). `STEM_INT` (SoM `GPIO1_IO01` → **P20.14**) is the MSP→SoM notify line. Round-trip:
  1. SoM sleeps → asserts `SOUND_INT` (and/or writes the mode register) → MSP enters **Sensing**, acquires the sensor bus.
  2. MSP monitors; a **wake condition** fires → MSP sets `PMIC_ON_REQ` OP bit high (held > `tON_DEB`) → PCA9450 powers the SoM up.
  3. SoM resumes → reads captured state over the Stem I²C slave → commands **Passive** → MSP releases the bus.

**Wake conditions (the "particular conditions"):** already implemented as trigger logic —
`devices::vl53l0x::{RangeTracker, Attention}` (approach/threshold), APDS proximity, RCWL-0516 radar on
P2.4 (port-interrupt / wake-from-LPMx.5); NFC field-detect as a secondary always-on wake into the MSP.

**Still to assign:** the concrete MSP port pin for the `PMIC_ON_REQ` OP bit (open-drain-capable, per
above) → `connections.toml som_wake`. Everything else — the PMIC behaviour, the level/timing rules, the
register-map home — is now designed from the datasheet.

**Open questions (remaining before the FULL mode-machine impl):**
1. **Mode 3** — undefined.
2. **Wake line residuals** — assign the MSP `som_wake` GPIO + confirm the PCA9450 `PMIC_ON_REQ` V_IH /
   pulse spec and the 3.3 V→1.8 V translation (see "How the wake is hooked up"). The *path* is resolved;
   these are the electrical/pin details.
3. **Passive sensor access** — does the SoM master the sensor bus directly (shared wiring), or are the
   sensors simply dormant while Passive? (Hardware-topology dependent.)
4. **Default/boot mode** and the exact **mode register** + persistence scheme (candidate: a Thepia-ext
   register; `SOUND_INT` may also drive the Passive↔Sensing transition directly).
5. **Transition sequencing / bus-handoff protocol** so the SoM and MSP never both master the sensor bus.

### Boot logic (the "basic boot logic" required)
0. **Stem slave up FIRST** — the SoM's always-on control channel (eUSCI_B1 slave at the role address) is
   configured **before** any sensor-bus work, so the node is reachable the instant it boots even if the
   sensor bus is wedged/slow. *(Regression fixed 2026-10-03: it used to come up AFTER enumeration, so a
   two-master-wedged Leaf bus delayed — and could indefinitely block — the SoM's only way in. Observed
   on the bench as "FR2355 appears on the Stem bus a few seconds late.")*
1. **Model detection** — read the device ID from the MSP430 **TLV device descriptor** / SYS device-ID
   registers; pick the model record at runtime.
2. **Pin mapping** — apply that model's map: which port pins are GPIO banks (regmap), which are the
   MCU-bus I²C, the sensor-bus I²C, ADC (VSOM/CHARGE), and the STEM INT/MSG line.
3. **Default mode = Passive** (resolves open-Q#4 for now) — at power-on the SoM/host is active and owns
   the buses, so the MSP stays an I/O-extender (OFF the sensor bus) until the SoM commands Sensing (when
   it suspends). This also prevents a **boot-time two-master fight** on the sensor bus. `mode::default_boot`.
4. **Sensor enumeration** — ONLY in Sensing: if a sensor-master bus exists, probe it and record what's
   present / sensable; expose to the SoM.

### Single binary across multiple models
Requirement: **one binary image serves multiple MSP430 models**, detecting the exact model and
mapping pins at runtime. Feasible **within a register-/memory-map-compatible family** (same peripheral
base addresses + FRAM/RAM origins; models differ mainly in package/pin-count). See the OPEN QUESTION
below — this does **not** trivially extend across families whose peripheral bases or memory maps differ.

### Size management & the per-image FRAM ceiling
If the image approaches its FRAM budget (gated by [../scripts/size-check.sh](../scripts/size-check.sh)),
**feature-gate** optional blocks (Cargo `features` + `#[cfg(feature = …)]`) so functionality can be
compiled out per build. Candidate gates: sensor-master + enumeration, voltage thresholds/messaging,
extended/custom registers, OTA/flash-lock.

**Per-image budget = the part's FULL program FRAM.** Update is **host-driven BSL reflash** (the i.MX8
SoM is root of trust — it enters the ROM BSL over RST/TEST and rewrites; a half-write is recoverable by
re-entering BSL, so no on-MCU bank pair is needed). See MCU_SELECTION.md "Update model" +
[[msp-fw-ota-host-root-of-trust]]. **On-MCU A/B self-update is a distant priority / non-driver**
(Henrik, 2026-09-21) — so we do NOT reserve a second bank; a node runs one full image.
_(Prior note, now deprioritized: Henrik earlier floated_ "We have discussed 2 bank update model, for
that 64K FRAM = 32K ceiling" _— that halving only applied if A/B were adopted, which it isn't for now.)_
Usable budget per part: **FR2433 = 15 KB**, **FR2155 = 32 KB** (the dual-I²C baseline), FR2476 = 32 KB
only via the toolchain lower-window if ever used.

---

## Permutation strategy (RESOLVED 2026-09-21 — derives from the 3rd canonical statement)

Goal from the canonical block: **the least number of firmware permutations in releases**, models not
yet known, multiple chips per product, different models per product, **runtime-detect where possible**,
**worst case the SoM selects the right image from a bundle**. Derived architecture:

1. **Widest runtime coverage per image.** Each built image runtime-detects the model (TLV device
   descriptor / SYS device-ID) and applies a pin-map table, covering **every model in its
   register-/memory-map-compatible family** with ONE binary. This minimizes images within a family.
2. **One image per incompatible family, not per model.** A single linked ELF can't span differing
   FRAM origins / peripheral bases (FR2433 `0xC400` + its PAC vs FR2476 `0x8000` + `e_usci_*` PAC), so
   the permutation count = **number of incompatible families in the product BOM**, not number of
   models. Same source; family chosen at build time via Cargo `--features` (+ PAC selection).
3. **SoM bundle-selection is the fallback, not the primary mechanism.** The release ships the small
   set of family images as a bundle; the UCM SoM, which knows the board it's flashing, picks the
   right one. Runtime detect handles within-family variation so the bundle stays tiny.

Net: permutations = **#incompatible-families** (aim: 1–2), each covering its whole family at runtime.

### Consequences for crate structure — IMPLEMENTED (2026-09-21)
- A `model` module: `detect() -> Model` from the device descriptor; a `PinMap` the rest of the code
  consumes. Model table is **runtime data within a family**, not `#[cfg]` per model. ✅ `src/model.rs`
  (+ `matches_build_family()` catches a wrong-family flash).
- The **family/PAC** is the only compile-time axis (a Cargo feature), kept to as few as the BOM forces.
  ✅ **live:** `fr247x` (default, FR2476/FR2475 dual-I²C) and `fr24xx` (FR2433 single-I²C) both build —
  the feature selects the optional PAC dep, the `#[cfg]` peripheral access in `main.rs`, the
  `memory-<family>.x` (via `build.rs`), and the gate budget (`prod.just`). Both verified gating (fr247x
  110 B/32 KB, fr24xx 110 B/15 KB). FR2155 joins as a future `fr215x` family. **FR2433 is NOT dropped —
  it is a first-class family; fr247x is only the default (the on-hand dev boards).**
- `#[cfg(feature)]` blocks (sensor-master, thresholds, extended regs, OTA) trim size per the 2nd
  canonical statement — orthogonal to the family axis (esp. to fit the smaller `fr24xx` budget).

**Still needed to implement (not blocking the seam):** the candidate MSP430 model list per product/
destination, to fill the pin-map tables. Until then, build the `model`/`PinMap` seam generically.
