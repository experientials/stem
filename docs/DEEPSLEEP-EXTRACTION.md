# Plan · extracting `deepsleep` — the MSP430 deepsleep-supervisor module crate

**Derived from** [ADR-0014](adr/0014-board-layer-structure.md) (crate boundary), [ADR-0013](adr/0013-async-runtime-per-core-tier.md)
(async `sched` + LPM waker), [ADR-0012](adr/0012-stembus-shared-contract-crate.md) (`stembus`), and
STEM-DIRECTION.md ("prod is a layer diag builds on"; "same-boot + master-commanded mode"; "commands over
I²C and UART"; "one image per product module"). ADRs are canonical; if this plan disagrees, the ADR wins.

**Goal.** Lift the MSP430 deepsleep-supervisor capabilities out of the prod binary into a **`deepsleep`**
crate that **both** prod (API/event-loop harness) and diag (test harness) build on — so diag can host and
exercise the real capability, and the same-boot/master-commanded behaviour falls out. Emptying prod is
**not** the goal; prod-bin stays a thin harness.

## Behaviour model — CANONICAL (verbatim, Henrik 2026-10-06)

> The deepsleep module will be active or passive. When active it masters the Leaf bus, when passive it is
> passive on both I2C busses, and mainly just reacts to interrupts from I2C contacting it and
> analog/trigger sensors asking it to reflect a change

> (refinement, Henrik 2026-10-06) MSP in passive mode is reachable on both Leaf and Stem I2C.

Derivation (drives §3): **Active** = masters the Leaf (sensor) I²C bus (today's Sensing — enumerate /
monitor), Stem = slave. **Passive** = the MSP is an I²C **SLAVE on BOTH buses** (Stem=UCB1 slave AND
Leaf=UCB0 slave on dual parts) — reachable on either — and never *masters* Leaf (so the one-master-on-Leaf
invariant holds: a slave doesn't contend). It SLEEPS and is **reactive**; wake sources are (a) the **Stem
slave** ISR, (b) on dual the **Leaf slave** ISR, and (c) **analog/trigger sensor** interrupts (eCOMP /
GPIO edge) "asking it to reflect a change" → it updates the reflected state (PCA9698-style
InputPort/IntMask banks, §2) and may notify the SoM (`STEM_INT`). So LPM + interrupt-wake is the
**Passive** posture; Active is the busy Leaf-master loop. This is the authority if the phased text drifts.
>
> **⚠️ Supersedes DESIGN.md L76** ("Passive → it releases / never drives the sensor-bus pins"): a slave
> DOES drive SDA to ACK, so Passive is **UCB0-as-slave**, not the Phase-0 tri-state release
> (`bsp::board::leaf_i2c_release` → GPIO high-Z). That release makes the node UNreachable on Leaf in
> Passive — it must become "bring UCB0 up as a slave" instead. **Scope:** this is a DUAL-part change
> (two eUSCI_B slaves in Passive + the UCB0 `EUSCI_B0` ISR as a 2nd wake source); **fr2433 is unaffected**
> (one bus, already its slave bus, Passive-only) → build/prove 1b on fr2433 first, then reshape dual.

### Priority — Leaf is NICE-TO-HAVE (Henrik 2026-10-06)
> The second bus is Leaf, and not super important. Just nice to have, so we can implement it.

So the **critical path is the shared Stem command-slave in the Passive deepsleep posture — identical
across FR2433/FR2476/FR2355** (same `stembus` regmap + `deepsleep::stem` dispatch; proven on `0x21`
FR2476 + `0x22` FR2433 via the probe). Everything **Leaf** — Active/Sensing, UCB0-as-master, the
enumerate/sensor scan, the mode machine, AND the "reachable on both" UCB0-as-slave-in-Passive — is a
**nice-to-have**, implement when convenient. Re-scoping the remaining dual work accordingly:
- **Important (dual) — ✅ DONE + HW-proven 2026-10-06.** Extended the Stem-slave LPM to the dual's
  **UCB1**: new `EUSCI_B1` wake ISR (same hand-rolled `br` trampoline; inner masks UCB1 IE), and the
  silent dual serve loop sleeps in LPM0 when Passive (disable GIE → `irq_enable` → `WDTHOLD` →
  `bis #0x18,r2`). Used the **1b-style hand loop** (not 1c's `run_pended`) — the dual loop still carries
  the Leaf scanner/mode/console, so a full async convergence is deferred with the nice-to-have Leaf
  reshape; the sleep is the proven pattern inserted at the loop's idle point. Build: `rt` (device vector
  table) moved to a single **`bsp/rt`** feature (`dep?/rt`) that PROD enables and DIAG does not (fixes a
  diag regression); `memory-fr2476.x` VECTORS `0xFF80/0x80`, fr2355/fr2155 `0xFFA4/0x5C`. HW on `@fr2476`
  (silent) + stem-probe: `scan 0x21`, 2× `r 0x21` bit-stable (`0xD0`/`0x832A`/`0.1.0`) → wakes from LPM on
  the `EUSCI_B1` slave ISR. Console/debug builds stay no-sleep (ISR installed, GIE off → unchanged).
- **Nice-to-have (dual):** the Leaf second bus + Sensing + UCB0-as-slave-in-Passive + its 2nd ISR, AND
  the async convergence of the dual loop onto `run_pended` (where the multi-task pender earns its keep).

---

## 1. Crate shape — one crate PER PRODUCT MODULE, shared layers beneath

Each product module is its **own crate** — `deepsleep` (MSP430), later `lowvision`, etc (their
own MCUs). Not one crate: command sets differ *and* the MCUs differ (PAC/HAL/transport). 
Named for the module, not the build axis (reject `node`/`prod-core`: too generic / names prod-vs-diag).

**`deepsleep`** = MSP430 deepsleep-supervisor: MCU state (status/identity, mode, FRAM integrity) + the
semantic command/state model + command dispatch + the Stem-slave dispatch. Both prod-bin and diag-bin
depend on it.

**Shared layers below it** (NOT only `stembus`):
- **`stembus`** — wire vocabulary (regmap addresses, command codes, role/crc). Shared across **all**
  modules *and* the masters.
- **`bsp`** — MSP430 hw (board/model/hal/fmt/i2c/stem_i2c). Shared across **MSP430** modules.
- **`sched`** — the `core`-only async executor + LPM waker (ADR-0013). Shared across the **MSP430 tier**.
- **`devices`** — sensor drivers. Shared across **sensor-driving** modules.

**`deepsleep` is module-specific ONLY — not a dumping ground.** Each piece defaults to the lowest layer
that can hold it. Test: *if lowvision/bluetooth would need it, it belongs below `deepsleep`; if something
in `deepsleep` turns out reusable, move it DOWN — don't copy up.*

---

## 2. Interface model — semantic core (primary); PCA9698 a reserved, optional adapter

- **CORE = a semantic command/state model** — mode, sensing, presence, sleep, status — consumed by the
  **M7 (Zephyr)** via custom `stembus` client code. Standalone; owes nothing to PCA9698.
- **Command dispatch** = one internal `Command` enum + `execute`, fed by **two faces**: the Stem-I²C
  slave (regmap writes) and the console-UART (the s/r/d/m/l keys). **Implemented in `deepsleep`**;
  `stembus` carries only the *encoding*. The `Command` enum is MCU-internal (masters just write regmap).
- **PCA9698 = an optional Linux-userland adapter, and a layout CONSTRAINT.** Its only purpose is to let
  Linux *userland* on the i.MX8 manipulate the board via the stock `gpio-pca953x` driver + `gpiod` — **no
  custom driver**. It **may never ship** (if the M7 does all consumption). So it is **not load-bearing** —
  but we **reserve its register regions** so it stays implementable:
  - `stembus` keeps the PCA9698 **protocol shape** (bank addressing + command-pointer + auto-increment)
    and reserves the PCA9698 regions (IP/OP/IOC/polarity/mask, 0x00–0x1F).
  - our semantic command/state registers live in the regions PCA9698 leaves free (the 0x30–0x3F window) —
    so the two **coexist without collision** (already the current layout).
  - Build the semantic interface first; the PCA9698 GPIO adapter is a **drop-in later** (Phase 4), never
    foreclosed.

### 2a. Command system → see docs/COMMANDS.md
The cross-MCU command design — one `postcard` `Command` vocabulary over **I²C-first-then-UART**, the
common API across all MCU modules, firmware patches, PCA9698 coexistence (the reserved `0x37`/`0x38`
stream port), the SoM two-bus model, and Henrik's verbatim statements — **moved to
[COMMANDS.md](COMMANDS.md)** (it's cross-MCU, not deepsleep-specific). Transport/regmap + role addressing
stay in [I2C-API.md](../I2C-API.md).

**deepsleep's part (Phase 2 below):** implement that common command API on the proven
`bsp::stem_i2c::next_event()` seam — accumulate `Rx` bytes after the reserved CMD port, `postcard`-decode
on `Stop`, dispatch via `deepsleep`'s `execute`.

---

## 3. Execution model — interrupt-driven + LPM, NOT poll

The node is a *deep-sleep* supervisor: it sits in LPM and **wakes on interrupts**. Polling is accepted
**only** where a source genuinely cannot interrupt — none of ours qualify. Verified capability:

| wake source | from LPM? | grounding (verified) |
|---|---|---|
| **Stem I²C slave** | LPM0–4 | `slau445i` §24.3.10 (verbatim): "operate the eUSCI_B in I2C slave mode while the device is in LPM4 and all internal clock sources are disabled. The receive or transmit interrupts can wake up the CPU from any low-power mode." Clock-stretch (§24.3) covers wake latency. **One shared eUSCI_B I²C interrupt vector** (§24.3.11) — the ISR dispatches TX/RX/state by flag |
| **GPIO** | any LPM (incl LPM4) | port edge interrupts — asynchronous |
| **Analog** | LPM via **eCOMP** | device DS: FR2476 ×1 eCOMP, FR2355 ×2 eCOMP (+ ADC). Threshold-wake = comparator; bare ADC needs a clock (not idle-wake) |
| **Watchdog** | LPM3 (interval mode) | `WDTTMSEL=1` → periodic `WDTIFG`; **mutually exclusive** with the reset-backstop (one timer) — prefer a Timer/RTC tick, keep WDT as backstop |

**Limit:** this is LPM0–3. **LPMx.5** powers down the eUSCI + locks I/O → the Stem slave is *not*
reachable there; only a wake pin/RTC revives it. "Stem-reachable sleep" = LPM0–3; deepest sleep is a
separate GPIO/RTC-woken tier.

**Software shape = the async `sched` executor + an ISR-fired waker** (ADR-0013's unbuilt "real waker"):
a task `.await`s an event (Stem request / GPIO edge / timer); the **ISR fires the waker** (sets a flag,
does no work); the executor **drops to LPM when all tasks are Pending**; the next interrupt wakes it.
`poll()` is the **degenerate, never-sleeps** version of this. The async-keyword task style *is* the
deepsleep model — this is why task-handling is vital, and it ties directly back to the async-`sched`
evaluation (ADR-0013). **No separate ADR:** this is ADR-0013's own "real waker / LPM pender" (its §"The
valuable hard part is LPM sleep" + follow-up #5) — ADR-0013 holds the *decision*, this plan is the
*application* to `deepsleep`.

---

## 4. Per-module assignment (grounded in the current import graph)

| prod module | lines | destination | notes |
|---|---|---|---|
| `regmap.rs` | 78 | **deepsleep** | `ShadowState` is MCU state; regmap *addresses* stay in `stembus` |
| `post.rs` | 105 | **deepsleep** | build-identity injected (§B) |
| `status.rs` | 175 | **deepsleep** | build-identity injected (§B) |
| `mode.rs` | 182 | **deepsleep** | dual-only (gate off fr2433); sensor-bus acquire/release via a `bsp::board` role, not raw `p.p1` |
| `enumerate.rs` | 371 | **deepsleep** | sensing (thin today, grows); pure bsp/devices/sched already |
| `stem.rs` | 475 | **SPLIT** | driver+ISR → `bsp/src/stem_i2c.rs`; pins → `board`; dispatch → `deepsleep` (§A) |
| GPIO-bank backing (`gpio::`) | — | **optional PCA9698 adapter** | NOT a core bsp seam — part of the droppable Linux adapter (§2) |
| `main.rs` | ~480 | **stays prod-bin** | thin harness; selects role per family; supplies build-identity; adopts `sched` (ADR-0013) |

**Already done / elsewhere:** `model.rs` → `bsp::model`; `clock`/`uart`/`fmt` → `bsp` (CONV-1); wire
contract → `stembus` (ADR-0012).

---

## 5. The ONE real design point — §A, the Stem-slave seam

`stem.rs`'s `poll()` welds the eUSCI slave **transport** (IFG/rx/tx/clear) to the protocol **dispatch**
(PCA9698 command-pointer + register file). Unpick it:

- **Transport = a driver → `bsp/src/stem_i2c.rs`** (NEW, sibling to the Leaf-master `i2c.rs` — **not**
  `board`; drivers live at the `i2c.rs` layer). eUSCI_B *slave* register programming, **family** cfg-split
  (UCB1 dual / UCB0 mode-split fr2433) — plus **its ISR** (the single shared eUSCI_B I²C vector,
  `slau445i` §24.3.11 — dispatch TX/RX/state by flag), which fires the waker. No `embedded-hal` slave
  trait exists, so this file exposes the invented seam.
- **Pin routing → `board/<chip>_<module>`** `route_stem_i2c_pins()` — **module** cfg-split (P3.2/3.6
  FR2476, P4.6/4.7 FR2355, P1.2/3 FR2433), like `route_console_uart_pins`. (The code already has this
  seam: `hw::route_pins` is the only chip-split; `init`+accessors are family-shared.)
- **Dispatch → `deepsleep`** — an **async task** over the seam's events + the semantic register/command
  layer. bsp ends up symmetric: `i2c.rs` (Leaf master) ↔ `stem_i2c.rs` (Stem slave); `board` routes pins
  for both.

**Seam shape — EVENT-based (decided by §3, no longer just "recommended").** Because the node is
interrupt-driven + LPM via async, events come from the **ISR** and are consumed by an **async task** — so
`stem_i2c` emits `enum SlaveEvent { Start, Stop, Rx(u8), TxReq, Nack }` and the dispatch awaits them. A
raw-accessor/poll shape (the old option a) can't express ISR→waker→async, so it's out. `poll()` survives
only as the degenerate fallback feed.

**Refinement adopted in 1a (reconciling §3 with §A).** A naive reading of §A ("async task consumes
per-**byte** events") would put a byte-level SPSC queue in the ISR — but that contradicts §3's "the ISR
fires the waker (**sets a flag, does no work**)" *and* forces sharing the `RegFile` into the ISR
(critical sections + footprint). The correct reading that satisfies both: the **ISR only masks its own
eUSCI_B source + wakes** (no byte work, no `RegFile`); the transport's **`next_event()` decodes the IFG
into `SlaveEvent`s** when the woken dispatch services it, with eUSCI auto-clock-stretch absorbing the
wake→service latency (`slau445i` §24.3). `RegFile` stays entirely in the main context — only the eUSCI
IE register and a wake flag cross the ISR boundary. So `SlaveEvent` lives at the `next_event()` seam, NOT
in the ISR; this is what 1a built (polled), and 1b adds the ISR on top unchanged in servicing — only
*when* `next_event()` runs changes.

**Not decisions:** clock-stretch is automatic (hw) — only servicing latency matters; and the transport
must let **diag bring the slave up identically** (both call `bsp::stem_i2c` + `deepsleep` dispatch).

---

## 6. The forced mechanic — §B, build identity (NOT a decision)

Cargo's crate-scoping dictates one answer:
- **Build stamp** (`FW_BUILD = env!("PROD_BUILD")`) is emitted by the **bin's** `build.rs` via
  `cargo:rustc-env`, which is **crate-scoped** — `deepsleep` can't `env!` it. → **inject** it as a
  `&'static str` into `Status::booted(model, build)` / `post::boot_log(.., build)`.
- **Semver** (`CARGO_PKG_VERSION_*`) is set per-crate by cargo → inside `deepsleep` it *is* deepsleep's
  version. No injection.
- Net: stamp = which image is flashed (bin-owned), version = which capability runs (crate-owned); diag &
  prod report the same version, different stamps. `model_name` already delegates to `stembus::role`.

---

## 7. Phased execution (each phase builds + is independently landable)

- **Phase 0 — ✅ DONE 2026-10-06** (`bb3ac85`/`5109ab5`/`b1fc603`/`2c0a584`). Created `crates/deepsleep`;
  moved `regmap`/`enumerate`/`status`/`post`/`mode` (via `git mv`, imports remapped to
  `bsp::{model,fmt,pac,i2c,hal}`+`stembus`); build-identity injected (§B — stamp as `&str`, version =
  `deepsleep`'s `CARGO_PKG_VERSION`; banner `== prod v`→`== v`); `mode`/`enumerate` `_dual`-gated; `mode`
  uses the new **`bsp::board::leaf_i2c_{acquire,release}`** role (+`bsp::i2c::release`), no raw `p.p1`.
  `stem.rs` still fused in prod. Verified: prod fr2476/fr2355/fr2433 + `--debug` + diag build in budget;
  HW on `@fr2476` — boot/status/enumerate + build-identity correct; mode role proven by Passive
  (`MST=0 SWRST=1`) → Sensing (`MST=1 SWRST=0`, OLED `0x3C` found).
- **Phase 1 — split the Stem slave (§A) + go interrupt-driven.** *(the hard one)* Done in verifiable
  sub-steps:
  - **1a — ✅ DONE 2026-10-06** (`a1633a7`). Pure relocation into the event seam, NO behavior change,
    still polled. Transport → NEW `bsp/src/stem_i2c.rs` (`hw` instance seam copied verbatim + the
    `SlaveEvent`/`next_event`/`provide_tx` seam); pins → `board::route_stem_i2c_pins` (per-module, keyed
    on the chip feature — dropped the prod-only `_stem_p3`/`_stem_p4`); dispatch → `deepsleep::stem`
    (`git mv`, `poll()` now drains the seam, identical IFG sequence). bsp symmetric: `i2c` (Leaf master)
    ↔ `stem_i2c` (Stem slave). Builds in budget prod fr2476/fr2355/fr2433 (+`--debug`) + diag; zero
    warnings from moved/new code. HW on `@fr2476`: clean boot + banner + status dump + Passive→Sensing
    (OLED `0x3C`) + serve-loop/loopback paths run without wedging. **MASTER-FACING PROOF — ✅ RETIRED
    2026-10-06** via the RP2040 **stem-probe** on the MCU bench (docs/MCU-BENCH.md — the Pico IS the Stem
    master, no jumpers/SoM needed): `scan` → `0x21` ACKs; `vstem 0x21` + `r 0x21 …` read back
    iface=`0xD0`, model=`0x832A`, build-id=`0x66D9D2CF` (== `@fr2476`'s boot stamp, bit-stable across
    passes → the right board, no collision), ver=`0.1.0`; `0x3E` Passive↔Sensing writes land. So a real
    external master reads **and** writes the regmap through the new `next_event`/`provide_tx` seam.
  - **1b — NEXT: add the ISR + LPM wake.** `init` enables the eUSCI_B slave IE; add the single shared
    eUSCI_B `#[interrupt]` (dual → `EUSCI_B1`; fr2433 → `USCI_B0`, verified in `pac/*/src/lib.rs`) that
    **masks its own IE + sets a wake flag + exits LPM, doing no byte work** (§3 + the 1a refinement);
    the serve loop enables GIE and drops to LPM when idle; the woken `poll()`/dispatch services via
    `next_event()` and re-enables IE.
    - **Design point 1 — WDT backstop vs LPM (safety-relevant; GROUNDED in slau445i §12, 2026-10-06).**
      A CPU asleep in LPM doesn't run the serve loop, so it can't `wdt_pet()`. **Checked assumption that
      was WRONG:** "deep LPM stops the WDT clock so it won't false-reset." slau445i §12 refutes it — in
      **reset (watchdog) mode the WDT_A has a fail-safe**: *"If SMCLK or ACLK fails as the WDT_A clock
      source, VLOCLK is automatically selected"* (p≈362) — so a clock-sourced reset-mode WDT keeps
      counting in LPM and **will reset the sleeper**; and §1 (p≈40): in watchdog mode *"the device does
      not enter LPMx.5."* TI's prescribed low-power path is `WDTHOLD`: *"When the watchdog timer is not
      required, the WDTHOLD bit can be used to hold the WDTCNT, reducing power consumption."* **Decided
      (also the lower-power choice):** keep the reset backstop **armed only while awake/processing** (that
      is where a hang can occur — the CPU is running code); **`WDTHOLD` it in the idle→LPM transition**
      and re-arm+pet first thing on wake. A sleeping CPU is not a hung CPU (it executes nothing), so it
      needs no backstop; a genuine hang happens during awake processing, which stays guarded. This both
      satisfies Koopman ("tie the kick to real work, never a dumb timer ISR" — `docs/references/firmware-architecture.md`
      §2/§4) and is what TI recommends for LPM. (Rejected: the earlier "periodic TB0 wake pets the WDT"
      — the firmware-architecture research flags unconditional-pet-from-a-timer-ISR as the anti-pattern;
      and WDT **interval** mode as heartbeat trades away the reset backstop per §3.)
    - **Design point 2 — console vs sleep.** The console/`debug` build polls the UART menu, so it can't
      deep-sleep identically. **Recommended:** console builds stay in the degenerate no-sleep loop; LPM
      engages in the silent production image (you don't deep-sleep a board you're interactively poking).
      This is fine for verification — the stem-probe reads over I²C, not the console, so LPM-wake is
      proven on the silent build via the probe.
    - **Implementation readiness (de-risked 2026-10-06), fr2433-first:**
      - **LPM-wake-from-ISR mechanism CONFIRMED in-toolchain:** `#[interrupt(wake_cpu)]` (msp430-rt
        0.4.1) clears SCG1/SCG0/OSC_OFF/CPU_OFF in the **stacked** SR via a naked trampoline
        (`bic.b #mask, 0(r1)`) — the correct way (writing the live SR doesn't survive RETI). No fragile
        hand asm. LPM *entry* is inline asm on SR/r2 (as `clock.rs` already does): LPM0 = `bis #0x18, r2`
        (CPUOFF|GIE) — start at LPM0 (safe; keeps clocks), tune to LPM3/4 (slau445i §24.3.10: slave wakes
        from LPM4) once proven.
      - **IE register = same bit layout as IFG** (RX b0, TX b1, STT b2, STP b3, NACK b5) — verified in
        `pac/msp430fr2433` (`ucb0ie_i2c`) → reuse the existing `UC*IFG` masks for IE. `init` sets the
        slave IE; the ISR MASKS it (so the latched IFG doesn't re-fire pre-service) + sets a wake flag;
        main re-enables after `poll()` drains.
      - **WDT:** arm backstop only while awake; `WDTHOLD` in the idle→LPM step; re-arm on wake (Design
        point 1).
      - **Integration dry-run (2026-10-06) — SOLVED up to one upstream blocker.** Probed the full wiring
        on fr2433; findings (so the next attempt is fast):
        - ISR goes in the **bin** (prod), not `bsp` — this is the first ISR in the codebase (no handlers
          existed; PAC `rt` was off everywhere). The PAC does NOT re-export `#[interrupt]` → use
          `msp430_rt::interrupt` directly, with `use bsp::pac::Interrupt as interrupt;` in scope (the
          macro name-checks `interrupt::USCI_B0`).
        - Feature wiring that works: `bsp` `fr2433 += "msp430fr2433/rt"`; prod `fr24xx += "msp430-rt/device"`;
          crate attr `#![cfg_attr(feature="fr24xx", feature(abi_msp430_interrupt))]`.
        - **memory-fr2433.x VECTORS for rt-mode = `ORIGIN=0xFF88, LENGTH=0x78`** (NOT the lint's
          `0xFFDA/0x26`). The PAC emits the FULL padded 59-word `__INTERRUPTS` + reset = 0x78 B; it must
          end at 0x10000 (link.x ASSERT) with reset at 0xFFFE → 0xFF88 is forced (real vectors still land
          correctly: PORT2→0xFFDA … reset→0xFFFE). **`scripts/lint-memory-x.sh` is buggy here** — it uses
          the lowest *real* vector and under-counts the reserved padding → its origin overflows. Fix the
          lint to count the full PAC array. (This file is generator-emitted; update `gen-memory-x.sh` too.)
        - `wake_cpu`'s naked trampoline + `naked_asm!` **compile fine** on nightly-2025-06-25.
        - **`wake_cpu` macro blocker → RESOLVED with a hand-rolled `br` trampoline.** The msp430-rt 0.4.1
          `wake_cpu` emits a short `jmp {inner}` (±1 KB) that truncates under our fat LTO; it's unfixed
          upstream/in every fork (checked). Instead of patching the dependency we write our OWN naked
          trampoline using `br` (absolute, full-range) in `prod` — self-contained, old-Rust-safe. Full
          pattern + investigation: **`docs/references/msp430-lpm-wake-pattern.md`**. (Optional follow-up:
          upstream a `jmp`→`br` PR to `rust-embedded/msp430-rt`.)

  - **1b (fr2433) — ✅ DONE + fully HW-proven 2026-10-06.** Implemented the interrupt-driven LPM path on
    fr2433: `bsp::stem_i2c::irq_{enable,disable}` (slave IE), the hand-rolled `br` wake trampoline +
    `usci_b0_inner` (masks IE, no byte work), and the `run_fr24xx` serve loop (service → disable GIE →
    irq_enable → `WDTHOLD` → atomic `bis #0x18,r2` LPM0+GIE). rt/device wiring + `memory-fr2433.x` VECTORS
    `0xFF88/0x78`. Builds in budget (4332/15360); fr2476/fr2355/diag green (no regression). **HW on
    `@fr2433` + the RP2040 stem-probe:** boots → full init (banner/status/gpio PASS) → enters LPM, NO
    ~16 s reset-loop (`WDTHOLD`-across-LPM proven); then the Pico master `scan`→`0x21 0x22` and 20× `r 0x22`
    reads each WOKE the sleeper via the eUSCI_B0 slave ISR, serviced, and returned bit-stable correct
    identity (iface `0xD0`, model `0x8240`, build `0xFDA60711` == boot stamp, ver `0.1.0`). So sleep →
    wake-on-slave-ISR → service → resleep is proven end-to-end. Pattern + investigation:
    `docs/references/msp430-lpm-wake-pattern.md`.
    - **Verify (via the stem-probe, now the proven path):** flash the silent fr247x image; `scan`/
      `vstem 0x21`/`r 0x21 …` return correct, stable data → the CPU woke from LPM on the slave ISR,
      serviced, and reslept. (Remote limit: can't meter current to *see* LPM; functional wake + code
      path is the evidence — stated plainly.)
  - **1c (fr2433) — ✅ DONE + HW-proven 2026-10-06.** The fr2433 dispatch is now the async `stem::serve`
    task (loops `next_event().await` → `Slave::handle`, shared with the sync `poll` so they can't drift),
    driven by the NEW **`sched::{Pender, run_pended}`** — ADR-0013's "real waker / LPM pender": the future
    returns `Pending` when the bus is idle, `run_pended` runs the caller's `idle` closure (disable GIE →
    `irq_enable` → `WDTHOLD` → `bis #0x18,r2`), and the eUSCI_B0 ISR wakes the CPU **and** `Pender::wake()`s
    (closing the wake-during-poll race). Pender wake flag is a single-core `volatile` byte (MSP430 has no
    `core::sync::atomic`; keeps `sched` zero-dep). Cost **+94 B** over 1b (4426/15360; the spike's ~0.5 KB
    is for a 3-machine model). fr2476/fr2355/diag green (sched change is additive). HW on `@fr2433`+probe:
    boots → LPM (no reset-loop) → the master wakes the async sleeper, `scan`→`0x21 0x22`, 2× `r 0x22`
    bit-stable (model `0x8240`, build `0x51EC29D9` == boot stamp). Functionally identical to 1b, now in the
    one async model the dual reshape reuses. This satisfies **ADR-0013 follow-up #5** for `deepsleep`.
- **Phase 2 — command surface.** IN PROGRESS. The pivot (COMMANDS.md §0) replaced the "Stem-regmap +
  UART face" plan with **cros_ec v3 framing + postcard payloads**. Landed so far:
  - `stembus::command` — the wire-agnostic vocabulary + the narrowed `Node`(introspection)/`Operate`
    seam + `execute_core`/`execute` (host-tested).
  - `stembus::cros_ec` — the v3 I²C framing codec + `EcSlave` framer + standard commands `HELLO` /
    `GET_PROTOCOL_INFO`; `deepsleep::ec` glue; prod `--features cros_ec`.
  - ✅ **HELLO + GET_PROTOCOL_INFO HW-proven on FR2433 ↔ Pico (2026-10-08)** — the COMMANDS.md §0 first
    milestone. Next: the `Node` adapter (`GET_VERSION`/`GET_BUILD_INFO` = identity) + vendor+postcard for
    our own `Command` data, then retire the regmap/PCA9698 command surface (see the replacement plan).
- **Phase 3 — diag consumes `deepsleep` + same-boot.** diag brings up the slave identically; ~1 s grace
  window (master commands mode before POST auto-runs); converge diag's `diag.rs` scanner onto
  `deepsleep`'s enumerate. **Size-watch:** diag ~72 % of 32 KB today (~9 KB free); `deepsleep` ~5 KB.
- **Phase 4 — OPTIONAL: PCA9698 gpiochip adapter.** Only if a Linux-userland consumer appears; drops into
  the reserved regions. May never happen.
- **Phase 5 — prod-bin slimming.** Drop the triplicated loop; the harness drives `deepsleep` over `sched`.

**Risks:** Phase 1 is the real cost (the ISR/waker/LPM plumbing + the non-embedded-hal slave seam, and
revisiting the no-ISR single-threaded assumption — shared state touched by ISRs needs care); diag size
(measure each move); keep `deepsleep` capabilities individually invocable + feature-gateable so the
FR2433 15 KB image pulls only what it ships.

## Status
Prereqs **done**: `bsp::model`, CONV-1 (clock/uart/fmt → bsp), `stembus`, ADR-0014. **Phase 0 DONE**
(2026-10-06) — `deepsleep` holds regmap/enumerate/status/post/mode. **Phase 1a DONE** (2026-10-06,
`a1633a7`) — Stem-slave transport split into `bsp::stem_i2c` behind the `SlaveEvent` seam, dispatch in
`deepsleep::stem`, still polled (no behavior change); **master-facing proof retired via the stem-probe**
(real Pico master reads+writes `@fr2476`'s regmap over the Stem bus). **Next actionable: Phase 1b** (add
the eUSCI_B ISR + LPM wake — the first behavioral change; servicing path unchanged, only *when* it runs).
1b carries two design points — WDT-backstop-vs-LPM (recommend a TB0 timer wake to keep petting) and
console-vs-sleep (recommend LPM in the silent image only) — both logged under Phase 1b above. HW verify
is unblocked: the stem-probe is the proven master path (no jumpers/SoM needed).
