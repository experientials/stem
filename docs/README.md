# stem docs — index & reading order

Natural reading order for understanding the system, then the reference/bench docs. Each line says what
the doc is for.

## Start here — what the system is & how it behaves
1. **[STEM.md](STEM.md)** — product experience + the real-world/technical scenarios the Stem bus serves
   (the "why"). Includes the MSP-tracks-PMIC / M7-vs-SoM-suspended clarifications.
2. **[POWER-STATES.md](POWER-STATES.md)** — the **S0–S5 power ladder**: what DSM is, the GPC (A53/M7
   domains), power magnitudes, the "SoM is a natural floor" principle, and the battery-threshold plan.
3. **[POWER-DISTRIBUTION.md](POWER-DISTRIBUTION.md)** — **how each component is powered**: source chain,
   per-part supplies, the handful of rails (µ-AON / gated SoM / peripheral), the rail-live-per-state
   matrix, and the SB-UCM carrier schematic-pass findings (`V_SOM_EN` gates the SoM, etc.).

## Firmware (the MSP supervisor)
4. **[../prod/DESIGN.md](../prod/DESIGN.md)** — the prod MSP firmware design: operating modes
   (Passive/Sensing), boot logic (Stem-slave-first, Passive default), the PMIC/wake path.
5. **[../I2C-API.md](../I2C-API.md)** — the Stem-bus register map (PCA9698 emulation + Thepia ext) and the
   **role-based addressing** (model→address).

> **PD controller (292 power module) — implementation plan:**
> **[proposals/pd-controller-cypm1111.md](proposals/pd-controller-cypm1111.md)** — hardware hookup +
> firmware bring-up for the **CYPM1111** node (derives from the power-module eval + ADR-0002/0003/0007/0008/0011;
> bench-first on CY7111 + BQ25601EVM).

## Build & bench
- **[FR2355-I2C-BRINGUP-PLAN.md](FR2355-I2C-BRINGUP-PLAN.md)** — active fault-isolation plan for the
  FR2355 Leaf bus (UCB0 / P1.2·P1.3) as master **and** slave at 100 kHz: bisect electrical → clock →
  master-vs-slave against the RP2040 known-good, driven by a signature-pattern library.
- **[BUS-RECOVERY.md](BUS-RECOVERY.md)** — the bus recovery & mastery **topic** (CANONICAL 4-level
  ladder: single-MCU state → SoM suspend/resume mastery transfer → coordinated multi-master →
  true hardware-arbitrated multi-master). The anchor the wedge work sits under.
- **[I2C-WEDGE-INVESTIGATION.md](I2C-WEDGE-INVESTIGATION.md)** — **level 1** of BUS-RECOVERY: living log
  for the Leaf-bus **wedge** phenomenon + recovery across MCU×LaunchPad×pull-up×device combos: the
  observation matrix, the open FR2355-trigger question, discriminating tests, and the wedge-injection tool.
6. **[MCU-BENCH.md](MCU-BENCH.md)** — the dev/test bench: the Pico as SoM/M7 **simulator**, the MSP
   boards, the Stem/Leaf wiring, and the `vstem`/`vleaf` test scenarios.
7. **[../TOOLCHAIN.md](../TOOLCHAIN.md)** — the build system: one `just build <chip>`, MSP430→Docker /
   RP2040→host cargo.
   - **[BUILD-PROFILES.md](BUILD-PROFILES.md)** — the four build axes (image / log verbosity / profile /
     symbols), why `--debug` today is a *logging* feature not a debug build, the proposed
     `--release`/`--log`/`--debug` scheme, and the `cargo-batch` evaluation.
   - **[TASK-STYLE-GUIDE.md](TASK-STYLE-GUIDE.md)** — how to write concurrent work on the MSP430 tier:
     cooperative `sched`/hand-written state machines vs `async`/`.await` on a `core`-only executor —
     the rules for each, the SAME logic shown both ways, and the measured cost to choose between them.
8. **[RP2040-BENCH-HOST.md](RP2040-BENCH-HOST.md)** · **[MCU_SELECTION.md](MCU_SELECTION.md)** ·
   **[DEV_BOARDS.md](DEV_BOARDS.md)** — bench host design, MCU choice, dev-board inventory.

## Why it's built this way / what's still open
9. **[adr/](adr/README.md)** — **Architecture Decision Records**: the chronological *why* behind the
   firmware + hardware design (single-binary-per-family, role-based addressing, Passive default,
   control-channel-never-blocks, battery-native rail…). Decisions live here; other docs link in.
10. **[OPEN-QUESTIONS.md](OPEN-QUESTIONS.md)** — open strategy/research questions **with resolutions**
   (addressing, persistence, POST-in-prod, anti-counterfeit (deferred), the I²C-collision questions).
11. **[BACKLOG.md](BACKLOG.md)** — firmware backlog.

## Key external references (in-repo PDFs / sibling docs)
- **i.MX8MP** power/compute: `ziloo/Hardware/refs/i.MX 8/IMX8MPRM.pdf` (RM Ch.5 GPC/DSM), **`AN13400.pdf`**
  (M7-in-suspend), **`AN12195.pdf`** (low-power audio), `IMX8MPCEC.pdf` (datasheet).
- **M7 coprocessor** (A53↔M7, RDC, suspend): `.claude/skills/ucm-dev/references/m7-coprocessor.md`.
- **PD controller / charger / PMIC**: `talkihw/Power Module/5V-USB-power-eval/` (DECISIONS.md +
  `datasheets/pmg1-s1-datasheet.pdf`, `bq25601.pdf`), `ziloo/Hardware/datasheets/i.MX8/PCA9450DS.pdf`.
- **Carrier schematic**: `ziloo/Hardware/datasheets/i.MX8/sb-ucmimx8plus_1v1/sb-ucmimx8plus_1v1.pdf`
  (bench carrier; the **292** product power module is not yet a schematic).

> Skill: when working under `stem/`, load the **`stem-dev`** skill — it maps the repo + build/flash.
