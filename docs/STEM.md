# Stem Usage

Product experience + QA focused document

## Real World Value

- VSYS (~3.6V) comes on because USB was plugged in to revive a dead battery
- VSYS is switched off because the battery is too low
- Battery is very-low so the PD Controller tells system to suspend and stay suspended
- Battery moves above very-low, letting RT M7 kernel run when needed.
- System has been out of use for a long time so M7 suspends and lets the Sensing supervisor determine when to resume the M7
- M7 System is suspended and movement/human presence/nearness is detected by Sensing supervisor, so it resumes the M7
- We have a distinction between M7 running/suspended and the full SoM is running/suspended.
- Left untouched, the product drifts to sleep on its own (idle → M7 suspends, Sensing supervisor watches).
- After weeks in a drawer, a pick-up / presence / nearby voice wakes it in a blink — the supervisor was watching at µA and resumes the M7, not a cold reboot.
- A parent taps a phone to the toy and its mode/config changes (NFC) — no opening it, no waking the SoM.
- One sense naps while another listens (vision off, sound-wake armed) to stretch the battery.
- A real attention event (approach/motion/sound) raises the shared interrupt → the M7 wakes, polls all MCUs, acts on the one that reported.
- Every boot, each MCU self-tests over UART; the M7 spots a sick one and re-flashes it — the product heals rather than bricks.

## PD Controller related

- Plugged in on a flat battery, the product keeps working off USB while the cell charges in the background (loads run off the input, not the charging cell).
- The battery crosses the deep-standby reserve, so the PD controller switches the SoM off — a week+ of watching instead of hours of full run.
- While the SoM is off, the PD controller runs the show over the Stem bus — MCUs + sensors still respond; the SoM returns only when it's worth it. (optional future plan)
- A critically low battery → the product parks gracefully (saves state, drops to the µA watcher) instead of dying mid-play.
- The battery level is actually known (charger telemetry) — "time left" and the sleep/wake thresholds are real, not guesses.

## M7 responsibility

- On resume the M7 picks up exactly where it left off — the supervisor hands back its recent-events history (state transfer).

## Functional scope (per executor)

Three executors split the work. **PD Controller = power authority; M7 (+A53) + Sensing Supervisor =
sensing & product logic.**

