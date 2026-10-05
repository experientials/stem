# Stem — open questions & future directions

Open **strategy/research questions that need further exploration**, tracked with their **resolutions**
as they land. Companion to [STEM.md](STEM.md) (usage/scenarios) and [../prod/DESIGN.md](../prod/DESIGN.md)
(the prod firmware design). Add a question the moment it's raised; move it to **Resolved** (don't delete)
when decided, with the decision + date.

---

## Open — strategy

### Q-PROV · Identity/provenance enforcement policy
On a wrong-family or mismatched-role node, does the SoM **hard-refuse** it or just **log** it?
`model::matches_build_family()` already flags a mismatch; the *policy* is undecided.
**Status:** OPEN (strategy — "needs further exploration", Henrik 2026-10-03).

### Q-PWR · MSP sideband pin directions (PMIC-tracking, NOT PD control)
**Mostly resolved (Henrik 2026-10-03):** "Tracks PMIC which isn't PD Controller, it is just the 4 pins
related to SoM running status." So the FR2355 4-pin sideband (`SOUND_INT`/`STEM_INT`/`PMIC_ON_REQ`/
`POR_B`) is **legitimate** — it's SoM-run-status tracking via the **PMIC (PCA9450)**, which is a
*different* chip from the **PD controller (CYPM1111)**; the MSP has nothing to do with the latter
(battery/USB power). The allocation is NOT wrong.
**Remaining:** nail down each pin's **direction** — which are inputs the MSP *tracks* (likely
`SOUND_INT`, `POR_B`, `PMIC_ON_REQ`) vs the MSP's **attention output** (`STEM_INT`). `model.fr2355.yaml`
currently marks `PMIC_ON_REQ` as an MSP output — revisit once the tracking-vs-driving roles are fixed.
**Status:** OPEN (pin directions only).

### Q-POST-INTEGRITY · FRAM checksums — M7 judges, MSP reports (2026-10-04)
Decided ([ADR-0010](adr/0010-integrity-m7-judges-msp-reports.md)): POST computes **per-sector CRC16**
over the program FRAM; the **MSP reports the raw CRCs over I²C** (Stem debug registers), the **M7 holds
the golden values and judges** (reflashes mismatched sectors over BSL). Fail-safe: a buggy/corrupt MSP
verifier can only cause a false "corrupt" (harmless reflash), never a false "OK". **DONE (2026-10-04, verified on FR2355 hardware):** `post::fram_crcs` computes 8× per-sector CRC16 on
**every** boot incl. the **silent** image (built at 11% FRAM — no console) — ADR-0010 always-run. The
CRCs are exposed over I²C at **`DBG_CRC` = 0x3F**, a windowed register (write byte-index 0..15, read that
CRC byte; see I2C-API.md + regmap.rs). Verified: idx0..4 read `E8 08 81 FA A7`, matching the console's
`E808 81FA A7…`. The MSP only reports (write just moves the cursor; data is read-only); the M7 judges.
Console (`console`/`--debug`) echoes the same values. **Remaining (future):** the M7 side (golden-CRC
compare + per-sector BSL reflash); CRC computation cost (~0.3 s at 1 MHz over 32 KB) is a boot one-time.

