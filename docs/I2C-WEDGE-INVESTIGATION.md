# I2C bus-wedge — ongoing investigation across hardware combinations

> **Scope.** This is **level 1** (single-MCU state) of the broader bus recovery & mastery topic —
> see [BUS-RECOVERY.md](BUS-RECOVERY.md) for the canonical 4-level ladder (levels 2–4 cover
> SoM suspend/resume mastery transfer, coordinated multi-master, and true hardware-arbitrated
> multi-master).
>
> **Purpose.** A living log for the Leaf-bus (eUSCI_B0, P1.2/P1.3) **wedge** phenomenon and its
> recovery, tracked across MCU × LaunchPad × pull-up × device combinations. This is expected to be a
> **recurring topic** as new parts/benches come online — capture each new observation here rather than
> re-deriving. Companion to [FR2355-I2C-BRINGUP-PLAN.md](FR2355-I2C-BRINGUP-PLAN.md) (the FR2355
> first-encounter) and the `g` signature battery ([diag/src/signature.rs](../diag/src/signature.rs)).
> Project memory: `i2c-scl-low-wedge-recovery`.

## What "wedge" means

The Leaf bus stops behaving: probes false-ACK, register reads fail, and the bus does not return to
idle-high. Two mechanically distinct modes:

| Mode | Idle levels | Cause | Clears by |
|---|---|---|---|
| **SDA-low** | SDA=0, SCL=1 | a *slave* holding SDA mid-byte | 9-clock bit-bang + STOP |
| **SCL-low** | SDA=1, SCL=0 | *our own eUSCI* left holding the clock (aborted/timed-out read), **or** a slave stretching SCL | UCSWRST release (master self-hold); a slave stretching SCL cannot be forced |

The driver (`crates/bsp/src/i2c.rs`) now handles both; the per-transaction gate and `prod/enumerate.rs`
reason on **both lines** (`bus_idle`), not SDA alone. See the two fix commits
(`recover()` SCL-low; `enumerate` both-line).

## The core open question

**The SCL-low wedge was reproducible on the FR2355 bench but NOT on FR2476 with the identical device
and the *legacy* (SDA-only) recover.** So the *trigger* is bench/electrical-specific, not a property
of the shared firmware. The firmware fix is **defensive hardening** (correct and worth keeping); the
*root trigger* on FR2355 is **still unidentified.**

## Observation matrix (append as combinations are tried)

| Date | MCU / board | LaunchPad pull-ups | Leaf device(s) | recover() | Result |
|---|---|---|---|---|---|
| 2026-10-05 | FR2355 `8E57…` | **added 5.6 kΩ** | PCA9698 @0x20 | legacy (SDA-only) | **WEDGED** (SCL=0, 82 phantom ACKs, reads fail) |
| 2026-10-05 | FR2355 `8E57…` | added 5.6 kΩ | PCA9698 @0x20 | fixed (both-line) | clean, P5 OK, repeatable |
| 2026-10-05 | FR2476 `0E5D…0A002800` | LaunchPad onboard | PCA9698 @0x20 | legacy (SDA-only) | **CLEAN** — no wedge |
| 2026-10-05 | FR2476 `0E5D…0A002800` | LaunchPad onboard | PCA9698 @0x20 | fixed (both-line) | clean |

Context (Henrik, 2026-10-05): dev boards are **dirt cheap**; the unit that "worked before" was an
*identical board from the same batch, not the exact same board* → **per-board hardware fault is a live
hypothesis**, alongside pull-up strength and P1.2/P1.3 (OA0±/SAC) loading. Production faults are
expected — which is *why* robust recovery is a requirement, not a nicety.

## Leading hypotheses for the FR2355 trigger (unresolved)

1. **Pull-up strength / rise-time** — FR2355 on *added* 5.6 kΩ vs FR2476's (likely stronger) onboard
   pulls → marginal SCL edges desync the eUSCI into holding SCL low.
2. **P1.2/P1.3 loading** — those pins are also `OA0±`/SAC inputs on the FR2355 LaunchPad; possible
   extra capacitance/second load the FR2476 board lacks.
3. **Per-board hardware fault** on `8E57…` — cold joint, marginal pin, damaged pad.

## Discriminating tests (when back at the bench)

- **2.2 kΩ on FR2355 + legacy recover** → wedge vanishes ⇒ rise-time/pull-ups is the trigger.
- **Another FR2355 board from the batch, same extender + pull-ups, legacy recover** → clean ⇒ `8E57…`
  is a faulty unit; still wedges ⇒ model/pull-up, not per-board.
- **Scope SCL rise-time** on FR2355 vs FR2476 → quantifies the electrical margin directly.
- Repeat the matrix on **FR2155** (production dual-I²C baseline) and **FR2433** when available.

## Proposed tool: on-demand wedge injection (future work — not yet built)

To actually *exercise* the `BUS_WEDGED` reporting path and the recovery (instead of waiting for a flaky
bench to wedge), add a deliberate wedge injector to the diag signature battery. Sketch:

- **Inject SCL-low (master self-hold):** start a master read (`UCTR=0`, `UCTXSTT`), let the eUSCI clock
  in one byte, then **return without reading `RXBUF`** — the eUSCI stretches SCL low waiting for the
  CPU. Snapshot `bus_levels` (expect SDA=1, SCL=0), then call `recover()` and confirm it returns to
  idle-high. Expected: legacy recover FAILS to clear it; the fixed recover clears it.
- **Inject SDA-low (slave-style):** drive P1.2 low as GPIO, confirm the 9-clock dance frees it.
- **Assert the fault surface:** after injecting, run `prod::enumerate` (or the diag equivalent) and
  confirm `faulted=true` / `BUS_WEDGED` is reported (not a false `BUS_OK`).

Wire it as a new signature pattern (e.g. `P0 inject` before `P1 idle`, gated behind a key) so a single
`g`-style run proves both *detection* and *recovery* deterministically on any board — closing the one
verification gap that a healthy bench leaves open.

## Status

- Driver + prod fixes committed on `feat/rp2040` (`recover()` SCL-low; `enumerate` both-line).
- Verified: healthy-bus no-regression on FR2355 (sensing build: finds 0x20, `fault=0x00`/`BUS_OK`).
- **Open:** the FR2355 trigger (above), the wedge-injection tool, and extending the matrix to more
  MCU/board/pull-up combinations.
</content>
