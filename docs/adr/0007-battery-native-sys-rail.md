# ADR-0007 · Present a battery-native SYS rail (don't boost to 5 V)

Status: Accepted
Date: 2026-10-03   Source: POWER-DISTRIBUTION.md design pass (was cited as "DECISIONS.md D3")

## Context
The carrier rails can be fed either from a boosted fixed 5 V or directly from the battery-native system
rail. Boosting everything to 5 V adds a converter, loss, and heat, and fights the NVDC charger topology.

## Decision
Present the **battery-native SYS rail** (bq25601 NVDC output, ~3.0–4.2 V, VSYS_MIN ~3.68 V) as the
carrier's main rail; do **not** boost to 5 V. Per-domain regulators derive what they need from SYS
(1.8 V AON for the MCU + Stem bus to match the SoM I/O domain, gated SoM/peripheral rails).

## Consequences
- Simpler, more efficient front-end; 1.8 V MCU/Stem domain needs no level shifters on the product carrier.
- Rails must tolerate the battery voltage range; anything needing 5 V (e.g. some USB/peripheral paths) is
  a local, gated exception.
- The CompuLab SB-UCM bench carrier (barrel-jack, Vin 4.5–18 V → its own bucks) is a bench reference
  only, not the product power front-end.
