---
name: prove-firmware-on-hardware
description: >-
  The PROCESS for porting MSP430 firmware (diag/prod) to a new part/role AND proving it actually
  RUNS on real silicon — not merely that it compiles, flashes, and byte-verifies. Use whenever
  bringing firmware up on a new MCU or board (e.g. FR2476→FR2355/FR2155/FR2433), validating a port,
  adding observability, or hitting "it builds and flashes but is it actually running / the console is
  silent". Covers the multi-channel validation methodology (JTAG/SBW memory-read of a boot record,
  the I2C slave register surface, console UART, GPIO liveness), the boot-record + feature-gated
  debug-macro primitives, the port→build→flash→PROVE loop, and the definition of "proven working".
  Triggers on: "prove it works on hardware", "validate without console", "boot confirmation",
  "the console is silent", "is the firmware actually running", "port diag/prod to <part>", "debug
  macros / trace flags", or any MSP430 bring-up where compile ≠ works. Complements `stem-dev`
  (build/flash mechanics) and `thepia-hwd` (driving the board).
---

# Prove firmware works on real hardware (port-and-prove)

**Core principle — compile ≠ works.** A build that compiles, flashes, and **byte-verifies** proves
only that the right bytes are in FRAM. It does **not** prove the chip boots, the clock is right, the
UART transmits, or any function runs. Proof = **observed correct behaviour on real silicon**, over a
channel that doesn't depend on the thing you're bringing up. (Bench lesson 2026-09-25: an FR2355 diag
compiled + flashed + byte-verified perfectly and produced **zero** console output — clock/pins bug.
Byte-verify said "done"; the chip said nothing.)

## The iteration constraint: one wire — USB → eZ-FET

**While you iterate, the ONLY feedback is the eZ-FET over USB.** No scope, no logic probe, no wired
I²C master, no human watching an LED. That single connection is **two sub-channels, and only two**:

1. **SBW/JTAG debug** (via mspdebug over the eZ-FET) — **robust**: needs nothing on the target to be
   working. Read FRAM/RAM, halt the CPU, read the **PC/registers** (running? where stuck?),
   single-step. **This is the feedback that survives a dead clock/UART.**
2. **eZ-FET backchannel UART** — **fragile**: needs target clock + baud + pin-mux + the backchannel
   routing all correct. Rich when it works; silent on the smallest bring-up bug.

## Validation channels — by availability

| Channel | Needs a working… | Available to an *iterating agent*? | Proves |
|---|---|---|---|
| **SBW/JTAG memory read + halt/PC** | nothing (eZ-FET halts CPU) | **✅ yes — the bedrock** | boot record, boot-stage, where it hung |
| **Console UART (backchannel)** | clock + baud + pins | ✅ yes, but **fragile** | human POST + debug detail |
| **I²C slave register surface** | a wired I²C **master** | ❌ no (SoM/bench only) | production proof (SoM reads it) |
| **GPIO / LED** | human eyes / a probe | ❌ no | crude liveness only |

**Consequences for the design:**
- The **boot record must be readable over SBW memory-read** (a known FRAM/RAM address) — that is the
  proof channel during iteration, because it's the only robust thing on the one wire you have.
- **Debug macros must be able to target a RAM ring buffer** (SBW-readable), not only the UART — so
  detail survives a dead console.
- **Never let console be the only proof.** It depends on the most things (DCO/SMCLK, baud, pin-mux,
  backchannel routing) — exactly what a port breaks. A silent console is a *bug to diagnose over SBW*
  (read `boot_stage`, read the PC), not a failed port.
- I²C-slave / GPIO proof come later, on a wired bench or in production — not in the agent loop.

## Two validation tiers — iterate on tier 1, prove for real on tier 2

Proof is **combined across two tiers**; a feature is fully proven only when it passes both.

| | **Tier 1 — dev-board iteration** | **Tier 2 — full-bench HIL** |
|---|---|---|
| Wiring | LaunchPad on **USB → eZ-FET** | MCU's **UART + I²C wired to the SoM M7 core** |
| Master | mspdebug over SBW | **the SoM M7 core** (the real stembus master) |
| Channels | SBW-read + fragile backchannel UART | production channels: **I²C register surface + UART**, driven by the M7 |
| Proves | chip-level: boots, clock/UART up, functions run | **system-level**: the MCU answers the *real* master over the *real* bus, in the production topology |
| Speed | fast, agent-driven, no extra hardware | slower, integration, needs the configured bench |

