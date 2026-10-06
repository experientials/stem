# Plan · extracting `deepsleep` — the MSP430 deepsleep-supervisor module crate

**Derived from** [ADR-0014](adr/0014-board-layer-structure.md) (crate boundary) and STEM-DIRECTION.md
("prod is a layer diag builds on"; "same-boot + master-commanded mode"; "commands over I²C and UART").
ADR-0014 is canonical; if this plan disagrees with it, the ADR wins.

**Goal.** Lift prod's capabilities out of the prod binary into shared crate(s) that **both** prod
(event-loop harness) and diag (test harness) depend on — so diag can host and exercise the real prod
code, and the same-boot/master-commanded behaviour falls out. Emptying prod is **not** the goal:
prod-bin stays as the thin API+timer loop.

## One crate PER PRODUCT MODULE — the MSP430 module is `deepsleep` (Henrik 2026-10-06)
Reject `node`/`prod-core`: `node` is too generic and `prod-core` names the prod/diag **build axis**,
not the thing. The thing is a **product module**, and modules run on different MCUs and do different
jobs — a **deepsleep supervisor** (MSP430), a **lowvision** module, a **bluetooth networking** module.

**Decision (Henrik):** each product module is its **own crate** with its own state + command
implementations — they are **NOT** in one crate. Two compounding reasons:
1. **Different command sets** — deepsleep's commands (mode, scan, presence, sleep) share nothing with
   lowvision's (capture, framerate) or bluetooth's (pair, advertise). The commands *are* the module's job.
2. **Different MCUs** — lowvision/bluetooth aren't MSP430 (different PAC/HAL/transport); a crate of
   MSP430 eUSCI/FRAM code can't be shared with them anyway.

**The only crate shared across modules is `stembus`** (the wire vocabulary). A master talks to a
deepsleep node and a bluetooth node with the *same* protocol crate, but each node **implements** its
commands in its own module crate. The command-dispatch pattern (UART byte / Stem regmap-write →
`Command` → `execute`) is a **convention each module repeats**, NOT a shared crate — transports and
command sets differ. Do **not** pre-abstract the dispatch into a shared crate.

**`deepsleep`** = the MSP430 deepsleep-supervisor module crate: MCU state (status/identity, mode,
GPIO shadow, FRAM integrity) + command implementations (UART + Stem adapters → one `execute`) +
Stem-slave RegFile serving + the (thin today, growing) sensing/mode logic. **Both prod-bin and diag-bin
build on it** — prod as the API/event loop, diag as the test harness ("diag is a layer on prod" =
"both are harnesses over `deepsleep`"). This supersedes the earlier `supervisor`/`stem-endpoint`/`node`
sketches: one module crate, not an endpoint/role split.

(The gated "stem" library vision — connective tissue every module uses to speak the Stem bus — if it
ever materialises, is a thin layer under `stembus`, separate from this. Keep it gated;
[[stem-nervous-system-vision]].)

---

## Per-module assignment (grounded in the current import graph)

| prod module | lines | depends on today | destination | notes / new abstraction |
|---|---|---|---|---|
| `regmap.rs` | 78 | `stembus::regmap` (re-export) + `ShadowState` | **deepsleep** | `ShadowState` is MCU state |
| `post.rs` | 105 | `stembus::crc`, `bsp::fmt`, `crate::{FW_*, model_name}` | **deepsleep** | needs **build-identity injected** (see §Build identity) |
| `status.rs` | 175 | `enumerate::Scan`, `bsp::model`, `stembus::flags`, `devices::KNOWN`, `crate::{FW_*, model_name}`, `bsp::fmt` | **deepsleep** | same build-identity dep |
| `stem.rs` | 475 | `regmap`, `status`, raw eUSCI slave (ucb1 dual / ucb0 fr2433) | **SPLIT** | **hw slave transport → `bsp::board`** (CONV-3); **RegFile+Slave dispatch → `deepsleep`** |
| `mode.rs` | 182 | `stembus::regmap`, raw `p.p1` sensor pins, `bsp::i2c::init` | **deepsleep** | dual-only (gate off fr2433); route sensor-bus acquire/release via **`bsp::board`** role, not raw `p.p1` |
| `enumerate.rs` | 371 | `bsp::{i2c,hal}`, `devices`, `sched` | **deepsleep** | sensing (thin today, grows); already pure — moves as-is; later converges with diag's scanner |
| `main.rs` | ~480 | everything | **stays prod-bin** | thin harness; selects the role per family; supplies build-identity; later adopts `sched` (ADR-0013) |

All of the above land in ONE crate (`deepsleep`), so there is no cross-crate edge to resolve — e.g.
`status` holding an `enumerate::Scan` is fine intra-crate. The fr2433 (Expander) build feature-gates OUT
`mode`/`enumerate` (Passive-only, no sensor master) while keeping the state/serving pieces.

**Stays elsewhere / already done:** `model.rs` → `bsp::model` (done); `clock`/`uart` → `bsp` (done, CONV-1);
wire contract → `stembus` (done, ADR-0012).

