# ADR-0001 · One firmware binary per compatible family (runtime model-detect)

Status: Accepted
Date: 2026-09-21   Source: prod/DESIGN.md "Permutation strategy (RESOLVED 2026-09-21)"

## Context
The node ships on several MSP430FR parts (FR2476/FR2475 dev, FR2155/FR2355 production dual-I²C, FR2433
single-I²C). Building and tracking a separate image per exact part multiplies release/test surface. But
the parts differ in PAC + memory map + peripheral count, so a single universal binary is impossible.

## Decision
One binary **per compatible family**, not per part. The family feature selects the PAC/memory map at
build time; within a family the firmware **detects the model at runtime** (`model::detect()` via the TLV
device-id) and maps pins/role from that. Permutations = number of *incompatible* families, not parts.

## Consequences
- Fewer images to build/verify; a dev part (FR2476) runs the same image as its production superset.
- Runtime detection must be reliable and cheap (it gates pin/role/addr) — see ADR-0002.
- A size ceiling per family image (FRAM budget, `scripts/size-check.sh`) is a hard gate.
- Crate structure split into shared `bsp`/`hal`/`devices` + thin family glue (DESIGN.md "Consequences").
