# Power distribution — how each component is powered (DRAFT 2026-10-03)

The **target** power architecture, derived from the component datasheets + decisions. See the
[docs index](README.md). Companion to [POWER-STATES.md](POWER-STATES.md) (the S0–S5 state ladder) —
this is *how each part is fed*; that is *which rails are live when*. **Board-specific nets/regulators are marked TO-VERIFY against the actual
292-power-module + carrier + 202 schematics** (`sb-ucmimx8plus_1v1.pdf`, the 292 design).

## Source chain
```
USB-C ──▶ CYPM1111 (PD ctrl) ──drives external PFETs──▶ VBUS
                                   │ (negotiates PD, controls charger over I²C)
         VBUS ──▶ bq25601 (NVDC charger) ──▶ SYS rail (battery-native ~3.0–4.2 V, VSYS_MIN ~3.68 V)
                        │                          │
                        └──charges──▶ 18650 (BAT)  └──▶ everything on the carrier
```
- **NVDC power path:** when USB is in, the **system load on SYS is served from VBUS**, and the cell is
  charged separately — so carrier loads **don't leach the battery's charge current**. On a dead battery
  the charger holds SYS at VSYS_MIN from VBUS (dead-battery boot). (DECISIONS.md D1/D3/D4.)
- **Never power the carrier from raw BAT** — only from SYS — else ship mode (BATFET open) can't
  disconnect it and the cell drains in storage. (DECISIONS.md firm rule.)

## Per-component power
| Component | Board | Supplied by | Voltage | Notes |
|---|---|---|---|---|
| **CYPM1111** (PD controller) | power module 292 | **direct off VSYS or VBUS** | 2.75–5.5 V (VBUS to 21.5 V via HV LDO) | wide input → **no regulator**; self-powers on dead-battery from VBUS |
| **bq25601** (charger) | 292 | VBUS / SYS | — | NVDC; CYPM1111 sets IINDPM/charge/ship over I²C |
| **18650 cell** (BAT) | 292/battery | charged by bq25601 | 3.0–4.2 V | ship mode = BATFET open (≈17 µA) |
| **PCA9450** (SoM PMIC) | carrier/SoM | **SYS, gated** | — | generates the full i.MX8MP rail set (VDD_SOC/ARM/DRAM/NVCC_1V8/3V3/SNVS…). Enabling the PMIC = enabling the SoM |
| **i.MX8MP SoM** | UCM module | via PCA9450 | per-rail | the "**SoM rail**" domain — gated as one unit |
| **MSP430 supervisor** (FR2355/2476/2433) | 202 (+others) | **1.8 V LDO off SYS** | 1.8 V | FRAM writes OK at 1.8 V (not flash); ~8 MHz max — fine for a supervisor; 1.8 V **matches the SoM I/O domain → no Stem-bus level shifters** |
| **Wake sensors** (presence/motion, always-on) | 202/face | **1.8 V AON rail** (with the MSP) | 1.8 V (or as the part needs) | kept alive whenever the MSP is; MSP enables each locally |
| **Heavy sensors / cameras** | 202/face | **gated peripheral rail** | 1.8/3.3 V | only when the SoM is up; MSP/SoM enables per-device |
| **USB3 SS redriver/mux** (TUSB1046…) | 292 | gated (USB-data active) | — | only when USB-C data path is used |
| **nRF52 radio** (if fitted) | module | M7-domain gated rail | — | omittable radio module; hosted by M7 over SPI |

## Rail groups — the "handful" from the power module (coarse), fine control at the MSP
The CYPM1111 is on the **power module (292)**, so it can't have many control tentacles across the
module↔carrier B2B. **Coarse domain gating at the PD controller; fine per-sensor control at the MSP**
(local on the carrier, owns the sensor enables).

| Rail | Source | Controlled by | Purpose |
|---|---|---|---|
| **SYS** (always-on) | bq25601 | — (always, while battery OK) | the battery-native carrier rail; feeds everything below |
| **1.8 V AON** | LDO off SYS | — (on with SYS) | MSP + wake sensors + Stem bus (1.8 V = SoM I/O domain) |
| **SoM rail** (gated) | SYS → PCA9450 | **drive `V_SOM_EN`** (carrier net, schematic-confirmed) | PCA9450 → i.MX8. **Never on inadvertently** |
| **Peripheral rail** (gated) | SYS | **CYPM1111 GPIO** (+ MSP per-device) | heavy sensors / camera / redriver (SoM-up only) |

*(Optional — autonomous wake while **stored**: a **nanopower LDO off raw BAT** for the MSP + one wake
sensor, alive even in ship mode. Cost: ship mode can't fully disconnect → continuous µA drain. Deferred
— button/plug wake + ship mode covers most products. DECISIONS.md D5.)*

