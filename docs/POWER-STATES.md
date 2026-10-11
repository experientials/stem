# System power states (DRAFT 2026-10-03 — refining with Henrik)

> **Docs:** [README](README.md) (index + reading order) · [STEM.md](STEM.md) (scenarios) ·
> [POWER-DISTRIBUTION.md](POWER-DISTRIBUTION.md) (how each part is powered) ·
> [OPEN-QUESTIONS.md](OPEN-QUESTIONS.md).

The whole-system power ladder, **lowest (no energy) → highest (full run)**, as a single unified model
(§1). The three actors each run their own state machine that *maps onto* this ladder (§3).

## What DSM is, and the GPC's role (Q3) — primary sources
From **IMX8MPRM Ch.5 (GPC), p566–573** and **AN13400** ("i.MX 8M Low Power Design By M Core Running In
System Suspend", Rev 1, both in-repo under `ziloo/Hardware/refs/i.MX 8/`):

- The SoM has **two independent CPU domains** — Quad-A53 and M7 — each with its own low-power modes
  **RUN / WAIT / STOP**. There is **one *system* low-power mode: DSM (Deep Sleep Mode)**.
- **DSM** = the SoC's deepest sleep. On entry the hardware **switches VDD_SOC from Run to DSM voltage and
  shuts down ALL PLLs and clocks**; the **GPC also puts the PMIC into standby** (`GPC_SLPCR[VSTBY]`),
  bypasses regulators (`RBC_EN`), retains memory, and can cut **VDD_ARM** (the A53 core rail). (RM p570.)
- **The GPC is the orchestrator**: it sequences power-down/up, handshakes with CCM (clocks), SRC (power
  seq), and Analog (the VDD_SOC voltage switch + PMIC standby). ATF programs it. Its always-on wakeup
  logic catches the wake IRQ.
- **Entry rule:** the system enters DSM when **both A53 and M7 are STOP** and DSM is enabled. So if the
  **M7 is left RUN/WAIT, the system does NOT enter DSM** — it uses **Fast-Wake-up-Stop mode (not DSM)**:
  VDD_SOC stays at **Run** voltage, the M7 runs (AN13400 demo: at 24 MHz), **DDR is held in Retention**,
  most PLLs/bus-clocks (AXI/AHB/NOC) are gated except what the M7 needs, and the **M7 wakes the A53 via
  the MU (Messaging Unit) interrupt**. (AN13400 §"Make the M core alive…", ATF optimization, p5–30.)

So: **"full SoM suspended" = DSM** (both cores STOP, deepest); **"M7 running / A53 suspended" =
Fast-Wake-up-Stop** (M7 alive, A53 in Stop, system *not* in DSM).

## 1. The unified ladder
> **VSYS is the SoM's power input.** Below it, the SoM gets no power at all. The **PD controller + MSP run
> on the SYS rail**, which can be up while VSYS-to-SoM is still gated — so the product can give an
> experience *before/without* the SoM. (Rail topology **TO VERIFY** against the 292-power-module + carrier
> schematic — which rail feeds the MSP vs the SoM, and exactly how the PD controller gates the SoM.)

| # | State | SoM | What runs | Entered when |
|---|---|---|---|---|
| **S0** | **Dead / disconnected** *(floor)* | off | nothing — no energy | battery dead **AND** no USB/VBUS |
| **S1** | **Ship / storage** | off | nothing (BATFET off, ≈17 µA) | ship mode; wake = QON button or VBUS |
| **S2** | **Low-battery — SoM FORCED-SUSPENDED** | **held off** | **PD controller + MSP give the product experience**; charger brings the battery up | USB/charging, but battery **can't yet run the M7**. Running the SoM on a dead/marginal battery makes no sense → **PD controller forces the SoM to stay suspended** |
| **S3** | **SoM suspended (DSM)** | DSM: both cores STOP, VDD_SOC→DSM volt, PLLs off, PMIC standby, DDR retention, VDD_ARM off | **MSP = Sensing** (watches sensors, triggers resume) | battery ≥ "enough to run M7"; VSYS to SoM up; SoM booted then DSM |
| **S4** | **M7 running, A53 suspended** | Fast-Wake-up-Stop (not DSM): **M7 RUN**, A53 STOP, VDD_SOC Run volt, DDR retention | M7 owns real-time I/O; wakes A53 via MU IRQ | always-on low-power work needed; A53/Linux suspend-to-RAM |
| **S5** | **Full run** | A53 (Linux) + M7 running | **MSP = Passive** (SoM owns the buses) | normal operation |

Power rises S0→S5. **Staged power-up:** when the battery crosses the threshold and VSYS comes up, the
SoM should **not** jump straight to S5 — it comes up controlled (DSM/M7-first), resuming to full only
when needed/allowed. (Your point 3.)

## 1b. Power magnitudes + the DSM-vs-M7-alive call (Henrik 2026-10-03)
Measured — **AN13400 p29–30** (in-repo) and the **FR2355 datasheet p1**:
- **M7 @24 MHz, A53 suspended, ATF-optimized** (DDR retention, PLLs/bus-clocks gated): **VDD_SOC ≈ 10.7 mW**
  (29.5 mW unoptimized). → S4 is **~tens of mW**.
- **FR2355 watching in LPM3:** **1.43 µA ≈ ~5 µW** (LPM3.5 620 nA; shutdown 42 nA). → the MSP floor.
- M7-alive : MSP-watching ≈ **~2000×**.

**Consequences (resolve the S3/S4 usage policy):**
- **DSM is rarely worth it** (Henrik): with both cores STOP but the **SoM still powered + DDR in
  retention**, DSM is *still mW-scale* (DDR retention dominates) — it saves little over S4 yet loses the
  M7's function. **Prefer S4 (keep the M7) over S3 (DSM).**
- **M7 @24 MHz is usable deep into the battery** — e.g. at ~10%: 10% of a 3 Ah cell ≈ 1 Wh at ~tens of
  mW ≈ **~a day** of responsive operation. So the M7 stays the low-power workhorse, not just a full-run core.
- **The real µW deep-standby is NOT DSM** — it's the **SoM fully OFF (DDR off, no retention) with the MSP
  as the sole watcher** (~µW → **weeks–months**). That µW floor is *why the external MSP exists*; a
  powered SoM (DSM) can't reach it. Cost: resume = a **cold boot** (seconds), so it's for storage /
  critical battery / long-idle, not quick wake.

So the practical tier set is **S5 full / S4 M7-alive (day-scale, to ~10%) / SoM-OFF+MSP (month-scale,
µW)**; DSM is the skippable mW middle. (This makes the S0–S2 "SoM off, MSP runs" rungs and the deep
standby the *same kind* of state — SoM unpowered, MSP the product — differing only by battery level and
MSP activity.)

## 1c. Power distribution + battery thresholds (Henrik 2026-10-03)

**Deep-standby floor is the PD controller, not the MSP.** CYPM1111 deep-sleep ≈ **150 µA while SYS is
live** (DECISIONS.md), and the MSP can only watch while SYS is up → system deep-standby ≈ **~200 µA**
(CYPM1111 150 + MSP ~10 + low-power wake sensor ~tens). So **1 week ≈ 30–50 mAh** (~1.5% of a 3 Ah 18650;
~6–10 weeks even from 10%). *µW-scale* only if the CYPM1111 is in **ship mode** and the MSP sits on a
**micropower always-on rail** (~µA → months). **Correction:** the earlier "µW deep standby" is µA-scale
unless that micropower rail exists.

**Rail plan — coarse at the PD controller, fine at the MSP.** The CYPM1111 is on the power module (292),
not the carrier, so it **can't have many control tentacles** across the module↔carrier B2B. Split:
- **PD controller** provides a **handful of coarse switched rails** across the B2B.
- **MSP** (local on the carrier/202) does **fine per-sensor/per-MCU control** (it owns the sensor enables).

Proposed handful of rails:
1. **µ-AON** — micropower always-on (from BAT via a tiny LDO, alive even in ship mode): **MSP + the
   critical wake sensor**. The µA-scale deep-standby + autonomous-wake rail.
2. **SoM rail** — gated (VSYS→PMIC→SoM). **The SoM lives here alone**, so "never power the SoM
   intentionally/inadvertently" = never enable this rail. Gated by a CYPM1111 GPIO → external load switch
   (datasheet-confirmed mechanism; PMG1-S1 drives external FETs / enables, it is not a multi-rail PMIC).
3. **Peripheral / sensor-group rail** — gated: heavier sensors / camera that only run when the SoM is up.

**The SoM is a natural floor — power management is binary, not a dial (Henrik 2026-10-03).** The single
largest step in the ladder is the SoM itself: full-run ~W → M7-alive ~tens of mW (~30–50×) → SoM-off
~150–200 µA (~**100–250×** more). Nothing meaningful sits between "SoM on" and "SoM off", so the system
lever is just **SoM-on vs SoM-off**, one clean crossover (confirms DSM is skippable; full-vs-M7-alive is
the A53's own GPC business, not a system rung). **Policy: cut the SoM early** — switch it off once the
battery hits the deep-standby **reserve** (e.g. ~50 mAh / 1 week), rather than letting the ~tens-of-mW
SoM burn the last capacity in hours. The reserve line *is* the natural floor; below it only the µA
watcher remains, down to the hard 3.45 V SoM cutoff.

**Battery thresholds — by projected runtime, not raw voltage** (PD controller owns this, via bq25601
telemetry; MSP just tracks the resulting SoM-run-status):
- reserve **≥ ~50 mAh** → guarantee the **1-week deep-standby** window → below this, **force deep standby**
  (gate the SoM rail);
- **~10%** → allow **M7-alive** (~a day);
- higher → allow **full run**.

**Board rethink (OPEN — real design pass, not a tweak):** the existing 292 / carrier / 202 designs
predate the PD-controller-as-power-brain. This rail split (µ-AON + gated SoM + gated peripherals, driven
by a power module with few B2B control lines) needs a dedicated hardware design pass. TO DO against the
292 power-module + carrier schematics.

## 2. Mapping to the Real World Value scenarios (STEM.md)
- "USB plugged to revive a dead battery" → S0/S1 → **S2** (charge while the MCUs run the experience).
- "Battery too low → PD controller suspends and stays suspended" → held in **S2 forced-suspend** (resume
  to S3+ blocked until the battery recovers).
- "Battery above very-low → M7 runs when needed" → S2 → **S3/S4**.
- "Long out of use → M7 suspends, Sensing supervisor decides resume" → S4 → **S3**.
- "Movement detected by Sensing supervisor → resume M7" → S3 → **S4**.

## 3. The three per-actor state machines (keep in their own areas)
Lead with the unified ladder above for system understanding; maintain each actor's own states where they
live, each noting which ladder rungs it owns:
- **PD controller (CYPM1111)** — Ship / dead-battery-boot / Active-charging / deep-sleep + **the
  forced-suspend gate on the SoM**. Owns **S0–S2** (and the S2→S3 release). Home: `talkihw/Power Module/…`
  (DECISIONS.md), `power-path-eval` skill.
- **SoM GPC** — per-domain RUN/WAIT/STOP; **DSM** (both STOP); **Fast-Wake-up-Stop** (M7 alive). Owns
  **S3–S5**. Home: IMX8MPRM Ch.5 + `AN13400` (in-repo).
- **MSP sleep-supervisor** — **Passive / Sensing** (+ its own boot). Rides **S2–S5** (whenever SYS powers
  it): Sensing in S3 (SoM asleep, MSP watches/resumes), Passive in S5 (SoM owns buses). Home:
  `prod/DESIGN.md`, STEM.md.

## How the MSP determines state
The MSP needs exactly one thing to do its job: **is the SoM running (it owns the sensor bus) or
suspended (the MSP must take the sensor bus)?** It gets this from two sources:

1. **The PMIC pins — the hardware fast-path** (works even when the SoM can't talk I²C, i.e. while
   suspended). The MSP reads the PCA9450 state machine from **`PMIC_ON_REQ` + `PMIC_STBY_REQ` + `POR_B`**
   (PCA9450DS p6–16) and maps it directly to its mode:
   - **RUN** (ON_REQ high, STBY low, POR_B released) → the M7/A53 owns the sensor bus → **Passive**.
   - **STANDBY / OFF** (STBY high, or ON_REQ/POR_B low) → SoM suspended → **Sensing** (MSP masters the
     sensor bus). `PMIC_STBY_REQ` directly flags DSM entry; the MSP edge-triggers on it.
   - PWRUP / PWRDN → transitioning (hold).
2. **The Stem I²C — the rich channel** (while the SoM is alive). The SoM, as Stem master, writes the
   MSP's `MODE_CTRL` (0x3E) + registers to command/convey mode and context directly — refining or
   overriding the pin-derived state.

**The MSP runs at a single rate** — there is no battery-level-dependent behaviour — so it needs **nothing
from the PD controller**; the PMIC pins + Stem I²C are the complete input. (Resolves OPEN-QUESTIONS
Q-STATEPINS.)

### Waking the SoM — two paths, by depth (reconciled 2026-10-03)
The MSP's pins let it *determine* the SoM state always, but whether it can *wake* the SoM depends on
whether the SoM is **powered** or **off**:
- **SoM suspended but POWERED** (rails up, M7 in stop/DSM): the MSP **wakes the M7 directly** by asserting
  its attention line (`STEM_INT`) — a wakeup source to the live SoC. Fast; costs ~mW to hold.
- **SoM OFF** (`V_SOM_EN` low): **deliberate** — the battery is insufficient, so the SoM **should not**
  wake. The MSP is **record-only** (senses + logs history for later transfer); it does **not** try to
  wake the SoM, and *failing to wake is the correct behaviour*. The SoM returns **only** when the **PD
  controller** judges the battery recovered enough for a useful runtime and re-asserts `V_SOM_EN` — a
  **battery-driven decision, not an MSP wake request**. (Long battery life; slower, staged boot.)

**PD-controller re-power (the SoM-OFF → on path):** re-power only once the charge is **enough above the
3.45 V SoM floor for a useful runtime** (a reserve/runtime decision, with charger telemetry). The SoM
comes up **staged, M7-first** — either the PD controller re-powers **+ signals suspend** (M7 only, A53
held), or the **default boot is M7-only** and the **M7 decides when a full boot is worth it**. Never
straight to full. So the **powered-suspend vs off** choice is a **responsiveness-vs-battery-life
tradeoff** (MSP-wakes-M7 fast at ~mW, vs PD-re-powers slow at ~µA).

**The "out" if a battery-aware rate is ever needed (Henrik 2026-10-03):** the **PD controller masters the
Stem I²C while the SoM is switched off** — it becomes the Stem master in the SoM-off regime and commands
the MCUs (e.g. sensing rate) by battery level, with **no extra handshake wire** (it uses the existing
bus + the role-based protocol). Requires the Stem bus to reach the PD controller / power module. A
future direction, not the current design — see OPEN-QUESTIONS.

## Open points
- **Rail topology (TO VERIFY):** which rail feeds the MSP vs the SoM; the exact forced-suspend mechanism
  (PD gates VSYS? holds SoM reset? commands PMIC standby?). Check the 292 power-module + carrier schematic.
- S3↔S4↔S5 transition triggers + latency (MSP Passive/Sensing handoff + attention/resume).
- WAIT-vs-STOP granularity within S4/S5 (light sleep, fast wake) — detail, not top-level states.
