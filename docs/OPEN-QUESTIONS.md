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

---

## Open — I²C addressing / collision / discovery (research)

Context: Q-ADDR wants **collision-free addressing by model+role**, plus robustness if two slaves ever
share an address. First-pass answers below are from recollection — **verify against the I²C/SMBus specs
(UM10204, SMBus 3.x) before relying on them.**

### Q-ADDR · Model+role → address, collision-free
Assign the address from MCU **model AND role**; idea: two `2..7` tables for the upper/lower 4 bits + a
**secondary address** on clash. Extends `Model::stem_addr()` (role→addr, already shipped).
**Status:** OPEN (design). Next: define the two tables + the clash/secondary-address rule.

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
