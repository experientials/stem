# FR2355 I²C bring-up & fault-isolation plan — UCB0 on P1.2/P1.3, 100 kHz, master **and** slave

> **Status: FR2355 wedge FIXED; root electrical trigger still OPEN (2026-10-05).** The Leaf master
> was not fundamentally broken. The new `recover()` reliably clears the FR2355 wedge — but an A/B test
> (below) proved the wedge is **bench-specific to FR2355**, not a shared-code bug (FR2476 runs clean on
> the *legacy* recover with the same device). So the fix **masks an electrical problem** whose root
> cause (likely pull-up strength / P1.2·P1.3 loading) is **not yet identified**.

## Resolution (2026-10-05) — recover() was blind to SCL-low wedges (real gap; FR2355-only trigger)

**What happens.** On the FR2355 bench, a read leaves our own eUSCI_B0 **holding SCL low** (master not
idle). The old `recover()` early-returned whenever **SDA** was high ([i2c.rs](../crates/bsp/src/i2c.rs)),
so it never cleared an **SCL-low** wedge; and every transaction's recovery gate (`if !sda_high`) only
checked SDA. Net: one wedged read poisoned every following one — the POST inventory failed and the bus
false-ACKed ~82 phantom addresses. **Caveat (see A/B below):** this wedge is triggered by the FR2355
bench's electricals, *not* by the shared code — FR2476 with the same device + legacy recover never
wedges. The fix is correct and worth keeping, but it is masking an unidentified electrical trigger.

**Fix** (in [crates/bsp/src/i2c.rs](../crates/bsp/src/i2c.rs), shared by prod + diag):
1. `recover()` now asserts **UCSWRST** first — the module releases SDA/SCL, clearing a master-held
   SCL outright — then does the 9-clock dance only if a *slave* still holds SDA; a slave stretching
   SCL is reported, not fought. It preserves the caller's UCSWRST state.
2. The per-transaction gate is now `if !bus_idle(p)` (both lines), so an SCL-low wedge triggers
   recovery instead of being missed.

**Verified on hardware** (`@fr2355` 8E57…, `g` = signature battery, 2026-10-05):
- Before: 82 phantom false-ACKs, all register reads FAIL, bus wedged (`P1 idle SDA=1 SCL=0`).
- After: `entry: bus recovered idle-high`; `P1 SDA=1 SCL=1`; exactly **1 real device (0x20), zero
  phantoms**; **P5 repeated-START OK**; P3/P4/P6 OK; P8 ladder OK at 10/25/50/100 kHz; **repeatable
  back-to-back** with no wedge return.

**Caveats / open items:**
- This bench has **only the PCA9698 @0x20** on Leaf; the "7/7 present" seen pre-fix was the wedge
  false-ACKing the expected-sensor address slots. **Real sensors must be connected** to validate P5
  against an actual WHO_AM_I read.
- P3 wrote `0xA5` to PCA9698 reg 0x10 but P5 reads back `0x00` — **transport is proven**, but the
  PCA9698 register read-back (AI/command-byte semantics) needs a separate check; the `stress.rs`
  write-verify may be hitting the same thing.
- The fix is defensive (recover-per-transaction). The *trigger* (why a read leaves SCL low) is worth
  a scope pass on P5, but the bus is now robust against it regardless.

### Principles that led to the fix (reuse these)

1. **Bisect, don't poke.** Every step eliminated a *class* of causes (physical → mux → clock →
   master-vs-slave → FSM). A day was lost poking; the fix came in two instrumented runs.
2. **Observe the bus before editing it.** The on-chip idle-level readout + the signature battery
   turned an opaque "doesn't work" into "SCL is low at P1, repeated-START is the failing stage."
   Instrumentation first, edits second.
3. **Deterministic signature patterns with known expected output.** Each pattern (P1–P8) has one
   decoded signature; the *first* deviation localizes the fault. `P1 SCL=0` named the wedge mode;
   `P5` named the failing transaction shape.
4. **Distinguish the symptom, the trigger, and the premise.** Symptom = reads fail; trigger = a read
   leaves SCL low; premise ("master doesn't work") was *wrong* — basic mastering worked all along.
   Reject the stated framing until the evidence confirms it.
5. **A harsh bench is the better test.** The fault only surfaced on a bus with an absent/flaky device
   profile (failed reads). Healthy benches hide latent recovery bugs (see FR2476 below).
