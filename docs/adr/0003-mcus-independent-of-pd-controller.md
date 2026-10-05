# ADR-0003 · MCUs and the PD controller are independent (no handshake wire)

Status: Accepted
Date: 2026-10-01   Source: STEM.md; memory `mcus-independent-no-handshake`; OPEN-QUESTIONS Resolved

## Context
Two power-relevant MCUs exist: the **MSP sleep-supervisor** (owns SoM sleep/wake + the sensor bus) and
the **M0 PD controller (CYPM1111)** (owns USB-C/charger/battery/SYS rail, on the separate 292 module). A
recurring temptation was to design a side-channel handshake so one tells the other what to do.

## Decision
They are **independent state machines with no handshake wire**. The MSP answers "is the SoM awake?"; the
PD controller answers "is the system powered?". The only coupling is the physical power dependency (no
SYS ⇒ MSP unpowered). Where they must coordinate (e.g. the PD controller commanding MCUs while the SoM is
off), they do it **over the Stem bus** as ordinary bus participants — the PD controller is a full Stem
node (ADR-0002, addr 0x23) that masters the bus in the SoM-off regime. One master at a time.

## Consequences
- No invented coordination lines; each reacts to its own power environment.
- The MCU dev bench excludes the PD controller (it has its own PD bench).
- "MSP tracks power" means tracking the **PMIC (PCA9450)** SoM-run-status pins — a *different* chip from
  the PD controller; the MSP has nothing to do with battery/USB power (OPEN-QUESTIONS Q-PWR).
