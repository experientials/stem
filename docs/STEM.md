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

