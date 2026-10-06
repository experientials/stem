# Plan · extracting the MSP430 capability crates (role-structured, NOT a single "prod-core")

**Derived from** [ADR-0014](adr/0014-board-layer-structure.md) (crate boundary) and STEM-DIRECTION.md
("prod is a layer diag builds on"; "same-boot + master-commanded mode"; "commands over I²C and UART").
ADR-0014 is canonical; if this plan disagrees with it, the ADR wins.

**Goal.** Lift prod's capabilities out of the prod binary into shared crate(s) that **both** prod
(event-loop harness) and diag (test harness) depend on — so diag can host and exercise the real prod
code, and the same-boot/master-commanded behaviour falls out. Emptying prod is **not** the goal:
prod-bin stays as the thin API+timer loop.

## Not one crate — structure by ROLE (Henrik 2026-10-06)
*"It could be a single crate, but not obvious that it should be. We have different roles for MCUs and so
far MSP430 is only used for Sensing Supervisor. node sounds generic, so probably not a good direction."*

Reject `node`/`prod-core` as the crate name: `node` is too generic, and `prod-core` names the prod/diag
**build axis**, not the thing. The thing is a **role**, and MCUs play different roles ([[role-based-stem-addressing]];
`stembus::role`: Supervisor / DevStandin / Expander / Power). MSP430's live role today is the **Sensing
Supervisor**.

**The role seam already exists in live code** — `run_dual` (Supervisor) and `run_fr24xx` (the fr2433
Expander role) use different capability subsets:

| capability | Sensing Supervisor (dual) | I/O Expander (fr2433) |
|---|---|---|
| stem-slave dispatch (RegFile), status/identity, post/integrity, regmap/ShadowState | ✅ | ✅ |
| mode machine (Passive/Sensing) | ✅ | ❌ (Passive-only) |
| enumerate (sensor-bus scan) | ✅ | ❌ (no sensor master) |

So the capabilities fall in **two groups**:
- **Stem-endpoint (role-agnostic):** slave dispatch + status + post + regmap — what every MSP430 role
  uses to be a Stem-bus citizen. The natural future home of the gated "stem" library
  ([[stem-nervous-system-vision]]) — but keep that name gated; use an honest interim name.
- **Role capability:** **Sensing Supervisor** = mode + enumerate (+ sensing). Dual-only. The one live role.

**Naming:** role crate = `supervisor` (not `node`/`prod-core`). The shared layer = an interim
`stem-endpoint` (do NOT pre-name it `stem` — the rename is gated on the prod fw proving out).

**Open decision (needs the roadmap):** is the fr2433 **Expander** a role we build around *now* (→ the
endpoint layer earns its own crate immediately, two live consumers), or a placeholder (→ keep endpoint
as a module group inside `supervisor` and lift it to its own crate when a 2nd role goes live)? See
§"Single vs split".

---

## Per-module assignment (grounded in the current import graph)

| prod module | lines | depends on today | destination | notes / new abstraction |
|---|---|---|---|---|
| `regmap.rs` | 78 | `stembus::regmap` (re-export) + `ShadowState` | **stem-endpoint** | role-agnostic; `ShadowState` is endpoint state |
| `post.rs` | 105 | `stembus::crc`, `bsp::fmt`, `crate::{FW_*, model_name}` | **stem-endpoint** | needs **build-identity injected** (see §Build identity) |
| `status.rs` | 175 | `enumerate::Scan`, `bsp::model`, `stembus::flags`, `devices::KNOWN`, `crate::{FW_*, model_name}`, `bsp::fmt` | **stem-endpoint** | same build-identity dep; the `Scan` field is Supervisor-only → generic over / optional |
| `stem.rs` | 475 | `regmap`, `status`, raw eUSCI slave (ucb1 dual / ucb0 fr2433) | **SPLIT** | **hw slave transport → `bsp::board`** (CONV-3); **RegFile+Slave dispatch → stem-endpoint** |
| `mode.rs` | 182 | `stembus::regmap`, raw `p.p1` sensor pins, `bsp::i2c::init` | **supervisor** | role-specific (dual-only); route sensor-bus acquire/release via **`bsp::board`** role, not raw `p.p1` |
| `enumerate.rs` | 371 | `bsp::{i2c,hal}`, `devices`, `sched` | **supervisor** | role-specific (sensing); already pure — moves as-is; later converges with diag's scanner |
| `main.rs` | ~480 | everything | **stays prod-bin** | thin harness; selects the role per family; supplies build-identity; later adopts `sched` (ADR-0013) |

Note the one cross-edge: `status.rs` holds an `enumerate::Scan` (Supervisor-only concept) in a
role-agnostic struct. Resolve by making the scan summary **optional/generic** in the endpoint `Status`
(the Expander reports `devs=0`), so `stem-endpoint` doesn't depend back on `supervisor`.

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
- **Dispatch → `stem-endpoint`::stem** — the `RegFile`/`Slave` state machine, transport-agnostic.
- This is the real work: a byte-stream slave trait that isn't `embedded-hal` (that's master-only). Design
  it so diag can bring the slave up identically.

