# stem backlog — tracked tasks & bugfixes

Durable list so findings don't get lost in chat. Each item: **severity · area · evidence · fix ·
status**. Check the box when done. Keep newest context at the top of each item.

---

## Bugs — found by exact console-output vs source comparison (FR2355 bench, 2026-09-30, build hd2355)

- [x] **BUG-1 · regs.rs pin-expectation table is FR2476-specific** — *DONE 2026-09-30* — `board::
  console_uart_pin_report` (reads the right port); **hardware-confirmed on FR2355**: dump now prints
  `P4.2 want=UCA1RXD sel=01 module` / `P4.3 want=UCA1TXD sel=01`, not the old P1.4/5 UCA0.
  - `diag/src/regs.rs:166-167` hardcode `P1.4="UCA0TXD"`, `P1.5="UCA0RXD"` (the FR2476 UART pins).
    On FR2355 the console UART is **UCA1 on P4.2/P4.3**, so the SFR dump prints
    `P1.4 want=UCA0TXD sel=00` — misleading a bench operator into thinking the UART is misconfigured.
  - Evidence: FR2355 console dump shows the P1.4/5 "want=UCA0…" lines while the UART actually works
    on UCA1 (the *register* dump `[console UART] UCAx…` — already board-aware — is correct).
  - Fix: make the pin-expectation table board-aware (have `board` expose the UART **port + pins**), so
    the dump reads the port the active board uses. NOTE: not the same pattern as the register-dump fix —
    FR2355's UART pins are on a **different port** (P4 vs P1), so the dump must read P4SEL for fr2355.
  - Note: I fixed the UART *register* dump (`board::console_uart_regs`) but missed this *pin* table.

