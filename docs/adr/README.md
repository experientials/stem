# Architecture Decision Records (ADR)

Chronological log of **why** the stem firmware + hardware are built the way they are — one decision per
file, numbered in decision order. An ADR captures the *reasoning at the time*; it is not edited to match
later reality. When a decision changes, add a **new** ADR that **supersedes** the old one (and mark the
old one `Superseded by ADR-NNNN`), rather than rewriting history.

This log is the home for decisions. Other docs hold the *living* state and should **link here** for the
why:
- **[../../prod/DESIGN.md](../../prod/DESIGN.md)** — current firmware design + the CANONICAL requirements block.
- **[../OPEN-QUESTIONS.md](../OPEN-QUESTIONS.md)** — open questions; its `Resolved` section predates this
  log and is being migrated into ADRs (it stays as the raw running record).
- **[../POWER-STATES.md](../POWER-STATES.md)** / **[../POWER-DISTRIBUTION.md](../POWER-DISTRIBUTION.md)** — power model/detail.

## Format (keep it short)
```
# ADR-NNNN · <title>
Status: Proposed | Accepted | Superseded by ADR-MMMM | Deferred
Date: YYYY-MM-DD   Source: <where the decision was made / canonical home>

## Context      — the forces; what made this a decision, not a default.
## Decision     — what we chose, stated plainly.
## Consequences — what this buys, what it costs, what it constrains later.
```

## Index
| ADR | Date | Title | Status |
|---|---|---|---|
| [0001](0001-single-binary-per-family.md) | 2026-09-21 | One firmware binary per compatible family (runtime model-detect) | Accepted |
| [0002](0002-role-based-stem-addressing.md) | 2026-10-01 | Stem I²C address is chosen from the model's role | Accepted |
| [0003](0003-mcus-independent-of-pd-controller.md) | 2026-10-01 | MCUs and the PD controller are independent (no handshake wire) | Accepted |
| [0004](0004-default-boot-passive.md) | 2026-10-03 | Default boot mode is Passive | Accepted |
| [0005](0005-control-channel-reachable-at-boot.md) | 2026-10-03 | Stem slave comes up before the Leaf enumerate (boot) | Superseded by 0006 (generalised) |
| [0006](0006-mode-switch-must-not-block-control-channel.md) | 2026-10-03 | A runtime mode switch must not block the Stem control channel | Accepted |
| [0007](0007-battery-native-sys-rail.md) | 2026-10-03 | Present a battery-native SYS rail (don't boost to 5 V) | Accepted |
| [0008](0008-tap-sys-never-raw-bat.md) | 2026-10-03 | Carrier loads tap SYS, never raw BAT (ship-mode + charge integrity) | Accepted |
| [0009](0009-bench-bitbang-leaf-bus.md) | 2026-10-03 | Bench host owns the Leaf bus as bit-banged open-drain GPIO (one-master) | Accepted |
| [0010](0010-integrity-m7-judges-msp-reports.md) | 2026-10-04 | FRAM integrity: MSP reports per-sector CRCs over I²C, M7 judges (fail-safe) | Accepted |
| [0012](0012-stembus-shared-contract-crate.md) | 2026-10-04 | `stembus` — one shared crate for the Stem-bus contract + codecs (M7/stem-probe/prod) | Accepted |
| [0011](0011-ship-mode-is-the-off-state.md) | 2026-10-04 | Ship mode (BATFET open) is the product's off / storage state | Accepted |
| [0013](0013-async-runtime-per-core-tier.md) | 2026-10-05 | Async runtime per core tier: embassy on Cortex-M/host, `core`-only async `sched` on MSP430 | Accepted (tier) · Proposed (async-sched) |
| [0014](0014-board-layer-structure.md) | 2026-10-06 | Board-layer structure: compile-time family module + runtime model-detect; LaunchPad = product-board module (resolves CONV-1 gate) | Accepted |