### Q-POST · Prod needs a structured boot self-test (POST) for M7 health monitoring
The M7 health-monitors each MCU by **rebooting it and reading the UART boot self-test**, and reflashes
the bad row/sector on failure (STEM.md). So the **prod** firmware must run a **POST on every boot** and
emit a **parseable pass/fail** over UART. **POST ≠ `diag`** — `diag` is the separate, full *manual*
diagnostic. Prod today only prints a boot banner + debug-window dump (and the loopback self-test is
Sensing-gated, so it won't run under the new Passive default). **Status:** OPEN (firmware gap) — define
the POST checks + a machine-parseable PASS/FAIL line the M7 can match on.

### Q-STATEPINS · Do the PMIC pins tell the MCUs the system state? (mostly RESOLVED 2026-10-03)
**The three PMIC signals `PMIC_ON_REQ` + `PMIC_STBY_REQ` + `POR_B` fully encode the SoM power state**
(PCA9450DS p6–16): OFF/SNVS → PWRUP → RUN (ON high, STBY low) → STANDBY/DSM (ON high, STBY high) → PWRDN.
- **Sufficient for the MSP's job — YES:** RUN ⇒ M7/A53 owns the sensor bus ⇒ **Passive**; STANDBY/OFF ⇒
  SoM suspended ⇒ **Sensing**. `PMIC_STBY_REQ` directly flags DSM entry.
- **NOT the full system state — two blind spots:** (1) *why* the SoM is off — forced-suspend-low-battery
  (S2) vs idle deep-standby (S3) are identical on these pins (PD-controller/battery context, not here);
  (2) **M7-alive (S4) vs full-run (S5)** — both are PMIC **RUN** (AN13400), the A53/M7 split is internal
  to the GPC. Covered instead by: A53/M7 + rich context over the **Stem I²C** while the SoM is alive (pins
  are the fast-path for when it's suspended); battery context from the **PD controller**.
- **RESOLVED (Henrik 2026-10-03):** the Sensing supervisor **runs at a single rate** — no battery-level
  behaviour — so the **PMIC pins + Stem I²C are sufficient**; the MSP needs nothing from the PD
  controller. The **"out"** if a battery-aware rate is ever needed: the **PD controller masters the Stem
  I²C while the SoM is off** (becomes the Stem master in the SoM-off regime, commands the MCUs by battery
  level — **no extra wire**, uses the existing bus + role-based protocol; requires the Stem bus to reach
  the power module). See [POWER-STATES.md](POWER-STATES.md) "How the MSP determines state".
- **Refines Q-PWR:** the MSP's SoM-state tap should be `PMIC_ON_REQ` + `PMIC_STBY_REQ` + `POR_B` (the
  real state machine); `SOUND_INT` is likely redundant with `PMIC_STBY_REQ`.

### Q-PERSIST · What persists vs is transient, per MCU
Henrik (2026-10-03): "It sounds like a question about what to persist and what is transient in each MCU.
I'm not psychic, we will have to find out as we implement." (`INIT_CODE`/FRAM persistence is a candidate,
not a decision.) **Status:** OPEN — resolve empirically during implementation.