### PD Controller (CYPM1111) — power & battery authority
- USB-C PD negotiation; drives the bq25601 charger; presents the battery-native **SYS** rail.
- Battery management: ship mode, dead-battery boot, charge, **telemetry** (level / time-left).
- **SoM on/off** — owns `V_SOM_EN` (the SoM's *outer* power gate, a carrier net): re-powers only when the
  battery is enough above the **3.45 V floor** for a *useful runtime*; cuts the SoM off below the
  **deep-standby reserve**. `V_SOM_EN` low is deliberate ("no battery, don't wake").
- Full **Stem-bus node** (addr 0x23); **masters the Stem bus while the SoM is off** (optional, for
  battery-aware coordination of the MCUs).
- Does **not** do sensing or product logic — only *whether and how there is power*.

### M7 (+ A53) — compute & product logic
- Runs the product when awake: M7 = real-time / always-on; A53 = Linux (vision / heavy compute).
- **Masters the Leaf/Sensor bus while alive**; masters the Stem bus while alive.
- Decides when to **suspend** (idle) and when a **full (A53) boot** is worth it.
- **Health-monitors** the MCUs (reboot + UART self-test + re-flash a bad one).
- On resume, **reads the Sensing Supervisor's recorded history** (state transfer).

### Sensing Supervisor (MSP) — sensing & wake while the SoM sleeps
- Watches sensors while the SoM is suspended/off; **records recent-events history** (readable,
  transfer-compatible with what the M7 tracks).
- Determines the SoM run-state from the **PMIC pins** → **Passive** (SoM owns the sensor bus) vs
  **Sensing** (MSP masters it).
- **Wakes the M7** (`STEM_INT`) when the SoM is **suspended but powered** and a sensor event fires.
- When the SoM is **OFF** (`V_SOM_EN` low — insufficient battery): **record-only** — does **not** try to
  wake; the SoM returns only when the PD controller re-powers on battery recovery.
- Runs at a **single rate**; acts as the SoM's **I/O-expander** (GPIO banks) when Passive.

## Technical Scenarios

- MSP can track the PMIC state and react to change. 
- MSP has a register for the state.
- MSP becomes the master when SoM is fully suspended.
- All MCUs can ask SoM to read their registers to see what they want to report
- The MCUs will log their boot state over UART so SoM M7 can verify that firmware is correct

TODO expand Technical Scenarios based on how we provide real world value.


## Test tracking (bench)

High-level status of what the bench proves, driven by the **`stem-probe` UART console**. Detail + pass
criteria + wiring live in **[MCU-BENCH.md](MCU-BENCH.md)** (the per-test matrix); this is the roadmap.
`[x]` = runs & passes today · `[~]` = partially works (electrical only / needs a target wired) ·
`[ ]` = blocked on firmware not yet written.

**Identity & addressing** *(hardware-verified 2026-10-03 via `thepia hwd console`)*
- [x] T1.1 bus census — `scan` sees exactly `0x20`/`0x21`/`0x22`, no collision (3 slaves coexist) ✓HW
- [x] T1.2 role↔silicon — `vstem` maps each address to its model id (0x830C/0x832A/0x8240), `iface=0xD0` ✓HW
- [x] T1.3 fw present — `vstem` shows a non-zero fw version on each node ✓HW

**Mode control (Stem)**
- [x] T2.1 Passive — `pass`/`vstem`: `0x3E` reads `0x00` at 0x20 ✓HW
- [x] T2.2 single-I²C refusal — FR2433 (0x22) rejects Sensing (Passive-only), via `vstem` ✓HW
- [~] T2.3 Sensing write — `sens`: `0x3E=0x01` **flaps on FR2355** (passes/fails run-to-run) — OPEN-QUESTIONS Q-SENSING-ENUM

**Leaf mastering handoff** *(needs a Leaf I²C target wired to P1.2/3 + Pico GP4/5)*
- [ ] T3.1 Sensing enumerate — **FAIL on HW**: `vleaf` reads `dev_count=0x00` though a device is present → Q-SENSING-ENUM
- [x] T3.2 Passive handoff — `vleaf`: node releases, Pico masters Leaf, sees `0x01` — one-master invariant ✓HW

**Sideband (SoM-run-status)**
- [~] T4.1 continuity — `sound0/1`,`por0/1`,`pins` toggle & read back (wiring only)
- [ ] T4.2 S1/S2 edge → mode — node acquires/releases Leaf on `SOUND_INT` edge *(FR2355 sideband fw)*
- [ ] T4.3 S3 wake — node asserts `PMIC_ON_REQ` on a wake condition, Pico reads it *(sideband fw)*
- [ ] T4.4 S4 soak — repeated edge toggles, no bus wedge *(sideband fw)*

**Product-logic contracts** *(regmap + prod firmware not yet built)*
- [ ] T5.1 POST — every boot emits a parseable pass/fail over UART (prod, not diag — OPEN-QUESTIONS Q-POST)
- [ ] T5.2 attention interrupt — any node raises it → master queries all, reads reason/detail registers
- [ ] T5.3 state transfer — supervisor's recent-events history is readable & M7-compatible
- [ ] T5.4 health monitor — master reboots a node, parses POST, re-flashes a bad one

**Console-driven automation** *(makes the above scriptable in CI)*
- [x] T6.1 generic `r <addr> <reg>` / `w <addr> <reg> <val>` — exercise any register without a new build
- [x] T6.2 `selftest` — runs T1–T3 and ends with one grep-able `RESULT pass=N fail=M` line
- [x] T6.3 transport — **`thepia hwd console interact`** is the bench path (verified live 2026-10-03:
  flashed via `thepia hwd pico flash`, ran `selftest` while hwd owned the CDC); reaches a
  **CI-runner-attached bench over the thepia mesh**. `scripts/bench-run.py` (pyserial) is the hwd-free
  fallback only. Caller parses `RESULT` for the verdict — `--expect` can't gate it (improvement filed via
  `thepia report`).

