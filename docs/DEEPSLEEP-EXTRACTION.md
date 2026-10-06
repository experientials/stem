# Plan · extracting `deepsleep` — the MSP430 deepsleep-supervisor module crate

**Derived from** [ADR-0014](adr/0014-board-layer-structure.md) (crate boundary), [ADR-0013](adr/0013-async-runtime-per-core-tier.md)
(async `sched` + LPM waker), [ADR-0012](adr/0012-stembus-shared-contract-crate.md) (`stembus`), and
STEM-DIRECTION.md ("prod is a layer diag builds on"; "same-boot + master-commanded mode"; "commands over
I²C and UART"; "one image per product module"). ADRs are canonical; if this plan disagrees, the ADR wins.

**Goal.** Lift the MSP430 deepsleep-supervisor capabilities out of the prod binary into a **`deepsleep`**
crate that **both** prod (API/event-loop harness) and diag (test harness) build on — so diag can host and
exercise the real capability, and the same-boot/master-commanded behaviour falls out. Emptying prod is
**not** the goal; prod-bin stays a thin harness.

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

- **Phase 0 — create `deepsleep`; move the no-new-abstraction pieces.** *(low risk, like the model move)*
  Move `regmap`/`post`/`status`/`mode`/`enumerate`; inject build-identity (§B); feature-gate
  `mode`/`enumerate` off fr2433; route `mode`'s sensor-bus acquire/release via a `bsp::board` role.
  `stem.rs` stays in prod-bin (still fused). **Verify:** prod fr2476/fr2355/fr2433 + diag build in budget;
  HW smoke on one dual + fr2433.
- **Phase 1 — split the Stem slave (§A) + go interrupt-driven.** *(the hard one)* Driver+ISR+waker →
  `bsp/src/stem_i2c.rs`; dispatch → `deepsleep` async task over `SlaveEvent`; adopt the `sched` async
  executor + LPM waker (ADR-0013). **Verify:** master read/write over Stem; CPU sleeps between
  transactions and wakes on the slave ISR.
- **Phase 2 — command surface.** One `Command` enum + `execute`; Stem-regmap face + UART face. Keep the
  PCA9698 region layout reserved (§2).
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
Prereqs **done**: `bsp::model`, CONV-1 (clock/uart/fmt → bsp), `stembus`, ADR-0014. **Next actionable:**
Phase 0 (un-gated). **Phase 1** needs the `sched` async executor + LPM waker (ADR-0013 follow-up) — now
clearly the `deepsleep` foundation, not an optional nicety.
