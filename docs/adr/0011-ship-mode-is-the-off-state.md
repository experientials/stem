# ADR-0011 · Ship mode (BATFET open) is the product's off / storage state

Status: Accepted
Date: 2026-10-04   Source: `talkihw/Power Module/5V-USB-power-eval/DECISIONS.md` (D5); Henrik

## Context
A toy may sit **uncharged for months**. The CYPM1111 (PD controller / power brain) deep-sleep floor is
**~150 µA** while powered (SYS live) — ~100× an MSP430 LPM3 — far too high as a sole always-on
keep-alive (≈22% of an 18650 over 6 months, and it would deep-discharge the cell). The cell's own
self-discharge (~2–3%/month) is the real floor; chasing µA in the PD chip is the wrong lever. We need an
"off"/storage state that doesn't bleed the pack, without adding an always-on element.

## Decision
**"Off" / storage = the bq25601 BATFET-disable ship mode** (REG07[5]). The **CYPM1111 commands
`BATFET_DIS` over I²C as its last act** on power-off / long-idle; the BATFET opens → **SYS collapses →
the CYPM1111 and all loads are disconnected from the cell** (≈**17 µA** charger quiescent at BAT). **Wake
is hardware**: the bq25601 **QON** pin (button) re-enables the BATFET, or **VBUS plug-in** powers the
CYPM1111 via its HV LDO. **No always-on keep-alive MCU is required** — which is *why* dropping the MSP430
as a keep-alive is fine (it was never going to be the standby win; ship mode is).

## Consequences
- Months-in-storage draw ≈ **17 µA + cell self-discharge**, not 150 µA.
- **Requires** the system to be fed from **SYS, never raw BAT** ([ADR-0008](0008-tap-sys-never-raw-bat.md)),
  or ship mode can't disconnect it and the CYPM1111 drains 150 µA forever.
- **Deep-sleep ≠ off** — power-off firmware *must* command ship mode; a crash that leaves SYS live bleeds
  150 µA (watchdog/entry logic should cover this).
- Wake is button/plug only. If **autonomous wake-while-stored** (timer/motion, no button/plug) is ever
  required, that needs a nanopower always-on element on BAT — **revisit then**; not needed now.
- Complements the battery-native rail ([ADR-0007](0007-battery-native-sys-rail.md)) and the PD
  controller's independence ([ADR-0003](0003-mcus-independent-of-pd-controller.md)).