### B. Build identity (small, but blocks status/post)
`status.rs` and `post.rs` read `crate::{FW_BUILD, FW_VER_MAJOR/MINOR/PATCH, FW_VERSION}` and
`crate::model_name` — all defined in prod/main.rs (env!-stamped at the **bin** level). The capability crate can't
read the bin's consts.
- **Decision needed:** inject a `BuildInfo { ver: (u8,u8,u8), stamp: &'static str }` from each bin into
  the endpoint calls that need it (`Status::booted(model, build)`, `post::boot_log(.., build)`), rather
  than the crate owning the stamp. `model_name` already delegates to `stembus::role` → trivial.

---

## Phased execution (each phase builds + is independently landable)

### Single vs split
Two live roles already use different subsets (table above). Choice:
- **(A) Split now** — `stem-endpoint` crate (role-agnostic) + `supervisor` crate (sensing). The fr2433
  Expander build depends on `stem-endpoint` only. Honest names from day one; justified *iff* the Expander
  is a role we build around now.
- **(B) One `supervisor` crate, endpoint as an internal module group** — lift the endpoint modules into
  their own crate when a 2nd role actually goes live. Less structure now; defers the split.

**Recommendation:** (A) **iff fr2433 Expander is a real parallel role**; otherwise (B). This is the one
decision that needs the roadmap — it's Henrik's call. Either way: name the role `supervisor`, keep the
shared layer's name as interim `stem-endpoint` (NOT `stem` — gated), and structure the modules along the
endpoint/role seam so (B)→(A) is a crate-boundary move, not a rewrite.

**Phase 0 — create the crate(s) per the §"Single vs split" choice; move the no-new-abstraction pieces.**
*(low risk, like the model move)*
- Move endpoint pieces (`regmap`, `post`, `status`) and supervisor pieces (`mode`, `enumerate`) to their
  destinations; `stem.rs` stays in prod-bin for now (still fused — split in Phase 1).
- Resolve **Build identity (§B)**: `BuildInfo` injected from prod-bin.
- Make `Status`'s scan summary optional/generic so `stem-endpoint` doesn't depend on `supervisor`.
- Route `mode`'s sensor-bus acquire/release through `bsp::board` (new `sensor_bus_acquire/release` role)
  instead of raw `p.p1` pokes.
- prod-bin depends on the new crate(s).
- **Verify:** prod fr2476/fr2355/fr2433 build within budget; HW smoke on one dual + fr2433.

**Phase 1 — split the Stem slave (§A / CONV-3).** *(the hard one)*
- Transport → `bsp::board` `stem_i2c` slave role; dispatch → `stem-endpoint`::stem.
- prod-bin wires board-slave + `stem-endpoint` RegFile.
- **Verify:** SoM/master read-write over Stem (loopback self-test + `@fr2433`/`@fr2355` slave reads).

**Phase 2 — transport-agnostic command surface.** *(feature; STEM-DIRECTION commands over I²C + UART)*
- One command enum + dispatch in `stem-endpoint` (shared across roles), fed by two front-ends: Stem regmap writes (generalise
  `MODE_CTRL`) and console-UART bytes (the current s/r/d/m/l keys). Both binaries share it.

**Phase 3 — diag consumes the capability crates + same-boot.** *(realises the vision)*
- diag depends on the capability crate(s); brings up the Stem slave identically; adds the **~1 s grace window** (master
  commands mode before POST auto-runs); converges diag's `diag.rs` scanner onto `supervisor::enumerate`;
  drives the capabilities one-at-a-time.
- **Verify:** install diag image, exercise prod functionality; master redirects mode over I²C/UART at boot.
- **Size watch:** diag is ~72 % of 32 KB today (~9 KB free); the capability crates (~5 KB) must fit — measure, don't assume.

**Phase 4 — prod-bin slimming (ADR-0013).** *(cleanup)*
- Drop prod's triplicated loop; adopt `sched` over the capabilities; LPM path later.

---

## Risks / watch-items
- **Phase 1 is the real cost** (non-embedded-hal slave abstraction, per-family eUSCI). Phases 2–4 are
  gated on it. Phase 0 is safe and worth doing first regardless.
- **diag size** (Phase 3) — the one place the extraction could bust a budget; measure at each move.
- **mode is dual-only** (fr2433 has no sensor master) — keep it `_dual`-gated; don't force it onto fr2433.
- **Don't let the capability crates bloat the prod binary** — capabilities must stay individually invocable +
  feature-gateable so the FR2433 15 KB image pulls only what it ships (STEM-DIRECTION.md size note).

## Status
- Prereqs **done**: `bsp::model` (model-detect), CONV-1 (clock/uart/fmt in bsp), `stembus` contract, ADR-0014.
- **Next actionable:** Phase 0 (un-gated, mechanical). Phase 1/CONV-3 needs the §A design agreed first.