- **Tier 1 is the fast bring-up loop** — where you actually *iterate* (the one-wire constraint above).
  SBW-read the boot record; get the port booting and functioning at the chip level.
- **Tier 2 is the acceptance proof** — the SoM M7 masters the stembus and runs actual tests against
  the MCU's I²C register surface + UART, exactly as production will. This is where the I²C-slave and
  UART-to-SoM channels (tier-1-unavailable) become the *primary* proof. Ties to hwd's HIL verbs
  (`C22` flash→run→assert, `C25` self-test harness).
- **The `proof-of-feature` log records which tier(s) a feature passed.** "Proven" for a shipping
  feature means **both**: tier-1 chip-level on the dev board *and* tier-2 system-level on the M7 bench.

## Observability primitives every image must carry

1. **Boot record** — at boot, firmware writes a fixed-layout struct to a **known address** (a
   reserved FRAM/RAM location, documented in `regmap.rs`/`memory.x`):
   `{ magic, fw_version(maj.min.patch), build_checksum, boot_stage, self_test_bitmap }`.
   - Read over **JTAG** during bring-up (host reads the address); **mirrored into the I²C register
     map** so the SoM reads the same values in production. One source of truth, two channels.
   - `boot_stage` advances through init (clock set, UART up, I²C up, scan done) so a **hang** is
     located by *how far it got*, not guesswork.
   - `build_checksum` ties into the field-update integrity model (STEM-DIRECTION.md) — the same
     checksum the loader verifies is the one the boot record reports.
2. **Feature-gated debug macros** — `dbg!`/`trace!`-style macros compiled in only under a `debug` /
   `trace` cargo feature. Emit to the console **or** a small **JTAG-readable ring buffer** in RAM
   (so detail survives even with no UART). **Off by default** → production is silent and small
   (respects the FR2433 15 KB / FR2355 4 KB budgets). Enable per-build for a hard bring-up.

## The port-and-prove loop

