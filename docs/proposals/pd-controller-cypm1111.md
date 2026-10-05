# Proposal: PD controller (CYPM1111) — hardware hookup & firmware implementation plan

Status: **proposal** (implementation plan). Self-contained hand-off for bringing up the **292 power
module's PD controller / power brain**, the Infineon **CYPM1111 (EZ-PD PMG1-S1)**.

**Derived from — read these for the *why*; this doc is the *how*:**
- Chip selection + firm design rules (canonical): [`talkihw/Power Module/5V-USB-power-eval/DECISIONS.md`](../../../talkihw/Power%20Module/5V-USB-power-eval/DECISIONS.md) and `REQUIREMENTS.md`.
- Stem node + addressing: [ADR-0002](../adr/0002-role-based-stem-addressing.md) (addr **0x23**), [I2C-API.md](../../I2C-API.md).
- Independence from the MSP supervisor: [ADR-0003](../adr/0003-mcus-independent-of-pd-controller.md).
- Rail architecture: [ADR-0007](../adr/0007-battery-native-sys-rail.md), [ADR-0008](../adr/0008-tap-sys-never-raw-bat.md).
- Off/storage state: [ADR-0011](../adr/0011-ship-mode-is-the-off-state.md).
- Power hookup detail: [POWER-DISTRIBUTION.md](../POWER-DISTRIBUTION.md).
- Toolchain / "C island" / field-update: [STEM-DIRECTION.md](../../STEM-DIRECTION.md) (§C-island, §Field-updatable firmware).

**Scope:** bench-V1/V2 = **PD sink + 1S charge + power-path/DPM + ship mode + dead-battery boot +
Stem-bus node**. Custom USB-C **alt-mode / T-USB is a future upgrade path — out of scope here**
(eval REQUIREMENTS).

---

## 1. Summary

The CYPM1111 is the **power-logic brain** of the 292 module: an Arm Cortex-M0 PD MCU that negotiates the
USB-C PD **sink** contract (5 V/3 A), drives the **bq25601** 1S charger over I²C, owns the battery/SYS
rail, resolves orientation for the external SuperSpeed mux, and participates on the **Stem bus as node
0x23** (mastering it while the SoM is off, ADR-0003). It replaces the **EOL/NRND TPS65988** on 292.

Firmware is **ModusToolbox / C** — a deliberate **non-Rust "C island"** (STEM-DIRECTION §C-island) so we
ride Infineon's proven PD stack rather than reimplementing PD in Rust (speed-first; solo engineer). It is
a Stem node by **bus behavior + the language-neutral register contract**, not by sharing the Rust
codebase.

**Bench-first:** bring up on **CY7111** (PMG1-S1 kit, onboard KitProg3) + **BQ25601EVM-877**, per the
eval bench BOM, before the 292 schematic exists.

---

## 2. Hardware hookup

### 2.1 Product — 292 module (derive the schematic from this)

Block (per POWER-DISTRIBUTION.md):
```
USB-C ──▶ CYPM1111 (PD ctrl) ──drives external P-FETs──▶ VBUS ──▶ bq25601 (NVDC) ──▶ SYS ──▶ (B2B) carrier/SoM
                      │                                              │
                      ├── I²C ──────────────────────────────────────┘ (IINDPM / charge I / ship mode / telemetry)
                      ├── GPIO ─▶ external SS redriver-mux (orientation/enable)   bq25601 ──▶ BAT ──▶ 18650
                      └── Stem bus (0x23) + V_SOM_EN ──▶ (B2B) carrier
```