6. **Prove on hardware, repeatably.** Byte-verify ≠ runs; one pass ≠ robust. Back-to-back clean runs
   with no wedge return is the bar.
7. **Promote the probe into a regression.** The signature battery stays as the standing bring-up/HIL
   check so this can't silently regress.

### Why FR2476 "worked" — corrected by an A/B falsification test (2026-10-05)

`recover()` has been **SDA-only since the day it was written** (commit `3844197`, on FR2476/FR2433),
and diag+prod share **one** `i2c.rs` in `bsp` across all chips — so FR2476 never had a separate fix.

**I first hypothesised the SCL-low wedge was chip-independent (same peripheral, same code) and merely
latent on FR2476. Henrik proposed the falsification: put the same PCA9698 on FR2476 and run the
*legacy* (SDA-only) recover — if the theory holds, it must wedge there too. It did NOT.** Measured:

| Bench | pull-ups | recover() | Result |
|---|---|---|---|
| FR2355 | added 5.6 kΩ | legacy (SDA-only) | **WEDGED** — SCL=0, 82 phantom ACKs, reads fail |
| FR2355 | added 5.6 kΩ | fixed | clean |
| FR2476 | LaunchPad onboard | fixed | clean |
| FR2476 | LaunchPad onboard | **legacy (SDA-only)** | **CLEAN** — P1 SCL=1, P5 OK, no wedge |

**Corrected conclusion:** the SCL-low wedge is **bench/electrical-specific to the FR2355 setup, not a
property of the shared code.** The legacy recover gap is real, and the new `recover()` genuinely
clears the FR2355 wedge — but it is **masking an electrical problem on the FR2355 bench**, not fixing
a bug that FR2476 also had. FR2476 (same extender, no sensors, SDA-only recover) never produces the
wedge at all. My earlier "latent in FR2476 too" claim was **wrong** and this corrects it.

**The remaining open question — what triggers the wedge on FR2355 specifically.** Prime suspects:
1. **Pull-up strength / rise time** — FR2355 runs on an *added* 5.6 kΩ; the FR2476 LaunchPad supplies
   its own (likely stronger) pull-ups, so FR2355's SCL edges may be marginal enough to desync the
   eUSCI into holding SCL low.
2. **P1.2/P1.3 loading on the FR2355 LaunchPad** — those pins are also `OA0±`/SAC inputs, possibly a
   second load/capacitance the FR2476 board doesn't have.
3. **Per-board hardware fault** (Henrik, 2026-10-05): dev boards are dirt cheap, and the unit that
   "worked before" was an *identical board from the same batch, not the exact same board*. A fault on
   *this* FR2355 unit (`8E57…`) — cold joint, marginal pin, damaged pad — is a live candidate.

Discriminating tests: fit **2.2 kΩ** on the FR2355 bench and re-run *legacy* (if the wedge vanishes →
rise-time); run the same extender+pull-ups on **another FR2355 board** from the batch on *legacy* (if
clean → the `8E57` unit is faulty; if it also wedges → model/pull-up, not per-board); **scope the SCL
rise** on FR2355 vs FR2476.

**Framing the fix given production faults are expected.** Because the boards are cheap and real units
*will* ship with marginal/faulty buses, **robust recovery is a requirement, not a nicety** — so keep
the `recover()` fix as **defensive hardening** regardless of whether this board's trigger turns out to
be pull-ups, LaunchPad loading, or a one-off fault. The fix is justified on its own terms; the root
trigger still deserves identifying so we know whether it's a bench artifact or a real FR2355-class
electrical margin that the *product* layout must design against.

Lesson (revised): *"works on the known-good bench" tells you nothing about the recovery path — but
neither does "the code is shared" prove a fault reproduces everywhere. And "it worked before" may be a
different physical board. Test the actual variable on the actual unit; the bus is an electrical system,
not just firmware.*

### What this means for the prod firmware's I2C capability

prod uses the same `bsp` driver, so it **inherits the `recover()`/gate fix automatically**. But the
review surfaced that **prod's own fault logic replicates the exact SDA-only blind spot**:

