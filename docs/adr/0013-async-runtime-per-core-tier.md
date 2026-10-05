# ADR-0013 · Async runtime per core tier — embassy on Cortex-M/host, a `core`-only async `sched` on MSP430

Status: Accepted (tier split) · Proposed (async-`sched` evolution — size spike DONE: code parity; stack-depth pass pending)
Date: 2026-10-05   Source: Henrik 2026-10-05 ("can we use embassy on MSP prod?" → "can we write async/await and execute with our own sched, resolving it all at compile time to the same size?")

## Context
Two MSP430 firmwares drive their work differently: **diag** uses the `sched` crate
(`Task::poll(&mut self, cx, now) -> Option<u32>`, `Slot::every`, `tick`), **prod** hand-rolls a
triplicated `loop { stem::poll(); pump_scan(); … }` and doesn't use `sched` at all (dead dep). The
question was whether **embassy** — already used on the RP2040 `stem-probe` (`embassy-executor`
`platform-cortex-m`, `embassy-rp`, `-usb`, `-time`, `-futures`) — could provide one cooperative
async runtime for prod too.

**embassy does not fit the MSP430 tier.** `embassy-executor` supports cortex-m / riscv32·64 / std /
wasm / avr — **not MSP430** (16-bit, no platform flavor). And prod is the **low-power sleep
supervisor** targeting **FR2433 ~15 KB FRAM** (`opt-level="s"` + LTO, size-gated): embassy's executor
+ `embassy-time` + task arena are not free, there is no async MSP430 HAL, embassy's wait is per-arch
WFE/WFI (MSP430 LPM would need a bespoke pender), and embassy 0.10 likely needs a newer rustc than our
pinned `nightly-2025-06-25` (the same version-lag that blocked cargo-batch).

But the deeper observation stands: **`sched::Task::poll` is a hand-written `Future::poll` minus the
`Waker`.** `ScanJob` ("advance a few probes per pass", ADR-0006) is literally a hand-coded future
state machine. Rust's `async fn` *generates exactly that* at compile time. So we can get async/await
ergonomics **without embassy** by writing our own `core`-only executor.

## Decision
**Choose the task-execution substrate by core capability tier, not one-size:**

1. **Capable cores (Cortex-M / host) → embassy async.** RP2040 `stem-probe` today; the RP2350 product
   node and the i.MX8 **M7** later. embassy earns its keep where it's supported and RAM/flash are ample.
2. **MSP430-class sleep supervisors → a tiny cooperative executor (`sched`).** No embassy. Today that
   is the run-to-completion `sched` crate; the **proposed evolution** is to make `sched` a
   **`core`-only async executor** so task bodies can be written as `async fn … .await` and compile —
   via monomorphization + LTO — down to ~the same state machines we hand-write now.