### Q-SENSING-ENUM · FR2355 Sensing mode flaps + Leaf enumerate returns 0 on hardware
> **REOPENED & NARROWED 2026-10-04 (a premature "RESOLVED" below is corrected) — two layers: (1) a bench
> pull-up issue that fixed the IDLE level, and (2) a deeper FR2355-LOCAL SDA DATA-PATH fault that only a
> connected device reveals, speed-independent.** When an I/O extender (PCA9698 @0x20) is on the Leaf, the
> FR2355 cannot exchange valid data with it at ANY clock. A diag write-verify sweep (0xA5 to the PI reg,
> ladder 10 kHz→1 MHz; run via the new console `m` command) gives **`0 nack, 200/200 corrupt` identically
> from 10–500 kHz** (1 MHz: 24 nack early-out). 10× RC margin changes nothing ⇒ **NOT rise-time, NOT
> pull-up strength, NOT the extender** (the Pico bit-bang reads 0x20 cleanly; the FR2476 reads its Leaf
> device on byte-identical firmware). The FR2355 reads **SDA low whenever it samples mid-transaction**
> though SDA idles H ⇒ a hardware defect on the FR2355's **SDA / P1.2 / UCB0SDA** path specifically (open
> to the shared SDA net, a low-R load/short, a LaunchPad isolation element on P1.2, or a damaged pin).
> Firmware re-exonerated by the gotcha checklist (UCSSEL=SMCLK, PM5CTL0=0, P1SEL0=0x0C/SEL1=0, **P1REN=0**
> so no internal pull-down, UCSWRST order, 9-clock recover present). **Decisive physical tests (no scope
> needed):** (a) continuity FR2355 P1.2↔extender-SDA (~0Ω) and P1.2↔GND (must be OPEN); (b) move the
> extender to the FR2476 Leaf — reads it ⇒ FR2355 board bad; (c) inspect FR2355 LaunchPad P1.2
> jumpers/isolation (slau680). **The pull-up finding below stands only for the IDLE level**, not the data
> path. Tooling added: diag console input (`p`/`m`/`s` keys via shared board UART-RX) + low-end ladder
> steps + extender write-verify target ([diag/src/stress.rs], [diag/src/tasks.rs]).
>
> ---
> *Superseded (kept for the record — this is the premature RESOLVED; it explains only the idle level):*
> **pull-up STRENGTH (weak RP2040-internal → discrete), NOT placement, NOT wire
> capacitance, NOT an FR2355 board/silicon fault.** The operator replaced the bench's reliance on the
> **RP2040 internal pull-ups** (weak, tens of kΩ) with **discrete resistors on the SAME (Pico) side** —
> location unchanged. The FR2355's own P1.2/P1.3 now idle `H/H` (debug build `CD1BBBCB`, `leaf-dbg:
> SDA=H SCL=H`), and when it **actively masters** in Sensing (UCB0 `MST=1 SWRST=0`) the scan is **clean**:
> `bus SDA=H SCL=H`, `fault=0x00`, `I2C bus: (none) [0 found]`. The 112-all-ACK fault is **gone**.
> `0 found` is correct — nothing is connected; a healthy, empty, properly-pulled-up open-drain bus.
>
> **Correct mechanism (the trail got this wrong twice; even the first RESOLVED draft did — record it
> straight):** the ONLY variable that changed is **pull-up resistance**. Because the resistor stayed on
> the Pico side, **placement is irrelevant** (scrap the "move it to the pin / bad joint" idea) and so is
> **wire capacitance** for the idle level (a capacitor can't hold a node low at DC; 10 cm ≈ 10–15 pF vs
> I²C's 400 pF budget — noise). Lower resistance fixes BOTH symptoms:
> - **Steady idle `L`:** a pull-up can only sit low at DC if something sinks current on that branch. A
>   weak ~50 kΩ internal pull-up loses the divider to a modest DC load/leakage on the FR2355 Leaf branch
>   (idle-L through 50 kΩ ⇒ branch load ≲ ~20 kΩ to GND); a few-kΩ discrete pull-up overpowers it (~2.7 V
>   ⇒ reads H). *The ~20 kΩ branch load is INFERRED from the idle-L reading, not yet measured — meter
>   P1.2/P1.3→GND (power off) to confirm, or rule it "internal pull-up simply not enabled in that state".*
> - **112-all-ACK during probing:** genuine RC rise-time, but against **total bus capacitance**
>   (~100–200 pF), never the 10 cm wire. τ = 50 kΩ × 150 pF ≈ 7.5 µs > the 5 µs SCL-high window at
>   100 kHz ⇒ SDA under-risen when the master samples ACK ⇒ false ACK on all 112 addresses. A few-kΩ
>   pull-up gives τ < 1 µs ⇒ correct sampling.
>
> **Superseded:** the "weak pull-up × capacitance → mis-sample" framing (half-right: RC yes, but the idle
> level is DC, and never the wire), the "FR2355-specific hardware fault / PROVEN by elimination" verdict
> (the FR2476 comparison was confounded — it had an OLED breakout carrying its own pull-ups), AND the
> first-draft "DC path / move resistor to the pin" account (resistor never moved). The durable fact:
> MSP430 eUSCI has **no internal pull-ups**; adequate *external* pull-ups (right resistance for the bus
> speed + capacitance) are mandatory, and the bench was under-pulled. The firmware fix that DID matter is
> separate and stands: [ADR-0006](adr/0006-mode-switch-must-not-block-control-channel.md) (mode switch
> must not block the Stem control channel). *The chase trail below is kept verbatim; read it knowing this
> conclusion supersedes the capacitance/rise-time, hardware-fault, AND placement claims in it.*

**Found on the bench 2026-10-03** running `selftest` via `thepia hwd console` (Pico @0x20/0x21/0x22 all
present). Two back-to-back runs gave **different** verdicts — `RESULT pass=6 fail=1` then `pass=5 fail=2`:
- **`mode -> Sensing` (write `0x3E=0x01`, read back) is non-deterministic** on the FR2355 — passes one
  run, fails the next. T1/T2 identity + Passive are rock-solid; only the Sensing transition flaps.
- **`Sensing dev_count=0x00` every run** — the FR2355 enumerates **zero** Leaf devices in Sensing, yet in
  **Passive** the Pico masters the same Leaf bus and finds **`0x01`** device. So a device *is* on the bus;
  the node isn't seeing/enumerating it when it masters.

**Chased 2026-10-03 via the FR2355's own UART console (`thepia hwd console -b @fr2355`) — root cause
identified; two defects, one firmware + one bench.**

**CONFIRMED (directly observed on the FR2355):**
- **`dev_count=0` is a BUS FAULT, not an empty bus.** The node's debug regs read **`fault=0x01`** and its
  scan reports **"!! BUS FAULT (SDA stuck / all-ACK) — scan unreliable [112 ACKs]"** when it masters the
  Leaf (UCB0 P1.2/P1.3) in Sensing. All 112 of 0x08–0x77 "ACK" → the `SANITY_MAX` guard
  ([enumerate.rs](../prod/src/enumerate.rs)) correctly collapses it to **0 real devices**. So the Stem
  `dev_count=0` is faithfully reporting a wedged bus.
- **It is induced/latched, not static wiring.** Immediately after the FR2355 reflash its first Sensing
  scan was **clean** (`I2C bus: (none) [0 found]`, `fault=0x00`); the 112-all-ACK fault appeared only
  **after Leaf-bus mastering activity** (the Pico's `vleaf` step), and then persisted.
- **The mode-readback FLAP is a main-loop stall, not a lost I²C write.** `apply_mode()`
  ([main.rs](../prod/src/main.rs) ~324) runs `enumerate::scan()` — bus **recovery** + **112 probes** —
  **inline in the service loop**. While it blocks, `stem::poll` isn't servicing the Stem slave, so the
  Pico's 200 ms readback races it: clean/fast scan ⇒ readback lands (PASS), faulted/slow scan ⇒ misses
  (FAIL). The flap tracks exactly whether the scan faults.
- **The FR2355 UART console drops keystrokes in Sensing** — same cause: the loop is stuck in the
  blocking faulted scan/recovery, starving UART RX polling. (The Stem slave survives only because the
  eUSCI_B1 **hardware** ACKs/clock-stretches; the software-polled UART does not.)

**ROOT CAUSE — two defects:**
1. **Firmware (the real bug):** a **mode switch must not block the control channel.** Boot was fixed to
   bring the Stem slave up first (main.rs:152–156), but the **runtime Sensing switch re-introduces the
   stall** by scanning inline. Fix options: time-slice/async the scan, hard-cap `i2c::recover`
   iterations, or run enumerate incrementally between `stem::poll` calls so the slave + console stay live.
2. **Bench (the trigger):** the Pico's I²C0 Leaf master is **permanently wired to the shared Leaf bus**
   and is not electrically detached when the FR2355 goes Sensing → a genuine **two-master-on-Leaf**
   exposure (the invariant the bench itself warns about). Fix: the Pico must tri-state GP4/GP5 (deinit
   I²C0 to high-Z) whenever it is not the Passive-mode Leaf owner, and bus-recover before handing off.

**UNCERTAIN (couldn't cleanly confirm — console flakiness in Sensing):** whether the Pico's I²C0
*specifically* is the agent holding the bus (a `pico reset` test was inconclusive — the FR2355 console
stopped reporting), and the exact electrical mechanism (idle SDA reads **H** yet every probe ACKs —
not a textbook stuck-low; points at interference during the ACK window rather than a hard short).
**UPDATE 2026-10-03 — firmware fix #1 landed ([ADR-0006](adr/0006-mode-switch-must-not-block-control-channel.md)), verified on HW:**
- The runtime mode switch no longer runs the Leaf scan inline — it does the fast bus handoff, publishes
  the mode immediately, and runs the enumerate **incrementally** (`enumerate::ScanJob`, `begin_mode`/
  `pump_scan` in prod/src/main.rs). Flashed to the FR2355 (build `24889BEF`).
- **Sensing-readback flap: FIXED** (6/6 stable). **Console in Sensing: responds** (was fully starved).
- **Residual (faulted bus only):** a `pass` write can still be dropped (mode stuck `0x01`, ~4/6 when
  `fault=0x01`; solid when `fault=0x00`). The stall shrank from the whole scan to a probe *batch*, but the
  slave still isn't serviced **inside a probe's `SPIN`** → RX-overrun on a slow (faulted) probe.

**UPDATE 2026-10-03 — bench fix (B) implemented, and it DISPROVED the two-master hypothesis:**
- `stem-probe` now owns GP4/GP5 as **bit-banged open-drain GPIO** (`leaf_scan`/`leaf_park`), parked Hi-Z
  + pull-up when idle, driven only during its Passive-owner scan — so the Pico is **never a hardware I²C
  controller** on the Leaf bus. The Pico's own Passive scan finds the device cleanly (**1** ACK). ✅ B is
  correct hygiene (upholds one-master) — **keep it** — [ADR-0009](adr/0009-bench-bitbang-leaf-bus.md).
- **But the FR2355 Leaf fault PERSISTS** with the Pico merely parked: `fault=0x01`, 112 all-ACK, in
  Sensing. And it faults on the **FIRST Sensing scan after a clean `msp reset`** (no prior Leaf activity).
  So it is **not** induced by the Pico-as-controller and **not** a latched-stuck-device-after-access.

**Revised root cause (leading, needs a PHYSICAL check — can't resolve in firmware):** the FR2355's Leaf
pins **P1.2/P1.3 are not on the same healthy net the Pico sees.** The Pico (GP4/GP5, with its pull-ups)
finds a single well-behaved device; the FR2355 mastering P1.2/P1.3 sees **SDA stuck low → 112 all-ACK**.
MSP430 eUSCI has **no internal pull-ups**, so if P1.2/P1.3 are unconnected to the pulled-up device net
(or shorted/held low on the LaunchPad side), the FR2355 sees exactly this. A healthy shared bus would
show **1** ACK on the FR2355 too, not 112.

**CLEAN BASELINE CONFIRM 2026-10-04 — consistent firmware, apples-to-apples.** Reflashed all 3 MSP430s
to the SAME current prod (mode=passive; verified running over Stem: iface=0xD0, scan 0x20/0x21/0x22) —
removing the earlier version-skew confound (FR2476 had been on a stale `ddef94e` sensing-default build;
that invalidated the first cross-board comparison). Then checked each Leaf (Sensing, read over Stem):
**FR2476 `fault=0 dev_count=1 known=1`** (finds + recognizes its device/OLED — Leaf works);
**FR2355 `fault=0 dev_count=0`** (clean scan, finds nothing — Leaf blind); FR2433 correctly refuses
Sensing (Passive-only). Same firmware, FR2476 Leaf works, FR2355 Leaf blind ⇒ **FR2355 Leaf is the
hardware fault** (P1.2/P1.3 / eUSCI_B0). Final airtight check if wanted: swap the two boards' Leaf
devices — the device following the FR2476 and not the FR2355 confirms the board.
**Console follow-up (prod/DESIGN.md, Henrik 2026-10-04):** default prod should log **version +
row/sector checksums**; a **--debug** build adds post-POST "what was found" basic info (not a full diag).
Today the default `console` is the verbose one and there's no MSP430 `--debug` class — restructure TODO.

**FIRMWARE EXONERATED 2026-10-04 (datasheet-verified) → fault is HARDWARE on this FR2355's Leaf.** With
the OLED **powered** (VCC/GND on-board) on P1.2/P1.3, the FR2355 scans **clean (`SDA=H SCL=H`) but gets
NO ACK** (`dev_count=0`). And the FR2355 datasheet **Table 6-63 Port P1 Pin Functions** confirms
`P1.2 UCB0SDA` and `P1.3 UCB0SCL` are both selected at **P1SELx = 01** — exactly what the firmware writes
(`P1SEL0|=0x0C, P1SEL1&=~0x0C`). So mux ✓, clock ✓ (UART readable), analog ✓ (none), pull-ups ✓,
thepia ✓, and identical fw works on FR2476 + Pico. **Conclusion: the FR2355's Leaf path is physically
faulty** — its P1.2(SDA)/P1.3(SCL)/eUSCI_B0, or the OLED's two wires swapped (P1.2=SDA, P1.3=SCL).
**Decisive test: move the OLED to the FR2476 Leaf** — sees it ⇒ swap the FR2355 board; doesn't ⇒ OLED
wiring. No more firmware to change.

**OLED test 2026-10-04 — isolated to the FR2355's eUSCI_B0 Leaf master (or OLED power).** Operator wired
a known-good I²C OLED directly to the FR2355 Leaf (P1.2/P1.3). Result: FR2355 Sensing still `dev_count=0`
(flickering fault); Pico `lscan` sees ONLY `0x20` (not the OLED) → the OLED is on the FR2355's own
isolated bus. Asymmetry on identical firmware: FR2355 **Stem slave** (eUSCI_B1) ✅, **console UART**
(eUSCI_A1) ✅ (so SMCLK≈1 MHz — clock is fine), **Leaf master** (eUSCI_B0 P1.2/3) ❌; FR2476 Leaf master ✅;
Pico bit-bang ✅. So **only the FR2355's eUSCI_B0-on-P1.2/P1.3 fails.** Remaining causes: (1) OLED not
powered (VCC/GND, needed for its pull-ups) — CHECK FIRST; (2) this FR2355 board's P1.2/P1.3/eUSCI_B0
faulty — decisive test: move the OLED/extender to the **FR2476 Leaf** (proven master); sees it ⇒ FR2355
board bad (swap), doesn't ⇒ OLED wiring/power. Firmware/PAC/mux/clock/thepia all exonerated.

**NARROWED FURTHER 2026-10-04 — fault gone on reconnect; now a per-line connectivity issue.** After the
operator reconnected the FR2355 Leaf: (1) FR2355 Passive → Pico `lscan` still sees `0x20` (tri-stated
FR2355 doesn't disturb the bus); (2) FR2355 Sensing → **`fault=0x00` (stable, the 112-all-ACK is GONE —
so the earlier fault was a bad/intermittent connection) but `dev_count=0x00`** (finds nothing) while the
Pico sees 0x20 on the SAME bus. A clean-but-blind scan = **SDA and SCL not both joined to the shared
net** (if only one line reaches the extender, no valid address match → no ACK → 0 devices, no fault).
**Check per line:** FR2355 J1.10(P1.2/SDA)↔extender-SDA↔Pico-pin6; J1.9(P1.3/SCL)↔extender-SCL↔Pico-pin7;
whichever doesn't beep is it (suspect the SDA/P1.2 leg, since it's clean not stuck-low).

**PROVEN 2026-10-04 — Leaf bus + I/O extender + Pico all healthy; fault isolated to the FR2355 board.**
With the FR2355's P1.2/P1.3 **physically disconnected** (only Pico + extender on Leaf), the Pico's
bit-bang master (`lscan`/`lr`, new stem-probe cmds) **reads the I/O extender fine**: `lscan` → `0x20`;
`lr 0x20 0x00`→`0xFF`, `0x01`→`0xFF` (Input Port regs, PCA9698-default all-high). So the bus, the extender
(@0x20 — the PCA9698 strap group; this was also the FR2476's "0x20" hit, NOT a Stem cross-wire), and the
Pico path are all good. **The Q-SENSING-ENUM fault is therefore specific to the FR2355's own Leaf
interface (its P1.2/P1.3 / that LaunchPad), now proven by elimination.** Next if pursued: reconnect ONLY
the FR2355 Leaf and investigate that board (its P1.2/P1.3 solder/jumpers/pin health) — everything else is
cleared.

**10 kHz experiment 2026-10-04 (read reliably over Stem — DBG_FAULT 0x3A / DEV_COUNT 0x38):** slowed the
FR2355 Leaf eUSCI to 10 kHz (brw 10→100, reverted after). The hard fault went **intermittent**
(`fault=0x00` one read, `0x01` the next) vs always-on at 100 kHz → part of the 100 kHz all-ACK *was*
edge/rise-time. **But on a CLEAN scan (`fault=0x00`) `dev_count=0x00`** — the FR2355 finds **zero
devices**. So there is **no I²C slave on the FR2355's P1.2/P1.3 net**. Note also: analog-override ruled
out by inspection (prod touches no SAC/OA/ADC; reset = digital); eZ-FET backchannel is P4.2/P4.3 (not
P1.2/P1.3, per slau680). Suspicious clue: the FR2476's "Leaf" scan found **0x20** = the FR2355's *Stem*
address → Leaf and Stem buses may be **bridged/cross-wired**, or there is no real Leaf sensor. **Open
(topology, needs the operator): what device is actually on the Leaf bus + at what address; is Leaf
shorted to the Stem net; multimeter-continuity FR2355 J1.10(P1.2)↔device-SDA, J1.9(P1.3)↔device-SCL.**

**BREAKTHROUGH 2026-10-04 — the fault is FR2355-SPECIFIC; firmware + pull-ups EXONERATED.** The **FR2476**
(0x21, commanded to Sensing via the Pico's generic `w 0x21 0x3e 01`) runs the **identical firmware and
P1.2/P1.3 UCB0 scan code** and scans its Leaf **cleanly**: `fault=0x00`, `bus SDA=H SCL=H`, `I2C bus:
0x20? [1 found]`. On the same code the FR2355 gives 112-all-ACK. So: **not the shared scan/mode code,
not the SEL/mux value, not a weak pull-up** (the FR2476 works on the same internal pulls, no external
resistors — the 6.8 kΩ pull-up chase was a detour; `SDA/SCL` measure 3.3 V at idle, bus healthy at DC).
The defect is **specific to the FR2355 mastering its own P1.2/P1.3** — its LaunchPad routing / an onboard
load or jumper on P1.2/P1.3 / that board's bench wiring / FR2355 silicon-pin specifics. **Next: inspect
the FR2355 LaunchPad P1.2/P1.3** (slau680 schematic — jumpers/LEDs/isolation), confirm its Leaf wires are
on the SAME net the FR2476 reads (device @0x20), and scope FR2355 P1.2/P1.3 during a Sensing scan vs the
FR2476's (working) scan. The pre-2026-10-04 hypotheses below are superseded by this.

**NARROWED 2026-10-03 (disconnect + continuity + restart) — ruled OUT Pico-as-driver and miswire:**
- Pico Leaf pins physically **disconnected** → FR2355 **still faults** (`fault=0x01`). So the Pico is not
  the cause (not a driver/contention issue). Pico's own scan then found **0** (the device sat on the
  Pico-reachable net).
- Operator **verified board-to-board continuity** (FR2355 P1.2/P1.3 ↔ net) — **not a miswire**; **bench
  restarted** (fresh).
- Post-restart FR2355 Sensing scan: **`bus SDA=H SCL=H` at idle, but `112 all-ACK` during probing.**

**This signature IS the diagnosis — pull-ups too weak for the bus SPEED.** Idle (seconds) → lines reach
H, so pull-ups exist but are weak. During a 100 kHz probe, after the FR2355 releases SDA for the ACK bit,
SDA can't rise fast enough (weak pull-up × bus capacitance) before the master samples it at the SCL-high
edge (~µs later) → sampled low → **false ACK on every address → 112 all-ACK**. The Pico's **slow
bit-bang** (tens of kHz, long bit periods) gives SDA time to rise → it reads the device fine; the FR2355
eUSCI at 100 kHz does not. Matches the earlier intermittency too (marginal rise time).

**LEADING HYPOTHESIS 2026-10-03 — weak Leaf-bus pull-ups (speed/strength asymmetry, not wiring):** the
bench Leaf bus relies on the **RP2040 internal pull-ups (~50–80 kΩ)** — there are no external pull-ups,
and MSP430 eUSCI has none. That is adequate for the Pico's own **slow bit-bang** scan (tens of kHz → it
finds the device cleanly), but marginal-to-inadequate for the **FR2355 eUSCI mastering at 100 kHz**: SDA
can't rise fast enough, so the FR2355 samples it low on every ACK → **112 all-ACK / "SDA stuck"**. This
explains BOTH the Pico-vs-FR2355 asymmetry AND the intermittency (marginal pull-up faults depend on
timing/temperature — matches `fault=0` some runs, `fault=1` others). Board = **RP2040 Pico H (2021)** —
standard 40-pin pinout; GP4=pin6, GP5=pin7.
- **Fix to try first:** add **external 4.7 kΩ (2.2–10 kΩ) pull-ups** from Leaf SDA & SCL to 3.3 V. If the
  FR2355 `fault` bit clears and `dev_count` becomes real, this was it.

**Also verify (physical):**
1. Wiring — FR2355 **P1.2↔Pico pin 6 (SDA) / P1.3↔Pico pin 7 (SCL)** + device + common GND. **GP≠header
   pin**: GP4/GP5 are physical pins **6/7**, not 4/5 — a classic miswire that would float the FR2355.
   Check the FR2355 LaunchPad for anything on P1.2/P1.3 (jumpers/LEDs). Scope P1.2/P1.3 in a Sensing scan.
2. Only if the bus is confirmed healthy (good pull-ups + wiring) and it still faults: revisit the FR2355
   `acquire_sensor_bus` pin-mux (P1SEL for UCB0) / `i2c::recover`.

**Firmware residual (separate, tracked in [ADR-0006](adr/0006-mode-switch-must-not-block-control-channel.md)):**
the mode-readback still flaps while the bus faults (slow probes → write loss). Independent of the wiring
cause above; its belt-and-suspenders fix is pumping the Stem slave during Leaf probes. Lower priority.

Evidence in the `@fr2355` console timeline (annotated).

---

## Open — I²C addressing / collision / discovery (research)

Context: Q-ADDR wants **collision-free addressing by model+role**, plus robustness if two slaves ever
share an address. First-pass answers below are from recollection — **verify against the I²C/SMBus specs
(UM10204, SMBus 3.x) before relying on them.**

### Q-ADDR · Model+role → address, collision-free
Assign the address from MCU **model AND role**; idea: two `2..7` tables for the upper/lower 4 bits + a
**secondary address** on clash. Extends `Model::stem_addr()` (role→addr, already shipped).
**Status:** OPEN (design). Next: define the two tables + the clash/secondary-address rule.
**NOTE (2026-10-04, found on the bench):** the role map put the **supervisor at 0x20** — which is also
the **default strap of a PCA9698-class I²C I/O expander** (0x20–0x27 group). On the bench they coexist
only because they're on different buses (FR2355 supervisor @0x20 on **Stem**; a real extender @0x20 on
**Leaf** — verified a real chip: its reg 0x30 reads 0x00, not the prod 0xD0 magic). On the product, if an
MSP emulates the expander or a real expander shares the bus, **0x20 collides**. Factor PCA9698 strap
addresses into the Q-ADDR scheme (avoid 0x20–0x27 for roles, or reserve them).

### Q-COLLIDE · Can an I²C slave detect another slave at the same address?
**First pass:** Not reliably in plain I²C. During address+ACK both slaves pull SDA low → indistinguishable
to master *and* slaves. During a slave-**read** (slave transmitting), it's wired-AND on SDA, so a slave
sending `1` while another sends `0` *could* detect the mismatch if it monitors SDA (like master
arbitration-loss) — but most slave peripherals don't. **The standardized fix is SMBus ARP** (128-bit
UDID, bitwise arbitration over the General Call to hand out addresses). **Status:** OPEN — verify; decide
if SMBus-ARP-style UDID arbitration is worth it vs the model+role tables.

### Q-BCAST · Can MCUs listen to a "broadcast" without answering?
**First pass:** I²C **General Call** (address `0x00`) = broadcast that enabled slaves ACK. Listening
**without** ACK = a bus-monitor/**snoop** mode (receive-only, no ACK) — non-standard, MCU-dependent
(some parts support listen-only). **Status:** OPEN — confirm eUSCI_B / RP2040 support a no-ACK snoop
and whether General Call fits the "query all MCUs" model.

### Q-DISCOVERY · Random-delay slave discovery protocol?
**First pass:** No standard *random-delay* I²C discovery. The standardized analogs are **SMBus ARP**
(deterministic UDID bit-arbitration) and **1-Wire Search ROM** (deterministic binary-tree walk of 64-bit
IDs). A random-backoff scheme (ALOHA/CSMA-style: slaves answer a discovery broadcast after a random
delay, master ACKs the first, collisions garble → retry) is **implementable but custom**. **Status:**
OPEN — research; likely unnecessary if model+role addressing (Q-ADDR) guarantees no clash.

---

## Deferred (relevant later, out of scope now)

- **Anti-counterfeit / genuine-module auth handshake over Stem** — "currently out of scope, but relevant
  later on" (Henrik 2026-10-03).

---

## Resolved

- **Role-based Stem addressing** — address chosen per model's role (`Model::stem_addr()`: FR2355/FR2155
  0x20, FR2476 0x21, FR2433 0x22). Canonical: [../I2C-API.md](../I2C-API.md). *(2026-10-01; proven on
  hardware 2026-10-03 — all three answer their role address.)*
- **Default boot mode = Passive** (was Sensing) — SoM owns the buses at boot; MSP stays I/O-extender
  until commanded Sensing. Also prevents a boot-time two-master Leaf fight. `mode::default_boot`,
  DESIGN.md Boot step 3. *(2026-10-03.)*
- **Stem slave comes up BEFORE the Leaf enumerate** — the SoM's control channel must be reachable
  instantly regardless of sensor-bus state. `run_dual` reorder, DESIGN.md Boot step 0. *(2026-10-03.)*
- **MCUs are independent of the PD controller** — no handshake; the PD controller owns power. See memory
  `mcus-independent-no-handshake` + STEM.md. *(2026-10-01.)*
- **Q-SENSING-ENUM — NOT fully resolved (an earlier bullet here claimed it was; corrected).** Two layers:
  (1) weak bench pull-ups made the IDLE bus read low — discrete pull-ups fixed that (idle `H/H`, empty
  scan clean); (2) with a real device (PCA9698 @0x20) the FR2355 still can't transact — a diag rate sweep
  shows `0 nack, 200/200 corrupt` from 10 kHz–500 kHz (speed-independent ⇒ not rise-time), while the Pico
  and FR2476 use the same bus fine. ⇒ an **FR2355-local SDA/P1.2 data-path hardware fault**, firmware
  exonerated (checklist incl. P1REN=0). Still OPEN pending physical tests (continuity P1.2↔SDA & P1.2↔GND;
  move extender to FR2476 Leaf). ADR-0006 stands. See the Q-SENSING-ENUM block above. *(2026-10-04.)*
