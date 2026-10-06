# Plan · prod-core extraction (the shared node-capability crate)

**Derived from** [ADR-0014](adr/0014-board-layer-structure.md) (crate boundary) and STEM-DIRECTION.md
("prod is a layer diag builds on"; "same-boot + master-commanded mode"; "commands over I²C and UART").
ADR-0014 is canonical; if this plan disagrees with it, the ADR wins.

**Goal.** Lift prod's *node capabilities* out of the prod binary into a shared crate that **both** prod
(event-loop harness) and diag (test harness) depend on — so diag can host and exercise the real prod
code (Stem slave, mode, status, enumerate, integrity), and the same-boot/master-commanded behaviour
falls out. Emptying prod is **not** the goal: prod-bin stays as the thin API+timer loop.

**Crate name:** `node` (candidate) — the node's capability library; aligns with the long-term "stem"
library vision ([[stem-nervous-system-vision]]). Decide at Phase 0; `prod-core` used as the label here.

---

## Per-module assignment (grounded in the current import graph)

| prod module | lines | depends on today | destination | notes / new abstraction |
|---|---|---|---|---|
| `regmap.rs` | 78 | `stembus::regmap` (re-export) + `ShadowState` | **prod-core** | trivial; `ShadowState` is node state |
| `enumerate.rs` | 371 | `bsp::{i2c,hal}`, `devices`, `sched` | **prod-core** | already pure — moves as-is; later converges with diag's scanner |
| `post.rs` | 105 | `stembus::crc`, `bsp::fmt`, `crate::{FW_*, model_name}` | **prod-core** | needs **build-identity injected** (see §Build identity) |
| `status.rs` | 175 | `enumerate::Scan`, `bsp::model`, `stembus::flags`, `devices::KNOWN`, `crate::{FW_*, model_name}`, `bsp::fmt` | **prod-core** | same build-identity dependency |
| `mode.rs` | 182 | `stembus::regmap`, raw `p.p1` sensor pins, `bsp::i2c::init` | **prod-core** | route sensor-bus acquire/release via **`bsp::board`** (semantic pin role), not raw `p.p1` |
| `stem.rs` | 475 | `regmap`, `status`, raw eUSCI slave (ucb1 dual / ucb0 fr2433) | **SPLIT** | **hw slave transport → `bsp::board`** (CONV-3); **RegFile+Slave dispatch → prod-core** |
| `main.rs` | ~480 | everything | **stays prod-bin** | becomes a thin harness over prod-core; supplies build-identity; later adopts `sched` (ADR-0013) |

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
- **Dispatch → prod-core::stem** — the `RegFile`/`Slave` state machine, transport-agnostic.
- This is the real work: a byte-stream slave trait that isn't `embedded-hal` (that's master-only). Design
  it so diag can bring the slave up identically.

### B. Build identity (small, but blocks status/post)
`status.rs` and `post.rs` read `crate::{FW_BUILD, FW_VER_MAJOR/MINOR/PATCH, FW_VERSION}` and
`crate::model_name` — all defined in prod/main.rs (env!-stamped at the **bin** level). prod-core can't
read the bin's consts.
- **Decision needed:** inject a `BuildInfo { ver: (u8,u8,u8), stamp: &'static str }` from each bin into
  the prod-core calls that need it (`Status::booted(model, build)`, `post::boot_log(.., build)`), rather
  than prod-core owning the stamp. `model_name` already delegates to `stembus::role` → trivial.

---

## Phased execution (each phase builds + is independently landable)

**Phase 0 — create the crate; move the no-new-abstraction pieces.** *(low risk, like the model move)*
- Create `crates/prod-core` (decide name). Move `regmap`, `enumerate`, `post`, `status`, `mode`.
- Resolve **Build identity (§B)**: `BuildInfo` injected from prod-bin.
- Route `mode`'s sensor-bus acquire/release through `bsp::board` (new `sensor_bus_acquire/release` role)
  instead of raw `p.p1` pokes.
- prod-bin depends on prod-core; `stem.rs` stays in prod-bin for now (still fused).
- **Verify:** prod fr2476/fr2355/fr2433 build within budget; HW smoke on one dual + fr2433.

**Phase 1 — split the Stem slave (§A / CONV-3).** *(the hard one)*
- Transport → `bsp::board` `stem_i2c` slave role; dispatch → prod-core::stem.
- prod-bin wires board-slave + prod-core RegFile.
- **Verify:** SoM/master read-write over Stem (loopback self-test + `@fr2433`/`@fr2355` slave reads).

**Phase 2 — transport-agnostic command surface.** *(feature; STEM-DIRECTION commands over I²C + UART)*
- One command enum + dispatch in prod-core, fed by two front-ends: Stem regmap writes (generalise
  `MODE_CTRL`) and console-UART bytes (the current s/r/d/m/l keys). Both binaries share it.

**Phase 3 — diag consumes prod-core + same-boot.** *(realises the vision)*
- diag depends on prod-core; brings up the Stem slave identically; adds the **~1 s grace window** (master
  commands mode before POST auto-runs); converges diag's `diag.rs` scanner onto `prod-core::enumerate`;
  drives prod-core capabilities one-at-a-time.
- **Verify:** install diag image, exercise prod functionality; master redirects mode over I²C/UART at boot.
- **Size watch:** diag is ~72 % of 32 KB today (~9 KB free); prod-core (~5 KB) must fit — measure, don't assume.

**Phase 4 — prod-bin slimming (ADR-0013).** *(cleanup)*
- Drop prod's triplicated loop; adopt `sched` over prod-core capabilities; LPM path later.

---

## Risks / watch-items
- **Phase 1 is the real cost** (non-embedded-hal slave abstraction, per-family eUSCI). Phases 2–4 are
  gated on it. Phase 0 is safe and worth doing first regardless.
- **diag size** (Phase 3) — the one place the extraction could bust a budget; measure at each move.
- **mode is dual-only** (fr2433 has no sensor master) — keep it `_dual`-gated; don't force it onto fr2433.
- **Don't let prod-core bloat the prod binary** — capabilities must stay individually invocable +
  feature-gateable so the FR2433 15 KB image pulls only what it ships (STEM-DIRECTION.md size note).

## Status
- Prereqs **done**: `bsp::model` (model-detect), CONV-1 (clock/uart/fmt in bsp), `stembus` contract, ADR-0014.
- **Next actionable:** Phase 0 (un-gated, mechanical). Phase 1/CONV-3 needs the §A design agreed first.