---

## Two hard design points (resolve these, the rest is mechanical)

### A. Stem-slave hardware abstraction (CONV-3 — the gating one)
`stem.rs` today fuses two things: the **eUSCI slave transport** (register-level ucb1/ucb0 init/poll/rx/tx,
clock-stretch handling) and the **protocol dispatch** (RegFile: pointer + read/write onto the regmap).
- **Transport → `bsp::board`** as a `stem_i2c` **slave** role — the non-embedded-hal target abstraction
  CONV-3 names. Per-family instance hidden (ucb1 on dual P3/P4; ucb0 on fr2433 P1.2/3), like `leaf_i2c`.
  Surface: `stem_slave_init(p, addr)`, `stem_slave_poll(...) -> event`, rx/tx byte primitives.
- **Dispatch → `deepsleep`** — the `RegFile`/`Slave` state machine, transport-agnostic.
- This is the real work: a byte-stream slave trait that isn't `embedded-hal` (that's master-only). Design
  it so diag can bring the slave up identically.

### B. Build identity (small, but blocks status/post)
`status.rs` and `post.rs` read `crate::{FW_BUILD, FW_VER_MAJOR/MINOR/PATCH, FW_VERSION}` and
`crate::model_name` — all defined in prod/main.rs (env!-stamped at the **bin** level). `deepsleep` can't
read the bin's consts.
- **Decision needed:** inject a `BuildInfo { ver: (u8,u8,u8), stamp: &'static str }` from each bin into
  the `deepsleep` calls that need it (`Status::booted(model, build)`, `post::boot_log(.., build)`), rather
  than the crate owning the stamp. `model_name` already delegates to `stembus::role` → trivial.

---

## Phased execution (each phase builds + is independently landable)

**Phase 0 — create `deepsleep`; move the no-new-abstraction pieces.** *(low risk, like the model move)*
- Create `crates/deepsleep`. Move `regmap`, `post`, `status`, `mode`, `enumerate` into it; `stem.rs`
  stays in prod-bin for now (still fused — split in Phase 1).
- Resolve **Build identity (§B)**: `BuildInfo` injected from prod-bin.
- Feature-gate `mode`/`enumerate` OFF for fr2433 (Passive-only Expander, no sensor master) so the FR2433
  image pulls only state + serving.
- Route `mode`'s sensor-bus acquire/release through `bsp::board` (new `sensor_bus_acquire/release` role)
  instead of raw `p.p1` pokes.
- prod-bin depends on `deepsleep`.
- **Verify:** prod fr2476/fr2355/fr2433 build within budget; HW smoke on one dual + fr2433.

**Phase 1 — split the Stem slave (§A / CONV-3).** *(the hard one)*
- Transport → `bsp::board` `stem_i2c` slave role; dispatch → `deepsleep`.
- prod-bin wires board-slave + `deepsleep` RegFile.
- **Verify:** SoM/master read-write over Stem (loopback self-test + `@fr2433`/`@fr2355` slave reads).

**Phase 2 — transport-agnostic command surface.** *(feature; STEM-DIRECTION commands over I²C + UART)*
- One command enum + dispatch in `deepsleep`, fed by two front-ends: Stem regmap writes (generalise
  `MODE_CTRL`) and console-UART bytes (the current s/r/d/m/l keys). Both binaries share it.

**Phase 3 — diag consumes `deepsleep` + same-boot.** *(realises the vision)*
- diag depends on `deepsleep`; brings up the Stem slave identically; adds the **~1 s grace window** (master
  commands mode before POST auto-runs); converges diag's `diag.rs` scanner onto `deepsleep` (enumerate);
  drives the capabilities one-at-a-time.
- **Verify:** install diag image, exercise prod functionality; master redirects mode over I²C/UART at boot.
- **Size watch:** diag is ~72 % of 32 KB today (~9 KB free); `deepsleep` (~5 KB) must fit — measure, don't assume.

**Phase 4 — prod-bin slimming (ADR-0013).** *(cleanup)*
- Drop prod's triplicated loop; adopt `sched` over the capabilities; LPM path later.

---

## Risks / watch-items
- **Phase 1 is the real cost** (non-embedded-hal slave abstraction, per-family eUSCI). Phases 2–4 are
  gated on it. Phase 0 is safe and worth doing first regardless.
- **diag size** (Phase 3) — the one place the extraction could bust a budget; measure at each move.
- **mode is dual-only** (fr2433 has no sensor master) — keep it `_dual`-gated; don't force it onto fr2433.
- **Don't let `deepsleep` bloat the prod binary** — capabilities must stay individually invocable +
  feature-gateable so the FR2433 15 KB image pulls only what it ships (STEM-DIRECTION.md size note).

## Status
- Prereqs **done**: `bsp::model` (model-detect), CONV-1 (clock/uart/fmt in bsp), `stembus` contract, ADR-0014.
- **Next actionable:** Phase 0 (un-gated, mechanical). Phase 1/CONV-3 needs the §A design agreed first.