The **source-level programming model becomes consistent across tiers (async/await)**; only the
executor differs by capability. This also fixes prod's divergence (adopt `sched`, drop the triplicated
loop and the dead dep) and is the natural path to the **LPM-sleep** behavior a sleep-supervisor needs
(which today's spin-loops lack).

## The async-`sched` evaluation (why it can be ~zero-cost — and the honest caveats)
Feasible on `core` alone — `core::future::Future`, `core::task::{Context,Waker,RawWaker,RawWakerVTable}`,
`core::pin`, `async fn` (incl. in traits since 1.75), `poll_fn`, `pin!` are all **stable at our pinned
1.90**, `no_std`, no-alloc. **No embassy, so no version/arch blocker.**

- **Code size ≈ parity.** The `async fn` future state machine is the same transform as the
  hand-written `ScanJob`; under `opt="s"` + LTO + monomorphization the executor's `poll` inlines. A
  fully-monomorphized `select`/join over a fixed set of concrete futures pays **zero** dispatch (vs
  `sched`'s current `&mut dyn Task` vtable — already a tiny cost).
- **Small fixed floor:** one shared `RawWakerVTable` (4 fn pointers in rodata) + `Context` plumbing.
  Tens of bytes, not KB.
- **RAM is the variable to watch.** The async transform stores all locals-live-across-`.await` in the
  future; rustc's layout is good but can be slightly larger than a hand-tuned struct. On 4 KB SRAM /
  15 KB FRAM this must be **measured**, not assumed.
- **The valuable hard part is LPM sleep.** A no-op waker + spin-poll == today's behavior (same size,
  still spins). A *real* waker (leaf future registers it; an ISR calls `wake()` → flag; the executor
  does LPM when all tasks are Pending) is **new, strictly better** capability a sleep-supervisor wants
  — small, but MSP430-specific, and written by us either way (embassy wouldn't give it to us on MSP430).
- **Async leaves must be authored.** No async MSP430 HAL exists: the eUSCI/Timer_A futures wrap our
  existing polled drivers + interrupts. For prod's event-driven shape this is modest; replicating
  diag's periodic cadences + deadline telemetry needs an async timer leaf (an `embassy-time`-analog).

**Verdict:** the approach is sound and the "same size" intuition is right *in the code dimension* —
prove it by building a **minimal prototype** (`core`-only executor + one `async` eUSCI leaf + a
hand-rolled `select`) and **comparing the FR2433 size gate** against the current hand-written loop
before committing. Code-size parity is expected; the RAM delta is the risk to measure.

### Spike result (2026-10-05) — MEASURED: code-size parity confirmed
Built a throwaway crate (same target/profile/`memory-fr2433.x`, `opt="s"`+LTO, pinned nightly) doing
identical work two ways: a hand-written incremental state machine vs the same work as `async fn` +
a `core`-only no-op-waker join executor. `msp430-elf-size`:

| Build | text | data | bss | FRAM | static SRAM |
|---|---|---|---|---|---|
| hand-written state machine | 170 | 0 | 0 | **170 B** | 0 |
| async + `core` executor | 174 | 0 | 0 | **174 B** | 0 |

**+4 bytes code, 0 static RAM.** The symbol dump shows the whole substrate (both futures, `yield_now`,
waker, `Context`, join loop) collapsed under LTO into a single 64-byte `run` — the async machinery
resolves at compile time to ~the same code the hand version emits. Built on pure `core` / the pinned
nightly, confirming no toolchain blocker. **Caveats:** (1) this isolates the *substrate* cost; real
firmware scales with the number of task state machines (the same ones you'd hand-write). (2) `bss=0`
because the stack-pinned futures keep their state on the **stack**, which `size` doesn't measure — so
*static*-RAM overhead is zero, but **stack-resident future-state depth is not yet quantified**; do a
`-Z emit-stack-sizes` / high-water-mark pass before a full migration. Spike kept at
`scratchpad/async-spike/`.

## Consequences
- **Consistent model, right-sized runtime per tier**: one async/await programming style; embassy on
  Cortex-M, `core`-only `sched` on MSP430. Answers the "consistent task execution" question without
  forcing embassy onto a part it doesn't fit.
- **prod convergence**: prod stops hand-rolling three loops and adopts `sched` (removing the dead dep);
  the async form also makes the ADR-0006 "serve-slave-while-scanning" interleave fall out of `.await`
  instead of a hand-written `ScanJob`.
- **No new toolchain constraint**: `core`-only async builds on the pinned nightly; we are not coupled
  to embassy's MSRV.
- **Costs**: we author the async substrate (executor, eUSCI/timer leaves, the LPM waker) and must
  size-measure on FR2433. Until the spike proves parity, `sched` stays run-to-completion as-is.
- **Follow-up (open)**: (1) prototype the `core`-only async `sched` + size spike on FR2433; (2) arm the
  prod WDT backstop and de-duplicate its serve loops regardless of the async decision (those are
  independent robustness fixes); (3) if the spike passes, migrate diag + prod task bodies to `async fn`.
</content>