Firm hookup rules (from eval DECISIONS "Firm design requirements" — carry into the 292 schematic):
| Net / choice | Rule | Source |
|---|---|---|
| **System fed from SYS, never raw BAT** | so ship mode can disconnect it; preserves power-path + dead-battery boot | ADR-0008, eval |
| **CYPM1111 supply** | direct off **SYS or VBUS** (2.75–5.5 V; VBUS to 21.5 V via HV LDO) — no regulator | POWER-DISTRIBUTION |
| **CYPM1111 ↔ bq25601** | **I²C** (CYPM1111 is master): set IINDPM, charge current, **BATFET ship mode**, read telemetry | eval D1/D5 |
| **External P-FETs** | CYPM1111 drives the VBUS provider/consumer P-FETs (budget FETs + gate drive) | pmg1-s1 DS |
| **VSYS_MIN** | set toward **3.68 V** (bq25601 max) for margin over UCM 3.45 V floor | eval D1 |
| **Charge current** | **1–2 A (0.5–1C)**, sized to cell — *not* 3 A (that's the input/power-path spec) | eval D5 |
| **VDDIO (single domain)** | one I/O voltage; **level-shift** the mismatch (1.8 V i.MX I²C vs SS-mux control level) | eval, pmg1-s1 DS |
| **External SS redriver-mux** | required (TUSB1046/HD3SS460 class); CYPM1111's integrated mux is **USB2-only** | eval, pmg1-s1 DS |
| **QON** | button/line → ship-mode exit (re-enable BATFET) | ADR-0011 |
| **B2B to carrier** | SYS rail (paralleled contacts), Stem bus, `V_SOM_EN`, USB2 + oriented USB3 SS pair; **Talki = flex, Ziloo/Bob = Hirose B2B** | POWER-DISTRIBUTION, eval |

> The 292 product schematic does **not exist yet** (docs/README "Key external references"). This plan is
> the input to drawing it.

### 2.2 Bench rig — CY7111 + BQ25601EVM-877 (first implementation)

Wiring (eval bench BOM): `PD charger → CY7111 USB-C in → negotiates 5 V/3 A → CY7111 VBUS out →
BQ25601EVM VBUS → SYS → electronic load`; `BQ25601EVM BAT → 18650`; `CY7111 I²C → BQ25601EVM I²C`
(CYPM1111 drives the charger + commands ship mode); **QON button on the EVM = ship-mode wake**. Add an
I²C level-shifter if CY7111 VDDIO ≠ EVM I²C pull-up rail. Measure VBUS V/I (inline meter) and ship-mode
µA (PPK2 / µA DMM). Optional: TI **EV2400 + BQSTUDIO** to poke bq25601 registers before firmware exists.

---

## 3. Firmware (ModusToolbox / C)

### 3.1 Structure
- **ModusToolbox project** targeting `TARGET_PMG1-CY7111` (BSP on GitHub), Cortex-M0 `thumbv6m`, built with
  Infineon's EZ-PD / PD middleware. Separate from the Rust stem build (`just build` MSP430/RP2040) — the
  **C island** (STEM-DIRECTION §C-island); it does **not** share a Rust crate.
- **Field-update granule = CYPM1111 flash rows** (STEM-DIRECTION §Field-updatable) — different from A/B
  whole-image swap; design the update path around row writes.

### 3.2 Modules (implement in bring-up order, §4)
1. **PD sink** — ModusToolbox PD stack: advertise Rd, negotiate **5 V/3 A** (higher PDOs possible to 20 V/5 A,
   not needed). Gate downstream high current on "contract valid".
2. **bq25601 I²C driver** — set IINDPM / charge current; read battery V/I **telemetry** (CYPM1111 ADC is
   8-bit, source-path sense only → rely on charger I²C); command **BATFET ship mode** (REG07[5]).
3. **Dead-battery boot sequence** — self-power from VBUS (HV LDO), present dead-battery Rd, negotiate,
   enable charger + trickle the dead cell, bring up SYS **before the SoM** (ADR-0003: no handshake wire).
4. **Power-domain gating GPIO** — coarse gating at the PD controller (few B2B lines): `V_SOM_EN` (outer SoM
   gate), peripheral rail, SS-mux enable (POWER-DISTRIBUTION §gating). Fine per-sensor gating stays at the MSP.
5. **Stem-bus node (0x23)** — slave register map (I2C-API PCA9698-emulation + Thepia ext) **and**
   SoM-off-regime **master** with one-master-at-a-time handoff (ADR-0002/0003).
6. **Ship-mode entry/exit** — on power-off / long-idle, command `BATFET_DIS` **as the last act**; wake via
   **QON / VBUS** (ADR-0011).
7. **Future (out of scope):** custom VDM/alt-mode, T-USB directional-USB-C.

---

## 4. Bring-up sequence (milestones)

1. **CY7111 PD sink** — negotiate 5 V/3 A against a PD charger; observe on KitProg3. *(no charger yet)*
2. **+ bq25601 over I²C** — charge a 1S cell, set IINDPM, read status. *(optionally EV2400+BQSTUDIO first to
   characterize registers before firmware)*
3. **+ power-path / DPM / supplement** — load SYS with the e-load; watch charge current back off as SYS load
   rises; confirm supplement mode.
4. **+ ship mode** — command `BATFET_DIS`; confirm SYS collapses + CYPM1111 unpowered; **measure ≈17 µA**;
   wake via QON. (ADR-0011)
5. **+ dead-battery boot** — flat/absent cell → self-power from VBUS, negotiate, trickle, bring up SYS.
6. **+ Stem-bus node (0x23)** — join the MCU bench (RP2040 SoM-stand-in master, MCU-BENCH.md); slave reg map,
   then SoM-off master handoff.
7. **Fold into the 292 schematic** — turn §2.1 into the product board.

---

## 5. Open items / dependencies

- **VDDIO level-shift choice** (1.8 V i.MX I²C vs SS-mux control level) — pick at schematic.
- **292 product schematic does not exist yet** — this plan feeds it.
- **Runtime target** (sizes 18650 vs 21700+) — eval open item.
- **Order CYPM1111 early** — ~13-week lead (stocking artifact, not shortage); get a 1k quote. Emergency-only
  fallback (documented, not pursued): STM32+UCPD+TCPP (eval D4).
- ~~Reconcile TOOLCHAIN.md~~ **done** — TOOLCHAIN.md now carries the C-island exception (CYPM1111 =
  ModusToolbox/C with its own build verb, not the cargo path; Rust is possible on the M0 core but
  impractical — no PMG1 Rust HAL/PAC + the PD stack is C).

## 6. References
- Eval: [`talkihw/Power Module/5V-USB-power-eval/`](../../../talkihw/Power%20Module/5V-USB-power-eval/) (DECISIONS, REQUIREMENTS, FINDINGS, datasheets `pmg1-s1-datasheet.pdf` + `bq25601.pdf`).
- Stem: [POWER-DISTRIBUTION.md](../POWER-DISTRIBUTION.md) · [I2C-API.md](../../I2C-API.md) · [STEM-DIRECTION.md](../../STEM-DIRECTION.md) · [MCU-BENCH.md](../MCU-BENCH.md) · ADR [0002](../adr/0002-role-based-stem-addressing.md)/[0003](../adr/0003-mcus-independent-of-pd-controller.md)/[0007](../adr/0007-battery-native-sys-rail.md)/[0008](../adr/0008-tap-sys-never-raw-bat.md)/[0011](../adr/0011-ship-mode-is-the-off-state.md).