1. **Scope the deltas** — datasheet (memory map, pin-mux, peripheral set) + **PAC diff**. Cite values.
2. **Trial-compile** against the new PAC to flush *code* deltas — but this is step 2, not the finish.
3. **Build real firmware** for the part (feature-gated chip select; don't leave a throwaway swap).
4. **Flash + byte-verify** on the real board (`thepia hwd msp flash`).
5. **PROVE — over JTAG/I²C first:**
   - Read the **boot record** → assert magic + version + checksum + `boot_stage == ready` +
     self-test bitmap. **This is the proof of "it runs."**
   - Then exercise functions and assert their results (I²C scan bitmap, ADC range, GPIO) via the
     boot record / I²C reads — not by eyeballing console.
   - Console banner + debug trace are the *human* view, checked last.
6. **If a channel is silent, drop down a channel to diagnose** (console dead → read the boot record
   over JTAG to see `boot_stage`; if `boot_stage` shows it never got past clock init → DCO/SMCLK;
   past UART-up but no output → baud or pins). **Do not** call the port done because it flashed.

## Definition of "proven working" (the bar for a port)

A port/build is **proven** only when, on real silicon:
- ✅ boot record reads back with correct magic + version + **build_checksum** + `boot_stage == ready`,
- ✅ the self-test bitmap passes (or absences are explained, e.g. "no I²C devices attached"),
- ✅ at least one **functional** assertion holds (I²C scan / ADC / GPIO), read over JTAG or I²C,
- ✅ console banner matches (when a console is expected).

"It compiled and flashed" is **not** proven. Record the evidence (which channels, what values) — this
is the transparency bar (`high-bar-mandate`).

## Multi-model single image — prove on EVERY model, not just one

A "combined" image meant to run on multiple parts (e.g. the FR2355+FR2476 combined/dynamic PAC — see
`stem-dev`) is **NOT proven by building once + testing one chip**. Flash the **same binary** (byte-verified)
to **each** target part and confirm the functional assertion on **each**. The runtime divergences
(per-model peripheral base, IRQ vector slot, pins) only show up on the model that uses the *other* branch.
- **Proven technique (2026-10-10):** one binary → flashed to @fr2476 AND @fr2355 → `scan` shows both role
  addrs (`0x20 0x21`), and a slave read returns the sentinel (`0x5A`) on EACH → proves each chip woke from
  LPM via *its* vector slot (0xFFDA vs 0xFFDE) with *its* runtime base/pins. One chip answering proves nothing
  about the other.
- **Watch the console timing trap:** a slower-waking part can clip a short `console interact --expect`
  timeout and look like "no answer" when it's fine. Re-read with `console send` + `console tail` + a
  generous wait before concluding a failure (this exact false-negative happened on FR2355 — the clean read
  was `0x5A`).

## Proof-of-feature mode → a committed feature implementation log

Encapsulate proof **in the source, per feature.** Build/run a feature in **`proof-of-feature` mode**
(a cargo feature + a host harness) that drives step 5 and **captures a timestamped log of all relevant
device state**, then save it as a **feature implementation log committed alongside the source**
(e.g. `proofs/<feature>-<part>.log`) — versioned, auditable evidence that the feature was **proven on
real silicon** at a given commit.

**Why commit the proof with the code:**
- The repo carries its own evidence — "feature X works on FR2355" is checkable from `git`, not by
  re-running or trusting an assertion (this *is* the high-bar transparency mandate, made durable).
- A regression shows up as a **diff**: re-run `proof-of-feature`, compare to the committed log.
- It pins the exact device (part + serial), build stamp + **checksum**, and observed values.

**Log contents** (structured + greppable): timestamp · git commit + build stamp + **build_checksum**
(the same one the field-update loader verifies — STEM-DIRECTION.md) · target part + serial · boot-record
dump · self-test bitmap · per-function assertions (**expected vs observed**) · channels used
(JTAG/I²C/console) · overall **PASS/FAIL**.

**Churn caveat (design it in):** timestamps + serials change every run, so **do not regenerate-and-
commit on every build** — update a proof only when the *feature or port materially changes*, treating
it like a **golden/snapshot** artifact (deliberate, reviewed diff). Keep the volatile header
(timestamp, serial, run id) **separate** from the stable body (assertions, boot-record layout) so a
re-proof of unchanged behaviour diffs cleanly — otherwise the logs become noise and stop being read.

## Tooling note (thepia hwd)
Realizing the JTAG channel needs a **target-memory read** verb — e.g. `thepia hwd msp read <addr> <len>`
(mspdebug `md`) — so the host can read the boot record without driving the eZ-FET by hand. If it's
missing, `report` it (don't drive mspdebug directly — that crosses the "thepia manages the eZ-FET"
line). Until then, the I²C-slave channel + console are available; JTAG-read is the gap to close.

## Reference docs
Datasheets/user-guides live in `bob-929/Hardware/datasheets/` (canonical) + `stem/datasheets/`
(firmware-local) — see **`bob-929/Hardware/datasheets/INDEX.md`** for the index + hard-won facts.
**Save resources; index them; reference them here.** A missing board user-guide (SLAU680) cost a full
silent-console detour below.

## Per-part bring-up gotchas (grow this list)
- **FR2476 → FR2355 (2026-09-25):**
  - Register API parity is total except **Timer_A → Timer_B** (`usec.rs`, `ta0`→`tb1`).
  - Memory: 4 KB RAM (`0x2000/0x1000`); **vector region stays minimal `0xFFE0/0x0020`** for the no-ISR
    diag (the datasheet's full `0xFF80` region makes ld fail ".vector_table shorter than expected").
  - **⚠ Backchannel UART is `eUSCI_A1` on the FR2355 LaunchPad, not A0** (SLAU680 §2.2.4) — the FR2476
    LaunchPad uses A0. Switch `uart.rs` to `uca1*`/`e_usci_a1` and route **P4.2/P4.3** (`P4SEL0|=0x0C`),
    not P1.6/P1.7. **The UART-module difference per board is the #1 porting trap.**
  - **Still-open:** even after the A1 fix the console is silent on both A0 and A1 ⇒ cause is common =
    **the clock** (SMCLK≠1 MHz, or `clock_init_1mhz` hanging on the FR2355 FLL-lock loop). **Blocked on
    observability** (needs `msp read` to see `boot_stage`) — do NOT blind-poke clock registers. See
    `diag/FR2355-SCOPE.md`.

## Related
`stem-dev` (build/flash mechanics, `connections.toml`), `thepia-hwd` (driving boards, verify-by-
stamp), STEM-DIRECTION.md (roles, field-update integrity), `high-bar-mandate` (prove, don't assert).