- [prod/src/enumerate.rs:69-70](../prod/src/enumerate.rs#L69) — `let (sda0,_) = bus_levels(p); let
  recovered = sda0 || recover(p);` → if **SDA is high but SCL is low**, it short-circuits and
  **never calls `recover()`**. An SCL-low wedge from a prior failed read survives into the scan.
- [prod/src/enumerate.rs:82-83](../prod/src/enumerate.rs#L82) — `faulted = !recovered || !sda1 || …`
  judges the bus by **SDA only**, so an SCL-low wedge is **not even flagged** `BUS_WEDGED` — the SoM
  would read a false "bus OK." (`status.rs` already has `fault::BUS_WEDGED` and withholds `BUS_OK` on
  fault — the *reporting* surface is right; the *detection* is SDA-blind.)

**Required prod I2C capability (the standard this episode defines):**
1. **Both-line wedge model.** Detect and recover on *either* SDA-low (slave) **or** SCL-low (master
   self-hold). Fix `enumerate.rs` to gate/fault on `bus_idle` (both lines), mirroring the driver.
2. **Fix the trigger, not just the symptom.** The read/probe error path (NACK/timeout) must always
   issue STOP + resync so it never *leaves* SCL low. Recover-per-transaction is the safety net, not
   the primary mechanism.
3. **Never-hang** — bounded spins everywhere (already true). A missing/flaky sensor must fail fast,
   not stall the supervisor.
4. **Honest fault observability over silent retry.** A persistently wedged bus must surface as
   `BUS_WEDGED` to the SoM (withholding `BUS_OK`/presence), not be masked by endless recovery. Don't
   report presence data gathered on a wedged bus.
5. **Hot-unplug / absent-device tolerance** as a first-class state — the sensor bus will routinely
   have devices come and go; that is normal operation for the supervisor, not an error to hang on.
6. **Distinguish "absent" from "wedged."** The `SANITY_MAX` all-ACK canary (enumerate) + the SCL
   check together separate "no device answered" from "bus is stuck" — both must be reported distinctly.

---

> **Original status (pre-fix):** The Leaf bus (eUSCI_B0 → P1.2 SDA / P1.3 SCL) does not yet
> work as master, and we also need the same pin pair to work as a slave (FR2355↔FR2355 and
> RP2040↔FR2355). This is unresolved after a full day. This doc is the **methodical** plan to end it.
>
> **Governing principle — bisect, don't poke.** A day was lost because the bus was changed without
> being *observed decoded*, and the master/slave halves were never *isolated against a known-good
> partner*. Every step below is designed to **eliminate a whole class of causes**, with an explicit
> pass/fail and a "if-fail-then". Do them in order. Do not skip to firmware edits.
>
> Verified facts this plan rests on (datasheet SLASEC4 + repo source, 2026-10-04):
> - P1.2→UCB0SDA and P1.3→UCB0SCL require **PxSEL = 01** (primary function); `P1SEL0=1, P1SEL1=0`.
>   The FR59xx examples use `P1SEL1` (their eUSCI is the *secondary* function) — do **not** copy them.
> - P1.2 also = `OA0−/A2/Veref−`, P1.3 also = `OA0+/A3` (SAC + ADC). Must be inactive on these pins.
> - Device `fSCL(max) = 400 kHz`. We run **100 kHz** (BRW=10 off 1 MHz SMCLK). 5.6 kΩ pull-ups are
>   in-spec for 100 kHz up to ~210 pF of bus capacitance.
> - eUSCI in module mode **overrides the internal P1REN pull-ups** → external pull-ups are mandatory.
> - The repo firmware ([crates/bsp/src/i2c.rs](../crates/bsp/src/i2c.rs), [prod/src/main.rs](../prod/src/main.rs))
>   already clears LOCKLPM5, uses `P1SEL0=01`, wraps UCSWRST, selects SMCLK, and implements 9-clock
>   stuck-bus recovery. **The obvious software init is correct** — so the fault is elsewhere.

---

## 0. Instrumentation — mandatory before any more edits

You cannot debug I²C blind. Set these up first; each phase references them.

| Tool | Role in this plan | Where |
|---|---|---|
| **Decoded bus trace** — logic analyzer (≥4 ch, I²C decode) or scope with I²C trigger | THE primary instrument. See START/STOP/ACK/NACK, byte values, SCL period, rise times. A cheap 8‑ch LA + PulseView/sigrok is enough. | on P1.2/P1.3 + GND |
| **`stem-probe`** (RP2040 bench host) | **Known-good reference master** (and bus inspector). `scan`, `read <addr> <reg>`, `leaf`. The SoM/M7 stand-in. | [debug/stem-probe](../debug/stem-probe), [docs/RP2040-BENCH-HOST.md](RP2040-BENCH-HOST.md) |
| **`debug_leaf`** (prod `--debug`) | On-chip readout: idle SDA/SCL levels, `UCB0CTLW0` (UCMST), `UCB0IFG` (UCNACKIFG). | [prod/src/post.rs:84](../prod/src/post.rs) |
| **diag BRW ladder** | Hardware pattern generator: SCL at 10/25/50/100/200/333/500 kHz. | [diag/src/stress.rs](../diag/src/stress.rs) `LADDER` |
| **`thepia hwd`** | Flash/console/reset the FR2355 (`fr2355` alias, JTAG id 0x01ff) and the RP2040. **Never kill hwd.** | [hwd.toml](../hwd.toml) |

**Exit criterion for Phase 0:** you can capture a decoded trace of a transaction and you have the
RP2040 probe flashed and talking on its own console.

---

## 1. Confirm the DUT and that the physical pins can be driven *at all* (no eUSCI)

**Hypothesis to kill:** wrong chip / dead pin / LOCKLPM5 / solder / board damage — none of which are
I²C bugs.

1. Confirm the part really is an FR2355: `hwd` → JTAG device-id **0x01ff** (alias `fr2355`,
   serial `8E57A36E1E000B00`). A wrong-chip assumption wastes days.
2. Flash a **GPIO-only** test (or a diag sub-mode): set P1.2/P1.3 as plain outputs
   (`P1SEL0/1=00`, `P1DIR |= BIT2|BIT3`), clear LOCKLPM5, toggle them ~1 kHz.
3. Scope both pins.

- **PASS:** both pins swing 0↔3.3 V cleanly → the silicon, LOCKLPM5, solder and traces are good.
  Proceed. This isolates *everything physical* from the eUSCI FSM.
- **FAIL (a pin won't move):** stop. It's LOCKLPM5 not cleared, a broken/short pin (a prior
  GND-less slave can put ~7 V on the bus and damage pins — known failure), wrong package pin, or a
  cold joint. Fix before anything else.

---

## 2. Electrical / bus layer

**Hypothesis to kill:** the bus cannot idle high or cannot move within the 100 kHz rise budget.

1. Power the bus, no transactions. Measure idle: **SDA and SCL must sit at ~3.3 V.**
2. On the scope, during a transaction, check SCL **rise time < 1000 ns** (100 kHz / standard).
3. Audit pull-ups: 5.6 kΩ to the **same 3.3 V rail** the FR2355 runs on; **no stacked breakout
   pull-ups** (Qwiic/Grove boards often carry 2.2–4.7 kΩ each — parallel them and total R drops).
4. Verify SDA/SCL are **not swapped** (P1.2=SDA, P1.3=SCL) end-to-end.
5. Confirm nothing drives `OA0±/Veref−` onto P1.2/P1.3; confirm common GND between all boards.

- **PASS:** idle ~3.3 V, edges in spec. Bus is sound.
- **FAIL (idle not high / edges too slow):** this is the whole bug — fix the bus, re-test. `debug_leaf`'s
  idle-level dump is the on-chip version of this check.

---

## 3. Clock sanity

**Hypothesis to kill:** master issues no clock because SMCLK isn't actually running → `UCTXSTT`
never clears, code hangs.

1. Route SMCLK to its pin (P3.4) and scope it, or read `CSCTL7` FLLUNLOCK bits via diag bootrec
   ([diag/src/bootrec.rs](../diag/src/bootrec.rs) `cs_ctl7`).
2. Confirm **SMCLK = 1 MHz, FLL locked**.

- **PASS:** SMCLK is 1 MHz and locked → BRW=10 ⇒ 100 kHz is achievable.
- **FAIL:** fix clock init before touching I²C; a dead/unlocked SMCLK mimics every I²C symptom.

---

## 4. Signature-pattern library — drive FR2355 **hardware functionality** as diagnostic stimuli

**This is the core new idea:** instead of one ambiguous "send a byte", emit a battery of
**deterministic patterns**, each with a *distinct decoded signature*. On the trace, the first
pattern that deviates from its expected signature pinpoints the broken FSM stage.

> **Implemented** — [diag/src/signature.rs](../diag/src/signature.rs), selected by the `g` key (or
> the S1 menu cycle) on the diag console. Build & run:
> `just build fr2355 --diag`, flash, open the console, press `g`. It sweeps for a device, uses the
> first one found as the P3–P8 target (else PCA9698 @0x20), runs P1→P8 once, prints a signature line
> per pattern, and returns to POST. Capture SCL/SDA on a logic analyzer alongside.

| # | Pattern (FR2355 as master, using eUSCI_B0 HW) | Expected decoded signature | A deviation localizes |
|---|---|---|---|
| P1 | **Bus-idle hold** (init only, no txn) | SDA/SCL flat at 3.3 V | pull-ups / mux (→ Phase 2) |
| P2 | **Address sweep 0x08–0x77** (START, addr+W, sample ACK, STOP) = i2cdetect | 112× {START · addr · ACK\|NACK · STOP} | master START/STOP engine + per-device ACK |
| P3 | **Single-byte WRITE** to a known slave | START · addr+W · **ACK** · data · ACK · STOP | TXBUF→TXIFG pacing, ACK handling |
| P4 | **Single-byte READ** | START · addr+R · ACK · data · **master-NACK** · STOP | RX path, master-NACK-before-STOP |
| P5 | **Repeated-START** (write reg ptr, Sr, read) — the real sensor shape | START · addr+W · ACK · reg · ACK · **Sr** · addr+R · ACK · data · NACK · STOP | the "phantom TXIFG" repeated-start bug |
| P6 | **Multi-byte burst** (N bytes, walking-ones `0x01,02,04,…`) | one START, N data bytes each ACKed, one STOP | TXIFG pacing / underrun under back-to-back |
| P7 | **Byte-value eye** — transmit `0xAA` then `0x55` repeatedly | unmistakable alternating SDA eye on scope; confirms **MSB-first** bit order | shift-register / bit-order / noise-vs-signal |
| P8 | **BRW frequency ladder** (10→25→50→100 kHz, [stress.rs](../diag/src/stress.rs) `LADDER`) | SCL period steps 100µs→10µs; each step a clean square | clock-source/divider math; the speed where it breaks |

**How to use it:** run P1→P8 in order against a known-good slave (Phase 5 picks the partner). The
*first* pattern whose signature is wrong is your fault. Example readings:
- P1 fails → bus (Phase 2).
- P2 all-NACK but STARTs present → master FSM works, addressing/slave wrong (not the bus).
- P2 shows **no START at all** (SCL never toggles) → clock (Phase 3) or mux/`UCTXSTT`-hang.
- P3 ACK but P5 drops the repeated-START byte → the phantom-TXIFG FSM bug, firmware fix.
- P8 clean at 10–50 kHz but corrupt at 100 kHz → marginal pull-ups/capacitance, not firmware.

> Patterns double as the regression suite (Phase 9): each becomes a HIL assertion in TESTING.md.

---

## 5. Bisect MASTER vs SLAVE against the known-good RP2040

**This is the step that ends the stall.** Use `stem-probe` as the known-good partner so each FR2355
role is tested against *proven* hardware — never FR2355-against-FR2355 until each half passes alone.

- **Test A — FR2355 = master → RP2040 = slave.** Run patterns P2–P6. Does the RP2040 slave ACK its
  address and return the expected bytes on the trace?
- **Test B — RP2040 = master (`scan` / `read`) → FR2355 = slave** (UCB0 own-address via
  `UCB0I2COA0 | UCOAEN`, see [prod/src/stem.rs:126](../prod/src/stem.rs)). Does `stem-probe scan` list
  the FR2355's address and `read` return its regmap?

| Test A (FR2355 master) | Test B (FR2355 slave) | Conclusion |
|---|---|---|
| ✅ | ❌ | **Slave** FSM/config bug: own-address/UCOAEN, UCTXIFG0/UCRXIFG0 handling, or clock-stretch timing ([the "takes some time" slave-latency class](https://e2e.ti.com/support/microcontrollers/msp-low-power-microcontrollers-group/msp430/f/msp-low-power-microcontroller-forum/1065644/)). → Phase 6b. |
| ❌ | ✅ | **Master** FSM/config bug: `UCTR` not set before `UCTXSTT`, TXBUF write-timing vs `UCTXSTT` clear, SA, or NACK handling. → Phase 6a. |
| ❌ | ❌ | Still **electrical/mux/clock** (re-run 2–3) or **silicon errata** (Phase 7). Not a role-specific bug. |
| ✅ | ✅ | Both halves are good — the problem is the **FR2355↔FR2355 combo** or the original sensor. → Phase 6c. |

**Keep the wiring identical between A and B** — only the firmware role changes. This is what makes
the truth table valid.

---

## 6. Firmware FSM specifics (only enter the branch Phase 5 selected)

**6a — Master (if Test A failed).** Verify, against [crates/bsp/src/i2c.rs](../crates/bsp/src/i2c.rs)
and SLAU445 §24:
- `UCSWRST=1` during *all* config; released last.
- `UCTR` set for a write, cleared for a read, **before** `UCTXSTT`.
- `UCB0I2CSA` holds the 7-bit address (right-justified, no R/W bit).
- Write TXBUF (or set `UCTXSTP`) **before** the eUSCI finishes the address ACK and clears `UCTXSTT`.
- NACK (`UCNACKIFG`) path issues a STOP and doesn't leave a phantom `UCTXIFG`.
- Confirm `recover()` doesn't leave pins mid-bit-bang when there are no pull-ups (benign now with
  5.6 kΩ, but verify the hand-back re-selects `P1SEL0`).

**6b — Slave (if Test B failed).** Verify against [prod/src/stem.rs](../prod/src/stem.rs):
- `UCB0I2COA0 = addr | UCOAEN` (own-address enable). For UCB0-as-slave on P1.2/P1.3, the PAC field
  split differs from UCB1 — confirm the right IFG accessor is used.
- `UCTR`/`UCTXIFG0`/`UCRXIFG0` serviced correctly and fast (short ISR).
- Uninitialized `UCTXIFG=1` is handled, not mistaken for a real event.
- Clock-stretch: the slave holding SCL low while preparing data is expected; confirm the master
  side tolerates it (and that you're not hanging forever absent `UCCLTO`).

**6c — FR2355↔FR2355 (if both passed alone).** Now connect two UCB0 buses. Watch for a two-master
contention window at boot ([prod/src/mode.rs:34](../prod/src/mode.rs)) and address collision. Only
one master at a time.

---

## 7. Silicon errata

If Phases 2–6 pass individually yet it still fails, pull the **FR2355 device erratasheet** (SLAZ…,
match the rev on the chip markings) and grep for **USCI/eUSCI I²C** items affecting that rev. Use
the `collect-pdf-info` skill. Rare, but a day-long stall with correct code is exactly when to check.

---

## 8. Decision tree (one-screen summary)

```
Pins drive as GPIO? ──no──▶ LOCKLPM5 / damage / solder / wrong pin        (Phase 1)
        │yes
Bus idles ~3.3V, edges <1µs? ──no──▶ pull-ups / rail / swap / SAC         (Phase 2)
        │yes
SMCLK = 1MHz locked? ──no──▶ clock init                                   (Phase 3)
        │yes
Run signature patterns P1–P8 vs known-good RP2040:                        (Phase 4+5)
   first wrong signature ▶ that FSM stage
   A✅/B❌ ▶ slave bug (6b)   A❌/B✅ ▶ master bug (6a)
   A❌/B❌ ▶ back to 2–3 or errata (7)   A✅/B✅ ▶ combo/sensor (6c)
```

---

## 9. Capture & regression (close the machinery gap)

When the root cause is found:
1. Record it here (root cause + the pattern that exposed it + the fix).
2. Promote the failing signature pattern (P2–P6) into a HIL assertion in
   [TESTING.md](../TESTING.md) Layer 2 so it can never silently regress.
3. If `debug_leaf` couldn't have localized it, extend `debug_leaf` so it could.
4. Update project memory if the lesson is cross-session (e.g. a pin/errata gotcha).

## Sources
- MSP430FR2355 datasheet SLASEC4 — Table 6-41 (P1 pin functions), I²C switching characteristics.
- SLAU445 — MSP430FR2xx/4xx Family User's Guide, eUSCI_B I²C (§24).
- SLAA734 — Solutions to Common eUSCI/USCI Serial Communication Issues.
- Repo: [crates/bsp/src/i2c.rs](../crates/bsp/src/i2c.rs), [prod/src/stem.rs](../prod/src/stem.rs),
  [prod/src/post.rs](../prod/src/post.rs), [diag/src/stress.rs](../diag/src/stress.rs),
  [docs/RP2040-BENCH-HOST.md](RP2040-BENCH-HOST.md), [hwd.toml](../hwd.toml).
</content>
</invoke>
