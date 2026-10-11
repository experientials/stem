# ADR-0004 · Default boot mode is Passive

Status: Accepted
Date: 2026-10-03   Source: OPEN-QUESTIONS Resolved; prod/DESIGN.md Boot step 3; mode::default_boot

## Context
At power-on the SoM/host is typically active and owns the sensor (Leaf) bus. If the MSP booted into
Sensing it would grab the Leaf bus as master while the SoM is also a master → a **two-master fight** (the
exact bench failure mode). The MSP only needs to master the Leaf bus once the SoM has suspended.

## Decision
Boot default is **Passive** (was Sensing). In Passive the MSP stays an I/O-extender and holds eUSCI_B0 in
reset with P1.2/P1.3 tri-stated (off the Leaf bus); it enters Sensing only when commanded (SoM write to
MODE_CTRL, or the future edge-triggered sideband). A sensing-only build (no `mode-passive`) falls back to
Sensing.

## Consequences
- No boot-time two-master contention on the Leaf bus.
- The Leaf enumerate does not run at boot (Passive never masters) — state is published as "booted".
- Pairs with ADR-0005/0006: the control channel must stay reachable regardless of mode.
