# Pattern + investigation — MSP430 interrupt-driven LPM wake for an I²C slave

How a `stem` node (the `deepsleep` module in **Passive**) sleeps in a low-power mode and wakes on its
eUSCI_B **I²C-slave** interrupt to service a master, then resleeps — the deepsleep posture
(DEEPSLEEP-EXTRACTION.md §3). This doc is both the **reusable pattern** (the dual parts will need the
same for their UCB1/UCB0 slaves) and the **investigation record** for how we arrived at it, so neither is
lost. Grounded in slau445i (saved: `datasheets/User's Guide slau445i.pdf`) and the firmware-architecture
sources (`docs/references/firmware-architecture.md`). First implemented for **fr2433** in `prod/src/main.rs`.

---

## The pattern (copy this for any MSP430 eUSCI_B slave that must wake from LPM)

**1. Hand-rolled `br` wake trampoline — NOT the `msp430-rt` `#[interrupt(wake_cpu)]` macro.** A naked
`extern "msp430-interrupt"` fn exported as the vector symbol clears the LPM bits in the *stacked* SR and
**`br`s** (absolute, full-range) to the real handler:
```rust
#[unsafe(naked)]
#[no_mangle]
unsafe extern "msp430-interrupt" fn USCI_B0() {        // the vector symbol (dual: EUSCI_B1)
    core::arch::naked_asm!(
        "bic.b #0xF0, 0(r1)",   // clear SCG1|SCG0|OSCOFF|CPUOFF in the pushed SR (r1=SP) → wake on RETI
        "br #{inner}",          // ABSOLUTE branch (R_MSP430_16) — placement-independent, LTO-safe
        inner = sym usci_b0_inner,
    );
}
extern "msp430-interrupt" fn usci_b0_inner() { /* … reti emitted by the compiler … */ }
```
Why hand-rolled: the macro's `wake_cpu` is correct in intent but emits a short `jmp {inner}`
(R_MSP430_10_PCREL, ±1 KB) that **truncates under LTO** (see investigation below). `br` has full range.
Clearing `0xF0` (SCG1|SCG0|OSCOFF|CPUOFF) on the stacked SR is what makes the CPU return to *active* mode
after the handler's `reti`.

**2. The ISR does NO byte work and never touches the register file.** The inner handler only **masks its
own IE** (`bsp::stem_i2c::irq_disable`) so the still-latched IFG won't immediately re-fire while the CPU
is awake, then returns. The register file stays in the main context → **no critical section** needed
(firmware-architecture.md §4-5; Ganssle/Koopman/Barr).

**3. The serve loop: service → arm wake → atomic sleep.**
```rust
loop {
    bsp::board::wdt_arm_backstop(p);        // backstop guards the AWAKE service window
    stem::poll(p, &mut slave, &mut rf);     // drains next_event() directly off IFG/RXBUF/TXBUF (IE masked)
    let _ = rf.take_mode_request();
    msp430::interrupt::disable();           // close the GIE window FIRST …
    bsp::stem_i2c::irq_enable(p);           // … then re-arm the slave IE the ISR masked (no premature fire)
    bsp::board::wdt_hold(p);                // WDTHOLD across sleep (see §WDT)
    enter_lpm0_gie();                       // `bis #0x18, r2` — GIE on + CPUOFF, ATOMIC; wakes on slave ISR
}
```
Ordering matters: disable GIE **before** `irq_enable`, so no ISR fires between arming the IE and the
atomic sleep (which would mask the IE and then sleep forever). The single `bis #0x18,r2` enables GIE and
halts in one instruction — no lost-wakeup race (Samek's chicken-and-egg; firmware-architecture.md §1). A
pending IFG at that point fires immediately instead of sleeping.

**4. Watchdog vs LPM — `WDTHOLD` across sleep, backstop armed only while awake.** slau445i §12: in
reset (watchdog) mode the WDT_A has a **clock fail-safe** (ACLK/SMCLK loss → auto VLOCLK), so LPM does
**not** stop it — a sleeping node would be reset. And watchdog mode blocks LPMx.5. So: arm the backstop
only while awake (where a hang can actually occur — the CPU is running code), `WDTHOLD` it in the
idle→LPM step, re-arm on wake. A sleeping CPU executes nothing, so it can't hang and needs no backstop.
This is both TI's low-power guidance and Koopman's "kick tied to real work."

**5. LPM depth.** Start at **LPM0** (`CPUOFF` only — all clocks on; definitely wakes on the eUSCI ISR;
least risk). Deeper (LPM3/4; slau445i §24.3.10 says the slave wakes even from LPM4 with all clocks off)
is the production power target — tune once wake is proven on a master.

### Build wiring (device-mode interrupts — first ISR in this codebase)
- Consumer crate: `#![cfg_attr(feature = "<fam>", feature(abi_msp430_interrupt))]` (experimental ABI).
- `bsp` chip feature forwards `<pac>/rt` (emits the PAC `__INTERRUPTS` table); the bin forwards
  `msp430-rt/device` (so msp430-rt uses the PAC table, not its default).
- **ISR lives in the bin** (not a lib) — it's the vector-table root; a lib ISR risks dead-code
  elimination. The PAC does NOT re-export `#[interrupt]`.
- **`memory-<chip>.x` VECTORS for device mode must fit the FULL padded PAC `__INTERRUPTS` + reset.** For
  fr2433: the PAC emits 59 words + reset = **0x78 B**, ending at 0x10000 → `ORIGIN=0xFF88, LENGTH=0x78`
  (reset lands at 0xFFFE, PORT2 at 0xFFDA — hardware-correct; the 40 reserved words fill 0xFF88..0xFFD9,
  free vector FRAM above ROM END 0xFF7F). **`scripts/lint-memory-x.sh` is buggy** — it uses the lowest
  *real* vector (0xFFDA/0x26) and under-counts the reserved padding, so its origin overflows; the
  linker-observed section size is authoritative. TODO: fix the lint + `gen-memory-x.sh` to count the full
  PAC array.

---

## Investigation record (2026-10-06)

**Symptom.** Enabling `#[interrupt(wake_cpu)]` (msp430-rt 0.4.1) for the fr2433 slave ISR → link error:
`relocation truncated to fit: R_MSP430_10_PCREL against .text.…usci_b0_inner`.

**Root cause.** The `wake_cpu` naked trampoline branches to its handler with `jmp {inner}` — a 10-bit
PC-relative jump (±1 KB). Our release profile uses **fat LTO**, which whole-program-reorders `.text` and
places the handler >1 KB from the trampoline → the short `jmp` can't reach it. (Confirmed by disassembly:
the macro emits `jmp`; our image `.text` is ~4 KB.)

