#![no_std]

//! Cooperative, run-to-completion task scheduler for stem.
//!
//! The model: there is one stack and no preemption. Each task's [`Task::poll`] runs **one short,
//! non-blocking step** and returns; the scheduler calls it again when its next period is due.
//! Anything that would "wait" (an I²C reply, an animation frame delay, a comms ACK) becomes a
//! *state held in the task*, not a spin — so N concerns interleave over time on a 1 MHz MCU
//! without threads, an allocator, or an RTOS.
//!
//! This crate is deliberately **hardware-agnostic**. It never references a PAC, a timer, or a
//! chip: the caller owns those, passes its own context `C` (peripherals + a cross-task
//! blackboard) into every `poll`, and passes the current time (`now`, monotonic milliseconds)
//! into [`tick`]. That keeps one copy of the scheduler correct for FR2433, FR2476, the examples,
//! and host unit tests — matching the repo's "must not depend on FR2476-only features" and
//! "no HAL" conventions.
//!
//! # Contract
//! - **Never block in `poll`.** A task that spins stalls *every* other task (no preemption). The
//!   hardware watchdog is the backstop for a task that violates this; it is not a licence to block.
//! - **Private vs shared state.** A task's own bookkeeping lives in `self` (`&mut self`). State
//!   two tasks share (e.g. one produces a reading another consumes) lives in the context `C` —
//!   the explicit, typed blackboard — never in globals.
//!
//! # Example
//! ```ignore
//! struct Cx<'a> { p: &'a Peripherals, motion: bool }
//! struct Sampler { prev: bool }
//! impl<'a> sched::Task<Cx<'a>> for Sampler {
//!     fn poll(&mut self, cx: &mut Cx<'a>, _now: u32) -> Option<u32> {
//!         cx.motion = read_pin(cx.p);          // publish to the blackboard
//!         Some(if cx.motion { 5 } else { 50 }) // variable cadence: fast when active
//!     }
//! }
//!
//! let mut s = Sampler { prev: false };
//! let mut slots = [sched::Slot::every(50, &mut s)];
//! loop {
//!     let now = clock.now_ms();               // caller's time base
//!     sched::tick(&mut slots, &mut cx, now);
//! }
//! ```

/// A cooperative task, generic over the application context `C` it is handed each step.
///
/// Implement this for any struct that wants to be scheduled. `C` is whatever the app defines —
/// typically a struct holding the peripherals handle and a shared blackboard. The trait is
/// object-safe, so a heterogeneous set of tasks lives in one `[Slot<C>]` via `&mut dyn Task<C>`.
pub trait Task<C> {
    /// Run one non-blocking step at time `now` (monotonic ms). Return the delay in ms until this
    /// task next wants to run, or `None` to keep its current period. Must return promptly — see
    /// the crate-level contract.
    fn poll(&mut self, cx: &mut C, now: u32) -> Option<u32>;

    /// Short label for logging/diagnostics. Optional.
    fn name(&self) -> &'static str {
        "task"
    }
}

/// One scheduled task plus its timing state. Build with [`Slot::every`] and hand a `&mut [Slot]`
/// to [`tick`]. Holds the task by `&mut dyn Task<C>`, so the caller owns the task instances and
/// this crate stays alloc-free.
pub struct Slot<'a, C> {
    task: &'a mut dyn Task<C>,
    due: u32,
    period: u32,
    // Deadline instrumentation (ms, derived from `now` — no hardware). Cheap always-on telemetry:
    // how well the cooperative loop is meeting each task's cadence under real load.
    runs: u32,
    max_late: u32, // worst observed lateness = actual-run-time − scheduled-due-time
    overruns: u32, // times a task ran more than a full period late (i.e. it was starved past its slot)
}

impl<'a, C> Slot<'a, C> {
    /// Schedule `task` to run every `period_ms`. It first runs on the next `tick` (due = 0), then
    /// on the cadence it returns from `poll` (or `period_ms` while it returns `None`).
    pub fn every(period_ms: u32, task: &'a mut dyn Task<C>) -> Self {
        Self { task, due: 0, period: period_ms, runs: 0, max_late: 0, overruns: 0 }
    }

