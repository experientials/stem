# ADR-0005 · Stem slave comes up before the Leaf enumerate (boot)

Status: Superseded by ADR-0006 (generalised to runtime)
Date: 2026-10-03   Source: OPEN-QUESTIONS Resolved; prod/src/main.rs:152; prod/DESIGN.md Boot step 0

## Context
The Stem I²C slave is the SoM's only control channel into the node. Originally the node brought the slave
up **after** the Leaf enumerate, so a wedged/slow sensor bus (e.g. two masters contending) could delay —
or indefinitely block — the SoM's only way in.

## Decision
At boot, bring up the **Stem slave (eUSCI_B1) first**, before entering the boot mode / running any Leaf
enumerate. The control channel must be reachable the instant the node boots, independent of sensor-bus
state.

## Consequences
- The SoM can always reach the node after a cold boot, even if the Leaf bus is wedged.
- This fixed only the **boot** path. The **runtime** mode switch still ran the Leaf scan inline and
  re-introduced the stall — found via OPEN-QUESTIONS Q-SENSING-ENUM and generalised in **ADR-0006**.