**Was it an upstream bug? Checked exhaustively (2026-10-06):** upstream `rust-embedded/msp430-rt` master,
latest published 0.4.1 (taiki-e's line), the feature author's own fork `RossPorter506/msp430-rt-wake-cpu`,
`taiki-e/msp430-rt`, `jlin/msp430-rt` — **all still emit `"jmp {inner}"`**; **no open PR or issue** about
it. So the fix exists nowhere. BUT (Henrik's insight) nobody hit it because the normal compile style keeps
the handler within the `jmp`'s 1 KB reach — it's the **macro's short `jmp` × our fat LTO** that trips it.
So it's fixable in *our* code, not only by patching the dependency.

**Decision — hand-rolled `br` trampoline in our own code (above).** Self-contained (no `[patch]`, no
vendored crate), old-Rust-safe (compiles on nightly-2025-06-25), LTO-safe. Verified: builds + links with
LTO ON, in budget (fr2433 4332/15360 B); disassembly shows `bic.b #240, 0(r1)` + `br #0x…` (absolute);
on `@fr2433` it boots, runs full init, enters LPM, and does **not** reset-loop (WDTHOLD-across-LPM holds).
**Not yet proven:** wake on a real master transaction (needs fr2433 on the Stem bus + the probe).

**Rejected alternatives:** patch `msp430-rt-macros` `jmp`→`br` (dependency surgery — keep as an *optional*
upstream PR for the community, not on our critical path); disable LTO (loses the size budget).

**Upstream follow-up (nice-to-have, not blocking):** file a `jmp`→`br` fix at
`rust-embedded/msp430-rt` so `wake_cpu` is robust under LTO for everyone; if merged + released we can drop
the hand-rolled trampoline and use the macro.

---

## Async executor layer (Phase 1c — ADR-0013 LPM pender)
The 1b serve loop above is the hand-written form. 1c expresses the SAME behaviour in the one async model
(so the dual multi-task reshape reuses it), with **no change to the trampoline/ISR/WDT/LPM mechanics**:
- `sched::Pender` — a real `Waker` over a single-core `volatile`-byte flag (MSP430 has no
  `core::sync::atomic`; keeps `sched` zero-dep). `sched::run_pended(fut, &PENDER, idle)` polls `fut`;
  when a round makes no progress and the flag is clear, it calls `idle` (the §3 sleep sequence).
- The dispatch is an `async fn serve` that loops `next_event().await` (a `poll_fn` leaf: `Ready(ev)` /
  `Pending` when the bus is idle) → `Slave::handle` (shared with the sync `poll`).
- The ISR inner adds **`PENDER.wake()`** next to the IE mask — so a wake racing the poll isn't lost.
- Cost on fr2433: +94 B vs the hand loop. Functionally identical; proven on `@fr2433` + the stem-probe.

## Reuse for the dual parts (DEEPSLEEP-EXTRACTION.md §Behaviour model)
In Passive the dual nodes are slaves on **both** buses → two slave ISRs: `EUSCI_B1` (Stem) and
`EUSCI_B0` (Leaf). Each gets the same hand-rolled `br` trampoline + `bsp::stem_i2c::irq_{enable,disable}`
(already present for the `_leaf` UCB1 variant). The `memory-<chip>.x` VECTORS for the dual chips need the
same device-mode resizing (lint reports fr2476 `0xFFCA`, fr2355/fr2155 `0xFFCE` for the *real* vectors —
re-derive the full-padded origin as for fr2433). Active keeps today's non-sleeping loop.