## Which rails are live per power state (ties to POWER-STATES.md)
| Rail | S0 dead | S1 ship | S2 forced-suspend | S3 deep-standby | S4 M7-alive | S5 full |
|---|---|---|---|---|---|---|
| SYS | off | collapsed | **on** | on | on | on |
| 1.8 V AON (MSP + wake) | off | off | **on** | on | on | on |
| SoM rail (PMIC → i.MX8) | off | off | **OFF (forced)** | off | **on** | on |
| Peripheral / heavy sensors | off | off | off | off | partial | on |

The **SoM rail is the one big lever** (SoM-on vs SoM-off is the natural floor, POWER-STATES §1c): cut it
below the deep-standby reserve, enable it when the battery can run the M7.

## Design rules (carry into the schematic pass)
1. **Battery-native rail** — present SYS ~3.6 V, don't boost to 5 V. (DECISIONS.md D3.)
2. **Tap SYS, never raw BAT** (except the optional nanopower storage-watcher) — keeps ship mode + charge
   clean; carrier loads run off VBUS during charging.
3. **CYPM1111 runs direct off SYS/VBUS** — no regulator.
4. **MCU + Stem bus at 1.8 V** — matches the SoM I/O domain (no shifters on the product carrier; the
   *bench* uses 3.3 V + shifters, a bench-only difference).
5. **SoM gated as one domain** via a PD-controller GPIO → load switch / PMIC-EN; never inadvertently on.
6. **Coarse at the PD controller (few B2B lines), fine at the MSP** (local sensor enables).

## Schematic pass — SB-UCM carrier `sb-ucmimx8plus_1v1.pdf` (2026-10-03)
Read the 15-page carrier schematic. **It's the CompuLab *bench* carrier (barrel-jack powered), not the
Thepia product power** — but it grounds the UCM power/control interface any carrier must drive:
- **How the SoM is gated — CONFIRMED: `V_SOM_EN` (+ `VSOM_BP_EN#`)** is the carrier's SoM-power enable
  (p13), **a carrier net — NOT a PMIC pin**. **Two levels of gating:**
  - **`V_SOM_EN`** (carrier, upstream of the module; owner = **PD controller**) = the OUTER gate: *is the
    SoM module powered at all?* → the SoM-**OFF** lever.
  - **`PMIC_ON_REQ` / `PMIC_STBY_REQ` / `POR_B`** (on the PCA9450 inside the UCM; owner = **i.MX8/GPC**,
    MSP *tracks*) = the INNER sequencing: given power, **RUN / STANDBY / PWRDN**.
  - **Full SoM state = `V_SOM_EN` × PMIC-state.** Re-power caveat: after `V_SOM_EN` goes high the PCA9450
    comes up in SNVS; `PMIC_ON_REQ` must then be asserted to raise the rails (who drives it at cold
    re-power — carrier ONOFF / PD controller / strap — is a 292+carrier design detail).
- **PMIC / SoM-run-status sideband — CONFIRMED real carrier nets** to the PCA9450 (p2): **`PMIC_ON_REQ`,
  `POR_B_3P3` (open-drain, pull-up near SoC), `SYS_RST_PMIC`** + `SYS_I2C`. So the MSP's "tracks the PMIC"
  signals are real nets, not notional (ties Q-PWR: these are the SoM-run-status pins).
- **Always-on vs gated rails — CONFIRMED split:** `3V3_AW` ("always-wake") + `3V3_RTC` are always-on;
  `3V3_PER` / `5V0_PER` / `1V8_PER` are the gated/peripheral rails (each with an `_EN`, p4/p13). Maps
  directly to our **µ-AON vs gated-peripheral** grouping.
- **Input mismatch — FLAG:** the bench carrier input is **`Vin 4.5–18 V`** (barrel jack) → its own
  bucks make 5V0/3V3/1V8. That **does NOT match the battery-native SYS ~3.0–4.2 V**. So the *product*
  carrier + the 292 module need the battery-native front-end — the SB-UCM is a bench reference only.

## Open / TO-VERIFY
- **292 power module has NO schematic in-repo** — only DECISIONS/REQUIREMENTS (decided: CYPM1111 +
  bq25601, not yet designed). **The battery-native product power is the real open piece** — build the 292
  schematic to this model (SYS front-end + the µ-AON/SoM/peripheral rails + the `V_SOM_EN` gate).
- **Product carrier** (bob-929, not the CompuLab SB-UCM): must accept battery-native SYS (~3.6 V), drive
  `V_SOM_EN` to gate the SoM, and route `PMIC_ON_REQ`/`POR_B`/`SYS_RST_PMIC` + the Stem bus to the MSP.
- Exact **PCA9450 rail set + sequencing** (it's on the UCM SoM module; gating is via `V_SOM_EN` upstream,
  not the PMIC-EN).
- Which sensors are **wake (AON)** vs **heavy (gated)**; **B2B pin budget** for the coarse rails + control.
- **Board rethink** stands: the existing designs predate the PD-controller-as-power-brain (POWER-STATES §1c).
