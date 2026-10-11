# ADR-0006 · A runtime mode switch must not block the Stem control channel

Status: Accepted
Date: 2026-10-03   Source: OPEN-QUESTIONS Q-SENSING-ENUM (HIL finding); generalises ADR-0005

## Context
ADR-0005 made the Stem slave reachable at **boot**. But the **runtime** Passive→Sensing switch
(`apply_mode` in prod/src/main.rs) ran `enumerate::scan()` — I²C bus recovery + up to 112 address probes
— **inline in the main service loop**. While that blocked, `stem::poll` was not servicing the Stem slave
and the UART console RX was not polled. On a wedged Leaf bus (recovery spins, probes time out) the stall
is longest *exactly* when the control channel matters most.

Observed on hardware (Q-SENSING-ENUM): the Pico's Stem mode-readback **flapped** (it raced the blocking
scan), and the FR2355 **dropped console keystrokes** in Sensing — both symptoms of the same inline stall.

## Decision
A mode switch performs only the **fast** part synchronously — `ModeMachine::switch` (acquire/release the
bus: pin mux + eUSCI bring-up) — then **publishes the new mode immediately** and runs the Leaf enumerate
**incrementally in the main loop** (a `ScanJob` stepper advanced a few probes per iteration, with a
hard-bounded one-shot recovery). `stem::poll` runs between every scan step, so the Stem slave and console
never stall more than one step. This generalises ADR-0005's principle from boot to **all** mode changes:
*sensor-bus work never blocks the Stem control channel.*

## Consequences
- The SoM mode-readback is stable; the console stays responsive in Sensing (removes the flap + starvation
  on a healthy/recoverable bus — see Verification for the faulted-bus caveat).
- `dev_count`/status become eventually-consistent: after a Sensing switch the SoM may briefly read the
  pre-scan status until the incremental scan completes (a few main-loop passes). Acceptable — the node is
  reachable throughout, which matters more than an instantaneous count.
- The synchronous `enumerate::scan()` is kept as a convenience wrapper (runs the job to completion) for
  the dev-console `s` command, where a brief block is acceptable.
- Does **not** fix the underlying bench two-master fault (Q-SENSING-ENUM defect #2 — Pico must tri-state
  its Leaf master); it makes the node *survive* that fault without losing its control channel.

## Verification (2026-10-03, on FR2355 via `thepia hwd`)
- **Sensing-readback flap: FIXED.** 6/6 `sens`→read `0x3E` returned `0x01` (was flapping). ✅
- **Console in Sensing: responds** (was starved for the whole multi-second scan) — but still drops the
  *occasional* keystroke on a faulted bus (succeeded on retry 3/4). Partial.
- **Residual, faulted-bus only:** a `pass` write can still be lost → mode stuck at `0x01` (seen ~4/6 when
  `fault=0x01`; solid `0x00` when `fault=0x00`). **Root cause:** the incremental step shrank the stall
  from the whole scan to a *batch* of `SCAN_BUDGET` probes, but the eUSCI_B1 slave is still not serviced
  **inside a single probe's blocking `SPIN`** — on a faulted bus one probe (recovery + full `SPIN`) is
  ~tens of ms ≫ the I²C inter-byte time, so a concurrent master write can RX-overrun and be dropped.
- **Completion (follow-up):** pump the Stem slave *during* each Leaf probe (reuse the `read_reg_pumped`
  pump pattern → a `probe_pumped` the `ScanJob` calls with `pump = || stem::poll(p, slave, rf)`), so the
  slave is serviced within the inter-byte window regardless of bus health. Until then, the residual is
  **gated by defect #2**: fixing the bench two-master fault keeps the bus un-faulted, where this fix is
  already solid. Tracked in OPEN-QUESTIONS Q-SENSING-ENUM.