~~Known issue: legacy `read` expected FR2433 at `0x20`~~ — **resolved**: `read` retired (superseded by `vstem`).

## Clarifications & goals (captured 2026-10-03, verbatim where crafted)

**Scope — we are building a SoM SIMULATOR, not the SoM:** "Right now we are not implementing the SoM
logic, we are making a simulator with the Pico in order to test the other MCUs." The Pico (SoM-sim)
drives the M7 suspend/resume pin states and **only masters while it simulates the M7 being alive**; by
iterating the switching we measure/track the exact delays needed. (The MSP has **no logic yet** to
detect the simulated M7 suspend/resume — that's to build.)

**MSP ≠ the PD controller, but it DOES track the PMIC.** Two *different* things, don't conflate them:
- **PD controller (CYPM1111)** = battery / USB-C power delivery — the MSP is **uninvolved** ("MSP has
  nothing to do with power", Henrik 2026-10-03).
- **PMIC (PCA9450)** = the SoM's rails / run-status. "Tracking the PMIC" is **just the 4 pins related to
  SoM running status** (`SOUND_INT` / `STEM_INT` / `PMIC_ON_REQ` / `POR_B`) — which the MSP **does**
  track, to know when the SoM is alive vs suspended.

So the FR2355 4-pin sideband IS exactly that SoM-run-status tracking — legitimate, **not** power control.
(Remaining nuance: each pin's direction — which the MSP reads vs `STEM_INT` it drives — OPEN-QUESTIONS Q-PWR.)

**Sensor-bus mastership — M7 owns it when alive.** "I think the I2C proxying is more historic. When M7
is alive, it will be the Leaf/Sensor bus master." The MSP masters the Leaf/Sensor bus **only when the
SoM is fully suspended** (Sensing). So "sensor proxying over Stem" is the Sensing-mode captured history,
not continuous proxying.

**Sensing-supervisor event history + transferable state:** "The sensing supervisor would keep registers
describing recent events. It would have a history that can be read of what was sensed. It would have a
logic/state equivalent/compatible with what M7 would track, so the state can be transferred."

**The attention interrupt:** raised by **any MCU**, it causes the M7 to **query all MCUs over the Stem
bus**. Each MCU has a register reporting the **reason** and other registers for the **detail**. (Edge
vs level is immaterial.)

**Dedicated semantic registers over a generic interface:** "Since I can make dedicated registers with a
semantic meaning, the generic interface is less important. However it still makes sense to support
Linux hardware drivers. Putting I2C on M7 means that it would only be a story of Zephyr device
drivers/device tree. But the Zephyr implementation could expose an interface to Linux that also
emulates APIs for existing Device drivers."

**MCU health monitoring (M7-driven):** "The M7 monitors the health of MCUs by rebooting them and
capturing UART console. If self-test fails, it flashes the bad row/sector on the MCU." → the **prod**
firmware must run a **boot self-test (POST)** on every boot and log a parseable pass/fail over UART, so
the M7 can reboot-and-verify. **POST is part of regular/prod firmware — it is NOT `diag`.** (`diag` is a
separate, full diagnostic firmware for *manual* bench testing.) Prod today prints a boot banner + debug
dump; a structured POST the M7 can parse is a requirement — see OPEN-QUESTIONS Q-POST.

**Addressing — model+role, collision-free:** assign the I²C address from the **MCU model AND role** so
no collision is possible. Idea: two `2..7` tables used for the upper and lower 4 bits of the address; a
**secondary address** in case of a clash. (Extends the role-based `Model::stem_addr()` already shipped;
see OPEN-QUESTIONS Q-ADDR.)

See the **[docs index + reading order](README.md)**. Power model: **[POWER-STATES.md](POWER-STATES.md)**
(the S0–S5 ladder) → **[POWER-DISTRIBUTION.md](POWER-DISTRIBUTION.md)** (how each part is powered). Open
items: **[OPEN-QUESTIONS.md](OPEN-QUESTIONS.md)**.
- ...