    /// The task's label (for logging).
    pub fn name(&self) -> &'static str {
        self.task.name()
    }

    /// Times this task has run.
    pub fn runs(&self) -> u32 {
        self.runs
    }

    /// Worst lateness seen (ms): how far past its scheduled deadline the task actually ran. Grows
    /// when another task hogs the loop (cooperative = no preemption) — the direct measure of the
    /// "task running too long" risk.
    pub fn max_late(&self) -> u32 {
        self.max_late
    }

    /// Times the task ran more than a full period late — a genuine missed slot, not just jitter.
    pub fn overruns(&self) -> u32 {
        self.overruns
    }
}

/// Run every slot whose deadline has arrived at time `now`, in array order. Returns how many ran
/// this pass (0 when nothing was due). Call it in a tight loop with a fresh `now` each time.
///
/// Timing is **best-effort, non-accumulating**: a task that runs late reschedules from `now`
/// (`due = now + period`), so a slow pass causes a one-off skew, never a catch-up burst. The
/// due-comparison is wrap-safe across the full `u32` millisecond range.
pub fn tick<C>(slots: &mut [Slot<'_, C>], cx: &mut C, now: u32) -> u16 {
    let mut ran = 0;
    for s in slots.iter_mut() {
        // Wrap-safe "now >= due": the unsigned difference lands in the low half of the u32 range
        // exactly when `now` is at or past `due` (within ~24.8 days), and in the high half when
        // `due` is still in the future. Handles the counter wrapping without special-casing it.
        let late = now.wrapping_sub(s.due);
        if late < 0x8000_0000 {
            // Deadline telemetry (before poll, against the period that scheduled this due).
            s.runs = s.runs.wrapping_add(1);
            if late > s.max_late {
                s.max_late = late;
            }
            if late > s.period {
                s.overruns = s.overruns.wrapping_add(1);
            }
            let next = s.task.poll(cx, now).unwrap_or(s.period);
            s.period = next;
            s.due = now.wrapping_add(next);
            ran += 1;
        }
    }
    ran
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────
// Async primitives (ADR-0013) — the SAME cooperative, never-block, one-stack model, but a task body
// can be written as an `async fn … .await` and the compiler generates the state machine (vs the
// hand-written [`Task`] above). Additive: a build uses `Task`/[`tick`] OR these, or both.
//
// There is no task arena and no real wake machinery: this is a cooperative executor that RE-POLLS its
// futures every main-loop pass, so a `Waker` need do nothing. The caller pins its task futures
// (`core::pin::pin!`), builds a `Context` from [`noop_waker`], and polls them each pass — exactly as
// the hand-written loop calls `Task::poll`. `.await`ing [`yield_now`] is the async form of a
// cooperative step returning so its peers get a turn. Pure `core`: no alloc, no embassy. See the
// measured cost + the borrow rule ("never hold shared `&mut` state across `.await`") in ADR-0013 and
// docs/TASK-STYLE-GUIDE.md.
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

static NOOP_VTABLE: RawWakerVTable = RawWakerVTable::new(|_| NOOP_RAW, |_| {}, |_| {}, |_| {});
const NOOP_RAW: RawWaker = RawWaker::new(core::ptr::null(), &NOOP_VTABLE);

/// A no-op [`Waker`]: this executor re-polls every pass, so `wake()` is inert. One shared vtable (4
/// fn pointers) — the entire runtime cost beyond the generated futures (ADR-0013 spike: ~free).
pub fn noop_waker() -> Waker {
    // SAFETY: NOOP_VTABLE's fns are all no-ops / return the same no-op RawWaker; the data ptr is
    // null and never dereferenced. This satisfies the RawWaker contract trivially.
    unsafe { Waker::from_raw(NOOP_RAW) }
}

/// Yield control back to the executor exactly once: `Pending` now, `Ready` on the next poll. The
/// async equivalent of a cooperative [`Task::poll`] returning so its peers run (the never-block
/// contract, ADR-0006). `.await` this between units of work in a long async task.
pub async fn yield_now() {
    struct Yield(bool);
    impl Future for Yield {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            if self.0 {
                Poll::Ready(())
            } else {
                self.0 = true;
                Poll::Pending
            }
        }
    }
    Yield(false).await
}