- [ ] **BUG-2 · regs.rs chip-id recognition lacks FR2355/FR2155/FR2433** — *low (cosmetic)*
  - `diag/src/regs.rs:40-42` `chip_name()` maps only `0x832A`→FR2476, `0x832B`→FR2475. FR2355 (TLV
    id `0x830C` observed on the bench) → prints `MSP430 (unrecognized)`.
  - Fix, split by confidence:
    - **FR2355 `0x830C` — DONE 2026-09-30**, hardware-confirmed: `chip_name` now maps it; the board
      (JTAG id `0x01ff`) prints `chip: MSP430FR2355 id=830C` (was "unrecognized").
    - **FR2433 `0x8240` — DONE 2026-09-30**, empirically read over SBW (`msp read 0x1A04`) on a
      JTAG-confirmed FR2433 (id `0x01c6`), stable across reads, past the leading `0x55` artifact.
      Added to `chip_name`. (Not firmware-confirmed — diag doesn't run on fr24xx, see CONV-4/#3.)
    - **FR2155 id = gated on FUTURE hardware** — *not just "not attached now."* Henrik 2026-09-30:
      *"You can only test fr2155 logic with fr2355 dev board until some future where I might have a
      prod board with 2155."* Reading the FR2355 dev board yields FR2355's TLV (`0x830C`), NOT
      FR2155's — so FR2155's id can't be captured until a real FR2155 board exists. Do not guess.
      Item stays open **only** for FR2155.

- [ ] **BUG-3 · WDT-timeout reset loop on a stuck/no-shield I²C bus** — *medium (functional)*
  - Evidence: `SYSRSTIV=0016` (WDTIFG) + the boot banner **reprints** between POST cycles → the board
    reboots via the ~16 s watchdog backstop. Bus reads `SDA=L SCL=L` (no sensor shield / no pull-ups).
  - Likely cause: the POST's I²C ops on a stuck bus (112-addr scan + device inits + `recover`) hog the
    cooperative loop past the 16 s backstop before `pet_wdt` runs again (the code comments even note
    "the ~3 s POST hogs the loop").
  - Fix: bound the POST's total stuck-bus time (or pet the WDT inside long POST work) so a stuck bus
    degrades gracefully instead of reset-looping. **Confirm** root cause with a sensor shield attached.
  - Caveat: not confirmed as a regression — no `fr2355run1` reset-cause captured to compare.

---

- [ ] **BUG-5 · FR2355 Leaf `read_reg` fails on a device the probe ACKs (marginal pull-ups?)** — *low (bench/electrical; not blocking)*
  - Evidence (2026-10-06, MCP23017 I/O expander @0x20 on the FR2355 Leaf, 5.6 kΩ pull-ups): the address
    **probe ACKs** (`I2C scan: 0x20`, inventory `present`) but `read_reg(0x20, 0x00)` (IODIRA) returns
    `false` → `id-read FAILED`. A **transaction** failure (NACK/timeout mid-sequence), not an `id=XX
    MISMATCH`.
  - Likely cause: NOT the code — `i2c.rs` is shared `bsp` and the same `read_reg` is proven on **FR2476**
    (VL53L0X WHO_AM_I-style reads + ranging, 2026-10-06). `read_reg` does a **repeated-START read** with a
    master-TX→slave-TX turnaround, far more rise-time-demanding than a bare address probe — so weak 5.6 kΩ
    pull-ups into the bus capacitance ACK the probe but fail the read. Consistent with the long-open FR2355
    Leaf electrical root-trigger (see [[i2c-scl-low-wedge-recovery]]).
  - Confirm (A/B): move the MCP23017 to the **FR2476 Leaf** (clean bus) OR fit **2.2 kΩ** pull-ups on the
    FR2355 Leaf and re-run. `id=FF OK` on the stronger bus confirms electrical AND closes the FR2355 Leaf
    root-cause with a concrete reproduction. (MCP23017 added to diag's `DEVICES` manifest @0x20 with IODIRA
    POR=0xFF as a POST-time signature — recognition works; only the read is unconfirmed on this bench.)
  - Optional aid: finer-grained `read_reg` logging (which phase NACKs) — hold until the pull-up test.

---

- [x] **BUG-4 · prod `model.rs` FR2355 id was a placeholder → false "WRONG-FAMILY FLASH"** — *FIXED
  2026-09-30*. `DEVICE_ID_FR2355 = 0x830C` (was placeholder `0xF235`); FR2476 `0x832A` / FR2433 `0x8240`
  were already correct. Verified on hardware: FR2355 now boots `(FR2355)`, no wrong-family warning,
  status `0x0B→0x03`. FR2155 stays a placeholder (no board — can't read its own TLV; see [[diag-prod-variants]]).

## Convergence / tech-debt — from the diag/prod → bsp work (STEM-DIRECTION.md)

- [x] **CONV-1 · prod `clock.rs`/`uart.rs` → `bsp::board`** — *DONE 2026-10-06 ([ADR-0014](adr/0014-board-layer-structure.md))* · [GitHub #5](https://github.com/experientials/stem/issues/5)
  - Gate resolved by ADR-0014 (complementary axes: `bsp::board` = compile-time family, `bsp::model` =
    runtime member; LaunchPad modules serve as product-board modules until a product PCB exists), then:
    - [x] **clock:** prod/`clock.rs` deleted; run_dual + run_fr24xx call `bsp::board::clock_init_1mhz`
      (b9ef0e2). New fr2433 board module carries the factory-trim init.
    - [x] **uart:** formatters moved to `bsp::fmt` (shared, one copy — 5a246b6); the fr2433 console
      folded into `bsp::board::fr2433_launchpad` (b9ef0e2); both `uart.rs` collapse to
      `pub use bsp::fmt::*` + init/rx/getc board delegates. HW-verified @fr2433.
  - **Unblocked by this:** the **`deepsleep` extraction** (Stem slave CONV-3, `mode`/`status`/`post`
    lift, Leaf-scan convergence) — **full plan: [DEEPSLEEP-EXTRACTION.md](DEEPSLEEP-EXTRACTION.md)**
    (derives from ADR-0014).
- [ ] **CONV-2 · `bootrec.rs` de-hardcode** — *low (dormant); PAC import DONE, full agnosticism open*
  - DONE 2026-09-30: `diag/src/bootrec.rs` now uses `crate::pac::Peripherals` (via diag's bsp re-export),
    not the hardcoded `msp430fr2355`. Not build-exercised (still `mod`-undeclared/dormant).
  - Still open: making it *truly* chip-agnostic — its snapshot fields assume the fr2355 board
    (UCA1 ctlw0, p4sel0). Finish when it's actually wired in.
- [ ] **CONV-3 · Stem-I²C slave driver + dispatch split** — *feature (prod milestone)* · [GitHub #2](https://github.com/experientials/stem/issues/2) · **plan: [DEEPSLEEP-EXTRACTION.md §A](DEEPSLEEP-EXTRACTION.md)**
  - Split `stem.rs`: slave **driver + ISR → `bsp/src/stem_i2c.rs`** (sibling to `i2c.rs`, event seam),
    **pins → `board`**, **dispatch → `deepsleep`** (async over `SlaveEvent`). Non-embedded-hal target;
    interrupt-driven + LPM. See the plan for the event-seam decision and the wake model.
- [ ] **Command surface (Phase 2) — the COMMON cross-MCU control API (`postcard` over I²C + UART) + firmware
  patches** — *feature; design captured, build later* · **note: [DEEPSLEEP-EXTRACTION.md §2a](DEEPSLEEP-EXTRACTION.md)**
  - The SoM masters TWO buses (Henrik 2026-10-06): a **sensor bus** (sensors + MSP-passive-slave) and a
    **control bus** for the MCUs. On the control bus, ALL MCU modules (deepsleep/lowvision/bluetooth) speak
    ONE common `stembus::Command` API — `postcard`-serialized; I²C transaction (`START…STOP`) frames it for
    free, UART needs COBS. **Firmware patches ride this bus** (streamed; MSP→host BSL reflash, see
    `msp-fw-ota-host-root-of-trust`). **PCA9698-emulation is optional per-module compat, NOT the common
    denominator** (doubtful all MCUs support it) — where a module opts in, the CMD/RESP stream port lives in
    the 0x30–0x3F window so the stock `gpio-pca953x` driver keeps 0x00–0x1F untouched. Rides the proven 1b
    `next_event()` seam (accumulate `Rx` → decode on `Stop`). Full design + caveats + port addresses in §2a.
- [x] **CONV-6 · fr24xx (FR2433) I²C-SLAVE / regmap surface — the node's real role** — *DONE (boot-
  verified) 2026-09-30; physical GPIO/ADC backing + master-reads-slave proof still open*
  - Built the fr24xx node's whole job: answer the SoM's PCA9698-emulation register file on the single
    eUSCI_B0 (UCB0). Reused the chip-agnostic contract WHOLESALE — `regmap.rs` unchanged; `RegFile`/
    `Slave`/`poll` in `stem.rs` unchanged. Only the peripheral seam differs, isolated in a new cfg-split
    `stem::hw` module: DUAL → `e_usci_b1`/`ucb1*`/P3.2-6 (unchanged); fr24xx → `usci_b0_i2c_mode`/`ucb0*`
    *(correction 2026-10-01: "P3.2-6" holds only for FR247x; the dual path was later chip-split —
    FR23xx/FR21xx UCB1 = P4.6/P4.7 via `_stem_p3`/`_stem_p4`. See stem.rs.)*
    (IFG = `ucb0ifg_i2c`, mode-split PAC) / P1.2-3. `status.rs` decoupled from the sensor scan
    (`from_scan` gated `_dual`; `booted`/`read_reg`/`dump` shared). `run_fr24xx` now: detect → booted+
    Passive status → `stem::init` (UCB0 slave @0x20) → print-once banner+dump (console) → serve loop.
  - **DUAL PATH PROVEN UNCHANGED**: fr247x+console ELF is **byte-identical** (sha256) to committed HEAD
    with a pinned `PROD_BUILD` — the `hw`-seam extraction changed zero machine code on the dual families.
  - **fr24xx BOOT-VERIFIED on the bench** (@fr2433, sanctioned flash+console): the new node runs to the
    serve loop without faulting (reached the dump AFTER `stem::init`, so UCB0-slave config didn't hang),
    print-once (not the old reset-loop). The debug window (0x30–0x3F) reads **byte-exact** to source:
    `D0 40 82 5C BF D9 FD 01 00 00 00 00 01 00 00 00` → IFACE=D0, model=**0x8240 (FR2433)**, build=FDD9BF5C,
    status=0x01 (**BOOTED, no WRONG_FAMILY** — family recognised), ver=0.1.0, mode=0x00 (**PASSIVE**). Sizes:
    fr24xx+console 2814 B, **silent production image 918 B** (was a 110 B idle stub).
  - **UCB0 pins + SEL CONFIRMED (2026-09-30, gap #2 closed):** MSP430FR2433 datasheet **SLASE59F Table
    6-17 (Port P1 Pin Functions)** — the register-level decode, not an inference: **P1.2 P1SELx=01 →
    UCB0SIMO/UCB0SDA**, **P1.3 P1SELx=01 → UCB0SOMI/UCB0SCL** (also Fig 4-1 pin identity). Matches
    `hw::init` (SEL1:0 = 0,1). (Earlier I mis-justified SEL=01 by pointing at the UCA0 console — a
    *different* peripheral/pins; the P1 table is the correct source and confirms it directly.)
  - **PHYSICAL GPIO BANK BACKING DONE + BENCH-VERIFIED (2026-09-30, gap #3):** new cfg-split `stem::gpio`
    seam wires the PCA9698 IP/OP/IOC banks to real ports — bank 0 = P1, bank 1 = P2 (byte ports), with
    USABLE masks that exclude the reserved bus pins (P1.2/3 I²C, P1.4/5 console → P1_GPIO=0xC3). IOC→PxDIR
    (`PxDIR=!IOC`), OP→PxOUT, IP←(PxIN^polarity), all masked. Bank 2 (P3) stays shadow (per-bit PAC, only
    P3.0–2 bonded). Proven on hardware WITHOUT an I²C master via `stem::gpio_selftest`: drive **P1.0**
    through the RegFile OP/IOC path, sense back on IP → **`gpio bank0 self-test (P1.0 drive->sense): PASS`**
    → IOC/OP/IP + the RegFile read/write dispatch all work end-to-end on real silicon. Dual path stays
    **byte-identical** (cfg-split `read_ip`/no-op gpio). fr24xx+console 3138 B, silent 1046 B.
  - **Mask correction (2026-09-30):** first cut used `P2_GPIO=0xFF` on an unverified "XT1 unpopulated"
    claim. FIXED to **0xFC** — P2.0/P2.1 are **XOUT/XIN** (LFXT crystal; datasheet Table 6-18), excluded
    unconditionally so a populated crystal can't be driven. Masks are the datasheet-safe superset (P1:
    exclude UCB0+UCA0 bus pins; P2: exclude the crystal), still PROVISIONAL vs the product BOM.
  - **STILL OPEN (honest gaps):** (a) an actual I²C **master reading/writing** the slave on P1.2/P1.3 is
    UNVERIFIED — the eZ-FET/hwd is not an I²C master, so the boot dump + self-test prove the register FILE,
    the RegFile dispatch, and the GPIO backing, but NOT the eUSCI_B0 wire-level slave servicing (poll
    RX/TX/STT/STP). (b) the USABLE masks (which bits are wired as GPIO on the product) are PROVISIONAL
    pending an FR2433 connections.toml/BOM. (c) bank 2 (P3) + ADC voltages (0x2C/2D) not yet backed.

- [x] **CONV-5 · fr24xx (FR2433) has NO console/debug path** — *DONE 2026-09-30 (superseded by CONV-6:
  the looping spike graduated to print-once + the serve loop). Original notes kept for the record.*
  - Symptom: prod's `uart`/console is gated `#[cfg(all(_dual, console))]`; FR2433 isn't `_dual`, so
    `--features console` is a **no-op** — console and silent builds are byte-identical (110 B). No
    banner / no debug output on an FR2433. NOT a regression: `run_fr24xx` is an explicit TODO stub, and
    console was `fr247x`-only historically → never present for fr24xx.
  - Investigated 2026-09-30 (spike paused before flashing). CORRECTED understanding: the FR2433 UART is
    **NOT** a different register model — `usci_a0_uart_mode` exposes the SAME accessors
    (`uca0ctlw0`/`uca0brw`/`uca0mctlw`/`uca0ifg`/`uca0txbuf`/`uca0rxbuf`) as the dual families' eUSCI.
    The **only** difference is the peripheral **field name**: `usci_a0_uart_mode` (FR2433) vs
    `e_usci_a0` (dual). (My first attempt used the wrong field name `usci_a0`; and I briefly mis-read a
    filtered grep as "different registers" — both wrong.)
  - Tractable fix (the spike): a `uca0!` field-name macro in `prod/uart.rs` (`e_usci_a0` for `_dual`,
    `usci_a0_uart_mode` for fr24xx; same register methods) + un-gate `mod uart`/`mod clock` for
    console + clock factory-trim + bounded-wait for fr24xx (FLL regs present, like FR2355) +
    `run_fr24xx` console (P1.4/5 pins) + flash/verify on the attached FR2433.
  - **SPIKE SUCCEEDED 2026-09-30 (uncommitted).** FR2433 prints a clean banner
    `== prod v0.1.0 fr2433/… fr24xx (FR2433 Passive) — console up ==` at correct 9600 baud → proves the
    macro (usci_a0_uart_mode), the factory-trim clock (clean baud = FLL locked at 1 MHz), and P1.4/5 UCA0
    (the FR2433 LaunchPad backchannel — confirmed) all work. fr24xx+console = ~500 B (was 110 B silent).
    Verified the sanctioned way via `thepia hwd console tail` (once EXT-3 was fixed 2026-09-30). Caveat:
    the spike loops the banner — production is print-once + serve the I2C-slave surface / ADC / GPIO (the
    real fr24xx role, GitHub #3). Next: tidy to print-once + commit.
  - Prereq-ish for CONV-4 (a useful fr2433 diag/dev build wants debug output).

- [ ] **CONV-4 · fr2433 diag role** — *design (deferred)* · [GitHub #3](https://github.com/experientials/stem/issues/3)
  - FR2433 is single-I²C **slave-only** — diag's sensor-scan POST doesn't apply. Design a diag role
    (exercise the slave/expander + GPIO) when prod's fr2433 node duties exist (today it's a 110 B
    idle scaffold).

---

## Verification gaps — build-verified, not yet run on hardware

- [ ] **VER-1 · diag on FR2476 (post-refactor)** — build-verified only; run single-adapter on an
  FR2476 to confirm (same shared code, UCA0 board module).
- [ ] **VER-2 · prod on hardware (any family)** — never run (scaffold: boots/watchdog/idle). Run when
  node duties are implemented. · [GitHub #4](https://github.com/experientials/stem/issues/4)
- [x] **diag on FR2355 (post-refactor)** — DONE 2026-09-30 (build hd2355): CONFIRMED RUNNING + POST;
  register values match source exactly. See `diag/proofs/console-fr2355.log`.

---

## stem-probe (RP2040 bench host) — sniffer bring-up (2026-10-04)

- [ ] **PROBE-1 · measure the Pico console-logging transfer speed** — *medium · perf/observability* —
  Henrik 2026-10-04: measure the end-to-end throughput of how we LOG the Pico's console output (the
  `sniff`/`taps` decoded frames over USB-CDC → `thepia hwd console`). Motivation: during sniffer
  bring-up the decoded-frame output felt slow, and a slow/bottlenecked log path risks masking or
  truncating results. Measure: (a) firmware-side CDC write throughput (bytes/s the
  `write_packet` loop sustains in 64-byte chunks), (b) `hwd console tail`/`interact` readback rate,
  and (c) whether either drops bytes under a full 512-event dump. Decide if we need batched writes /
  a bigger CDC buffer / a terser frame format. NOTE: already confirmed NOT the cause of the truncated
  FR2355 captures (the firmware's own `sniff:` summary counts the decode, independent of the log path).
  - Evidence to date: `i2c_sniff` is a self-contained portable core (`debug/stem-probe/src/i2c_sniff.rs`);
    `sniff <sda> <scl> [ms]` + `taps` added to stem-probe. See the FR2355 LESSONS for the capture-timing
    gotchas that surfaced this.

## External — thepia `hwd` (report upstream, not fixed here)

- [ ] **EXT-1 · re-file two thepia reports** — `thepia hwd report` was broken (`agent not embedded /
  --features goose`); the *flash-unguarded-under-multi-adapter* and *console-non-UTF-8* reports never
  filed. Retry when the report channel works. (The per-board routing + cache-poison report DID file.)
- [x] **EXT-3 · thepia console session pins a stale device node (console-side C32)** — *reported + FIXED
  2026-09-30*. thepia now opens/reads the FR2433 console session (`console-2FAFB46F29002500.log` exists;
  `console tail --board 2FAFB46F29002500` reads the live looping banner cleanly — em-dash renders, so the
  old non-UTF-8 `tail` crash is handled too). The FR2433 spike is now verified the sanctioned way.

- [ ] **EXT-4 · thepia console should annotate host EVENTS (flash/reset/session/re-enum)** — *proposal
  filed 2026-09-30*. Timestamped `[hwd] …` marker lines interleaved with the firmware UART, raw stream
  kept separately retrievable (so verify-stamp/parsers aren't broken). Motivated by stale-vs-fresh log
  ambiguity, opaque verify-stamp failures, reset-loop diagnosis, and once-only-banner timing.

- [ ] **EXT-2 · `hwd.toml [msp] family = "fr247x"` stale for FR2355 work** — *thepia-gated, not trivial* —
  the cross-family guard didn't refuse an FR2355 flash (FR2355 = "unknown-family" to thepia). thepia
  only knows families `fr247x`/`fr24xx` — there is **no `fr235x`/`fr215x`**, so there's no correct value
  to set. Fix = either drop the guard (lose it) or wait for thepia to add the dual-family designations.
