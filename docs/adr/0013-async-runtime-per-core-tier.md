# ADR-0013 · Async runtime per core tier — embassy on Cortex-M/host, a `core`-only async `sched` on MSP430

Status: Accepted (tier split) · **In progress** (async-`sched` evolution — spikes DONE; FIRST subsystem migrated + hardware-verified: prod Leaf enumerate, 2026-10-06)
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

**Both authoring styles are supported — by ONE engine, not two.** `sched` becomes async-native; the
existing cooperative poll-step is then just the **degenerate case** — a one-line future (`poll_fn` /
an `async fn` that does a step and yields). So a task can be written either as a plain poll-step
(smallest; for trivial/periodic work) **or** as `async fn … .await` (for nested multi-step I/O),
and the *same* `sched` loop drives both. This is explicitly **not** two parallel runtimes (a
`Task::poll` engine beside an async executor) — that would mean two idioms, two driver conventions,
and a split loop to maintain; avoid it. One engine, cooperative = a trivial async task.

**Prerequisite (the real work):** async leaf drivers. `xfer(..).await` only exists if the
eUSCI/Timer_A ops are wrapped as futures over the existing bounded, never-hang blocking drivers (+ an
interrupt or a yield). That's shared infrastructure built once; without it the async style has nothing
to `.await`. The executor must be the one in `sched` (not a bespoke per-firmware loop), so diag + prod
**converge** instead of diverging further (today prod doesn't even use `sched`).

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

### Spike results (2026-10-05) — two models; the REALISTIC one is the one to trust

All builds: same target/profile/`memory-fr2433.x`, `opt="s"`+LTO, pinned nightly, pure `core`
executor (no embassy). Two models, hand-written state machines vs the same work as `async fn`:

**(a) Trivial substrate floor** (2 futures, one `.await` each): hand 170 B / async 174 B FRAM —
**+4 B, 0 RAM**. The whole substrate collapsed into one 64-B `run`. *But this is unrepresentative* —
too simple to exercise the per-future `poll`/resume/drop glue. Do not quote it as the real cost.

**(b) Realistic model** (3 concurrent machines: HIGH-pri Stem-slave service with a 16-B shadow;
LOWER-pri Leaf scan with a NESTED multi-`.await` `read_reg` + live buffer; mode/console):

| Dimension | hand | async | delta |
|---|---|---|---|
| **Code** (FRAM text) | 506 B | 1042 B | **+536 B (~2.1×)** |
| Static RAM (data+bss) | 0 | 0 | 0 |
| Persistent state, all 3 machines | 29 B | 51 B | +22 B |
| **Peak stack frame (shared, single)** | 28 B | 52 B | +24 B |

Per-machine state (`-Z print-type-sizes`): async leaf_scan 26 / slave 21 / mode 4; hand ScanJob 12
(Option<ReadReg> 10) / SlaveJob 16 / ModeJob 1.

**Findings:**
- **RAM model confirmed favorable — NO per-loop stacks.** Disassembly shows exactly ONE significant
  stack adjustment per binary (`sub #52,r1` async / `sub #28,r1` hand): the three machines live as
  locals in the single `run` frame (≈ the summed state), transient polls (`bus_*`, ~8–10 B leaves)
  layer shallowly on top. Parallel cost per machine = its small state struct (4–26 B), summed into
  one shared stack — not N reserved stacks. 0 static RAM.
- **Code size is the real cost, and it is NOT parity.** Async ~doubles the code here (+536 B) — the
  generated per-future poll/resume/drop glue + poll-after-complete guards for the nested
  `xfer→read_reg→leaf_scan` chain. On FR2433's 15 KB that's ~3.5% (prod ≈ 5 KB today, so affordable),
  but it **scales with the count + nesting depth of `async fn`s**. Budget it; don't assume free.
- Ergonomic win is real: the hand version needed an explicit `RrPhase` enum + `Option<ReadReg>`
  threaded through `ScanJob`; async expresses it as plain `.await`.

### RAM amplifier + mitigation (measured 2026-10-05)
RAM is favorable *by default* but async CAN eat significant SRAM: anything live **across an `.await`**
is resident while suspended. Measured (`-Z print-type-sizes`):

| Future | size | note |
|---|---|---|
| owns a `[u8;256]` across the await | **268 B** | the whole buffer sits in the future state |
| borrows `&mut [u8;256]` across the await | **16 B** | only the pointer is held; buffer lives once in run()'s frame |

Amplifies via **nesting** (top future = sum down the deepest await chain) and **`join`/concurrency**
(all suspended futures coexist → sum; N tasks each owning a 256 B buffer = N×268 B). It all lands in
the single executor stack frame (demo: `sub #580,r1`), so it reads as *stack*, not bss (static RAM 0).

**Mitigation = the existing `sched` discipline, carried to async:** keep scratch in shared
state/`&mut`, never own big locals across `.await` (268 B → 16 B just by borrowing); flatten nesting;
don't `join` when sequential suffices. The irreducible RAM is whatever must be live across suspension
summed over the concurrently-suspended set — a requirement hand code shares; async just makes
"held across await" explicit and easy to trip over.

**Verdict:** viable on MSP430 (pure `core`, no toolchain blocker, favorable RAM/stack model *with the
borrow discipline*), but code-size costs ~+0.5 KB per realistic subsystem — acceptable at prod's
current footprint, a real line item as async spreads. Decide per-subsystem, not wholesale. Spike kept
at `scratchpad/async-spike/`.

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
- **Follow-up**: (1) ✅ spike + size-measure done; (2) ✅ prod WDT backstop armed + serve loops
  de-duplicated; (3) **✅ first subsystem migrated — prod Leaf enumerate** (`enumerate::scan_async` +
  `leaf_scanner`, driven by `sched::{yield_now, noop_waker}`; hardware-verified on `@fr2355`:
  Sensing-switch arms the async scan, it completes with the Stem slave responsive throughout, +330 B
  silent / within budget). This also made prod's previously-dead `sched` dependency live. (4) OPEN:
  migrate further subsystems case-by-case (each ~+0.5 KB); build the interrupt-driven async eUSCI/timer
  leaves when a subsystem needs true `.await`-on-IFG (enumerate used blocking-probe-then-yield); the LPM
  pender for real sleep-between-events.
</content>
