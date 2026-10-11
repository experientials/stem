# ADR-0002 · Stem I²C address is chosen from the model's role

Status: Accepted
Date: 2026-10-01   Source: I2C-API.md "Stem address is ROLE-BASED (CANONICAL)"; proven on HW 2026-10-03

## Context
Multiple MCU nodes share one Stem bus and must be individually addressable, collision-free, and a master
(SoM/M7, or the RP2040 stand-in) must be able to tell *what* it is talking to. A dev stand-in part must
not be able to masquerade as the production supervisor.

## Decision
The node's Stem slave address is **chosen at boot from the detected model → its role**, not fixed in the
image. Values in the PCA9698 strap group: Supervisor (FR2355/FR2155) = **0x20**, Supervisor dev
stand-in (FR2476/FR2475) = **0x21**, I/O-expander (FR2433) = **0x22**, Power controller (CYPM1111) =
**0x23**. Implemented by `Model::stem_addr()` (prod/src/model.rs).

## Consequences
- The address alone identifies role → a master routes by address; a dev part can't pose as production.
- Verified on hardware (2026-10-03): all three MSP430s answer their role address (`scan` → 0x20 0x21 0x22).
- Collision-free *as long as* one part fills each role; the model+role→address refinement and a
  clash/secondary-address scheme remain open (OPEN-QUESTIONS Q-ADDR).
- Depends on reliable runtime model-detect (ADR-0001).
