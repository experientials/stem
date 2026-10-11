# ADR-0008 · Carrier loads tap SYS, never raw BAT (ship-mode + charge integrity)

Status: Accepted
Date: 2026-10-03   Source: POWER-DISTRIBUTION.md design pass (was cited as "DECISIONS.md D1/firm rule")

## Context
Carrier loads could be wired to the raw battery (BAT) or to the charger's system rail (SYS). With an NVDC
charger, where the load is tapped determines two things: whether **ship mode** (BATFET open) can actually
disconnect the cell in storage, and whether carrier loads **steal the cell's charge current**.

## Decision
Carrier loads are fed **only from SYS, never from raw BAT** (except the optional nanopower storage-watcher,
deferred). The NVDC path serves the system load from **VBUS** when USB is present (so the cell charges
separately), and holds SYS at VSYS_MIN from VBUS on a dead battery (dead-battery boot).

## Consequences
- Ship mode can fully disconnect the cell in storage (no slow drain); charging isn't robbed by loads.
- On USB, loads run off the input, not the charging cell — faster, cooler charge.
- Nothing may hang off raw BAT except a deliberately-deferred µA storage watcher (see
  POWER-DISTRIBUTION.md "Optional"); adding one re-opens the ship-mode-drain tradeoff.
