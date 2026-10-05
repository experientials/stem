# Task coding style — cooperative `sched` vs `async`/`.await`

> How to write concurrent work on the MSP430 tier (diag + prod). Two styles are allowed; this guide
> shows both, their rules, the SAME logic written each way, and how to choose. Both serve one
> contract. Anchors: [adr/0006](adr/0006-mode-switch-must-not-block-control-channel.md) (never block
> the control channel), [adr/0013](adr/0013-async-runtime-per-core-tier.md) (runtime per core tier +
> the measured async cost), [`crates/sched`](../crates/sched/src/lib.rs). For the Cortex-M/host tier
> use **embassy** (see `stem-probe`); this guide is the MSP430 tier only.

---

## The one rule both styles serve: never block the control channel (ADR-0006)

There is one stack, no preemption, no RTOS. A task runs **one short, non-blocking step** and returns;
long work is carried as **state across steps**, never a spin. A pass that blocks stalls *every* other
concern — the Stem I²C slave (the SoM's only way in) and the console included. The watchdog backstop
(`pet_wdt` each pass, ~16 s) is the safety net for a step that violates this, **not** a licence to
block.

Everything below is two ways of honouring that rule. Style A makes the state explicit; Style B lets
the compiler generate it.

---

## Style A — cooperative state machines (`sched::Task` / hand-written) — the CURRENT default

Work is a struct that holds its own progress; each `poll`/`step` advances it a little and returns.

**The `sched` trait** ([crates/sched](../crates/sched/src/lib.rs)) — for periodic, multi-task work (diag):

```rust
impl<'a> sched::Task<Cx<'a>> for Sampler {
    // ONE short step at `now` (ms). Return the delay until next run, or None to keep the period.
    fn poll(&mut self, cx: &mut Cx<'a>, _now: u32) -> Option<u32> {
        cx.motion = read_pin(cx.p);          // publish to the shared blackboard (cx), not a global
        Some(if cx.motion { 5 } else { 50 }) // variable cadence: fast when active
    }
}
let mut slots = [sched::Slot::every(50, &mut sampler), /* … */];
loop { pet_wdt(&p); sched::tick(&mut slots, &mut cx, clock.now_ms(&p)); }
```

**A hand-written incremental job** — for long I/O that must not block (prod's Leaf enumerate,
[enumerate.rs](../prod/src/enumerate.rs) `ScanJob`; the loop advances it a few probes per pass):

```rust
let mut job: Option<ScanJob> = None;         // armed on a Sensing switch
loop {
    pet_wdt(p);
    stem::poll(p, &mut slave, &mut rf);       // service the slave FIRST, every pass
    pump_scan(p, …, &mut job);                // advance the scan a BUDGET of probes, then yield
}
```

### Rules — Style A
- ✅ **Never block in a step.** Bounded loops only (leaf drivers use a `SPIN` cap — see
  [i2c.rs](../crates/bsp/src/i2c.rs)); a stuck bus returns an error, never hangs.
- ✅ **State that persists across steps lives in `self`** (the job's own fields: a phase enum, an
  index, a small buffer).
- ✅ **State two tasks SHARE lives in the context `C`** (the typed blackboard), **never in a global.**
- ✅ **Long work → an incremental job** advanced a bounded chunk per pass (ADR-0006), so the slave +
  console keep breathing.
- ✅ Return the next cadence (`Some(ms)`); self-adjust it (fast when active, slow when idle).
- ❌ Don't do a full multi-step transaction inline in one pass if it can block the slave.

### When to use Style A
Periodic multi-task scheduling (diag's radar/prox/post/ui/stress), and any step that is already a
simple one-shot. It's the smallest possible footprint (no executor, no futures) — the baseline.

---

## Style B — `async`/`.await` on a `core`-only executor — EMERGING (ADR-0013)

The same cooperative model, but the compiler generates the state machine from `async fn`. **No
embassy, no alloc, pure `core`** — builds on the pinned nightly. A minimal executor polls the futures:

```rust
use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

// No-op waker: this executor re-polls every pass, so wake() need do nothing (one shared vtable).
static VTABLE: RawWakerVTable = RawWakerVTable::new(|_| RAW, |_| {}, |_| {}, |_| {});
const RAW: RawWaker = RawWaker::new(core::ptr::null(), &VTABLE);

async fn yield_now() { /* Pending once, Ready next poll — hands control back */ }

async fn slave_service(/* borrows shared state */) -> ! {
    loop { /* service one slave event */ yield_now().await; }
}

pub fn run() -> ! {
    let mut hi = pin!(slave_service());          // futures pinned in this frame (no heap)
    let mut scan = pin!(leaf_scan());
    let w = unsafe { Waker::from_raw(RAW) };
    let mut cx = Context::from_waker(&w);
    loop {
        pet_wdt();
        let _ = hi.as_mut().poll(&mut cx);        // poll order = the only "priority" (cooperative)
        let _ = scan.as_mut().poll(&mut cx);
    }
}
```

### Rules — Style B (the ones that matter on a 4 KB / 15 KB part)
- ✅ **Borrow, don't own, large buffers across `.await`.** Anything live across a suspend point sits
  in the future for the whole suspension. Measured (ADR-0013): owning `[u8;256]` across an await =
  **268 B** future; passing `&mut` = **16 B**. Keep scratch in shared state / a `&mut` param.
- ✅ **Don't hold big locals across `.await`** at all if avoidable — read into a buffer, process,
  *then* await; or await, then use a fresh scope.
- ✅ **Flatten nesting.** A future embeds the full state of whatever it `.await`s (the top future =
  sum down the deepest await chain). Make non-awaiting helpers **plain `fn`s** so their locals don't
  persist in the parent future.
- ✅ **`join`/concurrency sums** — all simultaneously-suspended futures are resident at once. Prefer
  sequential `.await` when you don't actually need concurrency; reserve `join` for genuinely
  concurrent branches.
- ✅ Pure `core` only — `core::{future,task,pin}`, `pin!`, a no-op (or interrupt-set-flag) waker. **No
  `alloc`, no embassy** (embassy has no MSP430 target; ADR-0013).
- ✅ Still `pet_wdt()` every executor pass; still never block inside a `poll` between awaits.
- ❌ No `async` recursion (needs boxing → alloc → unavailable).
- ❌ Don't reach for embassy on MSP430 — wrong tier.

### When to use Style B
A subsystem whose hand-written state machine is painful — **nested, multi-step I/O** (e.g. a register
read = addr → reg → repeated-START → N bytes, each a suspend). There `.await` replaces an explicit
phase enum + index threading. Adopt it **per-subsystem** where the ergonomics earn the code cost
(below), not wholesale.

---

## The same logic, both ways — a multi-byte register read

**Style A (hand-written phase machine)** — explicit state, one transfer per `step`:

```rust
enum RrPhase { Addr, Reg, Restart, Read(usize), Done }
struct ReadReg { phase: RrPhase, addr: u8, reg: u8, buf: [u8; N] }
impl ReadReg {
    fn step(&mut self) -> bool {         // true when done; called once per pass
        match self.phase {
            RrPhase::Addr    => { xfer(self.addr << 1);       self.phase = RrPhase::Reg; }
            RrPhase::Reg     => { xfer(self.reg);             self.phase = RrPhase::Restart; }
            RrPhase::Restart => { xfer((self.addr << 1) | 1); self.phase = RrPhase::Read(0); }
            RrPhase::Read(i) => { self.buf[i] = xfer(0xFF);
                                  self.phase = if i+1 < N { RrPhase::Read(i+1) } else { RrPhase::Done }; }
            RrPhase::Done    => return true,
        }
        matches!(self.phase, RrPhase::Done)
    }
}
```

**Style B (async)** — the compiler generates the identical state machine:

```rust
async fn read_reg(addr: u8, reg: u8, buf: &mut [u8; N]) -> bool {  // buf BORROWED, not owned
    xfer(addr << 1).await;
    xfer(reg).await;
    xfer((addr << 1) | 1).await;
    for slot in buf.iter_mut() { *slot = xfer(0xFF).await; }
    buf[0] != 0xFF
}
```

Same work, same cooperation (one transfer per suspend), same RAM for the buffer (borrowed in both).
Style B is the logic; Style A is the logic *plus* the hand-maintained `RrPhase`/index bookkeeping.

---

## Choosing between them — the measured cost (ADR-0013)

Per-core-tier is settled: **embassy on Cortex-M/host; these two styles on MSP430.** Within MSP430,
choose per subsystem using the measured deltas (realistic 3-machine model, fr2433 profile):

| | Style A (hand `sched`) | Style B (async `core`) |
|---|---|---|
| Code | baseline | **~+0.5 KB per realistic subsystem** (poll/resume/drop glue; scales with nesting) |
| Static RAM | 0 | 0 |
| Per-machine state | small struct | small future (≈ same order; a few bytes more) |
| Stack | one shared frame | one shared frame (no per-task stacks) |
| RAM foot-gun | — | **owning a buffer across `.await`** (268 B vs 16 B borrowed) |
| Ergonomics | you write the state machine | compiler writes it |

**Default:** Style A for simple/periodic work and anything already one-shot. **Reach for Style B** when
a subsystem is a genuinely nested multi-step I/O state machine that's error-prone by hand — and the
~0.5 KB fits the budget. When in doubt on a 15 KB FR2433 image, measure with the size gate.

---

## Conventions both styles share (non-negotiable)

- `#![no_std]`, **no `alloc`**. One stack, no preemption.
- **PAC-agnostic where it can be:** the scheduler/executor and shared logic take the peripherals +
  shared state as parameters (the `sched` context `C`, or `&mut` args), so one copy compiles for every
  MSP430 family (STEM-DIRECTION / the chip-portability convention). Chip-specific glue is isolated.
- **Leaf drivers never hang:** bounded `SPIN` waits; a stuck bus returns an error
  ([i2c.rs](../crates/bsp/src/i2c.rs)).
- **`pet_wdt()` every pass** (the ~16 s reset-mode backstop, [prod/src/main.rs](../prod/src/main.rs) /
  [diag/src/main.rs](../diag/src/main.rs)). Deterministic bring-up runs unguarded; the loop is guarded.
- **State ownership:** private → `self`/the future; shared → the context `C` / a borrow; **never a
  mutable global.**
</content>
