# FR2355 — lessons learned

Per-hardware-target knowledge file (one `LESSONS.md` per `pac/<chip>/`). Durable, hard-won facts about
the **MSP430FR2355** target — chip, PAC, bench wiring, and open investigations — so they aren't re-learned
from scratch each session. Keep newest context at the top of each item. Mark each fact **CONFIRMED**
(observed against a real source/hardware) or **OPEN/PRELIMINARY** (not yet proven) — never blur the two.

> Convention: this file sits next to the chip's vendored PAC because `pac/<chip>/` is the natural
> per-target home. Lessons here span the whole target (silicon, clock, bus roles, bench), not just the
> generated register API. See `msp430-macos-dev` skill → "Maintain a LESSONS.md per hardware target."

---

## PAC / register API

- **CONFIRMED — a PAC describes registers at addresses, NOT pins.** `pac/msp430fr2355` contains the
  `e_usci_b0` register block (0x0540) and the `p1` port block (0x0200) as two *independent* objects.
  There is **no** signal→pin mapping anywhere in an SVD/svd2rust PAC (grep: no `UCB0SDA` constant exists,
  and can't). "The PAC is generated from TI source, therefore the pins are right" is a category error:
  pin-mux facts live in the **datasheet's pin-multiplexing tables**, and the firmware hard-codes them
  (`SDA_BIT`/`SEL` in `crates/bsp/src/i2c.rs`, `crates/bsp/src/board/mod.rs`). A wrong pin is a *firmware*
  bug, never a PAC bug.
- **CONFIRMED — FR2355 is genuinely FR2355, not a copy of FR2476.** They differ in ways a rename can't
  produce: Timer_B (`Tb0–Tb3`) vs Timer_A; different eUSCI address map (FR2355 A0/B0/A1/B1 =
  0x0500/0540/0580/05C0 vs FR2476's 0x0500/0540/0520/0580); FR2355-only `Captio`/`EComp1`/`Icc`/`Sac0–3`.
  Verified all peripheral base addresses against the TI datasheet (`datasheets/msp430fr2355.pdf`, the
  memory-map table on its p84 + per-peripheral sections).
- **CONFIRMED — the PAC follows TI's *device files*, which are a superset of the datasheet.** `Captio`
  @0x02EE appears in the PAC but the FR2355 datasheet never mentions CapTIvate/0x02EE — it comes from
  TI's device header (`msp430fr2155.h`/`...2355.h` via `pftbest/msp430_svd`). So the PAC legitimately
  contains a few registers the datasheet omits. To *prove* a vendored PAC is untampered: regenerate and
  `git diff` → zero (the pipeline is deterministic from `(SVD, svd2rust v0.37.1)`). `scripts/gen-pac.sh`
  now defaults to all four devices incl. fr2155/fr2355 (previously only fr2433/fr2476).

## Pins & bus roles (datasheet-confirmed)

- **CONFIRMED — Leaf-I²C = UCB0 on P1.2(SDA)/P1.3(SCL).** Datasheet `slau680`/device ds: `P1.2 =
  UCB0SIMO/UCB0SDA`, `P1.3 = UCB0SOMI/UCB0SCL`, selected at `SEL=01` (SEL1=0,SEL0=1). On the
  MSP-EXP430FR2355 LaunchPad these are BoosterPack **J1 pin 10 (SDA) / J1 pin 9 (SCL)** (Figure 10).
- **CONFIRMED — Stem-I²C = UCB1 on P4.6(SDA)/P4.7(SCL)** for FR23xx/FR21xx (`_stem_p4`), vs P3.2/P3.6 on
  FR247x. The UCB1↔pin map is NOT uniform across the family — see `prod/src/stem.rs`. Role: the MSP is
  always the Stem **slave** (SoM/RP2040 masters it); it configures UCB1 with an own-address (`UCOAEN`),
  no `UCMST`. That's why "UCB1 can't master" — it's deliberately a slave, not a defect; eUSCI_B0 and
  eUSCI_B1 are identical IP and either *could* master.
- **CONFIRMED — the LaunchPad's onboard light sensor is NOT on the Leaf pins.** The photodiode +
  transimpedance amp (D1, R3=2.37 MΩ, C6) is on **P3.1/P3.2/P3.3 (SAC2 / OA2)** via jumpers J8/J7/J9
  (populated by default); LEDs on P1.0/P6.6. So it doesn't load P1.2/P1.3. **Caveat:** J7 ties the
  photodiode's OA2− to **P3.2** — the FR247x Stem-SDA pin — so if you ever run the Stem bus on P3.2 on
  this board, pull J7 first.

## Clock — the FR2355 gotcha

- **CONFIRMED — use the FACTORY DCO trim, not FR2476's manual trim.** `board/fr2355_launchpad.rs`
  selects only the 1 MHz DCORSEL range; the FR2476 manual `DCOFTRIMEN`+`DCOFTRIM` values do **not** lock
  the FR2355 FLL (the lock loop spins forever). The lock wait is **bounded** (bails after ~20000 spins)
  so a never-locking FLL can't deadlock boot. On the bench the FLL reads **LOCKED** (`CSCTL7=0C57`,
  `CSCTL7 & 0x0300 == 0`), and legible 9600-baud UART confirms SMCLK ≈ 1 MHz.

## Bench & tooling gotchas (hwd / Pico sniffer)

- **CONFIRMED — the FR2355 LaunchPad has NO control-line reset.** `thepia hwd reset --board @fr2355`
  fails ("declares no control-line reset"); DTR/RTS isn't wired to RST. Reliable reset = re-flash over
  SBW (`thepia hwd msp flash …`) or mspdebug. The boot banner + regs dump + bus scan print **once** at
  boot (POST is not re-run every cycle), so to observe them you must reset *while watching*.
- **CONFIRMED — the diag POST bus scan is UART-throttled → SLOW (seconds).** Each probe's address is
  printed over the 9600-baud backchannel between I²C transactions, so the full 0x08–0x77 scan takes
  *seconds*, with each ~100 µs I²C probe separated by a long UART gap. This wrecks naive sniffer timing
  (a fixed short capture window only catches the first ~15 low addresses). Capture must span the whole
  slow scan.
- **CONFIRMED — "Leaf I²C on the Pico" = GP4(SDA)/GP5(SCL)**, per `stem-probe`'s canonical map
  (`debug/stem-probe/src/main.rs`). (Stem = GP18/19.) When tapping the FR2355 Leaf to the Pico, wire
  P1.2→GP4, P1.3→GP5, **and a common GND** — missing common GND makes every Pico input read a static
  level (the `taps` probe shows all-`L`). This cost a full debug round.
- **CONFIRMED — a passive I²C sniffer now exists in stem-probe.** Reusable portable core
  `debug/stem-probe/src/i2c_sniff.rs` (HAL-agnostic: pin reads via closures, no embassy/I/O — drops into
  any M0/M4/M7). Console commands: `sniff <sda> <scl> [ms]` (passive decode; default GP4/GP5) and `taps`
  (pull-down connection map across the free GPIOs → finds where SDA/SCL landed + proves common GND).
  See backlog **PROBE-1** (measure the console-logging transfer speed).

## OPEN — the Leaf-bus "false-ACK storm" investigation (2026-10-04, NOT resolved)

Symptom (CONFIRMED): the FR2355 diag POST reports a **false-ACK storm** on its Leaf bus — ~88/88
addresses 0x20–0x77 "present", inventory "7/7 present", yet **every register read FAILS** (OLED/IS31/
MC6470/Si7021 init+read all fail), while bus idles `SDA=H SCL=H`, FLL LOCKED, Vcc 3.3 V, UCB0 master
config `0x0F80` correct, LPM5 cleared. Also: the board **WDT-reset-loops** (see backlog BUG-3).

What the passive wire-sniff (GP4/GP5) showed — **PRELIMINARY, inconsistent**:
- Decoding works: real framing, proper NAK/ACK bits (e.g. 0x0A–0x1F → `+N` NAK, 0x20 → `+A` ACK). So at
  the wire the low addresses do **NOT** universally false-ACK — hinting the false-ACK may live in the
  **FR2355's own ACK-bit sampling**, not the bus electrical layer.
- BUT every capture stopped after ~13–17 probes (~up to 0x20); the decoded range didn't line up with the
  FR2355's reported 0x20–0x77 scan; and Pico-as-master `lscan` returned `0x09/0x48/0x49` (none of the
  known sensors). These inconsistencies are unexplained → **no confirmed root cause yet.**

Likely confounders to eliminate next:
1. **Bus contention** — the FR2355 isn't held off the Leaf bus during Pico `lscan`; post-boot it drives
   the bus for sensor tasks. Redo `lscan` with the FR2355 **held in reset** for a clean Pico-master read.
2. **Capture/scan timing** — catch a *full* slow scan (span the whole seconds-long POST scan), or trigger
   on a known, repeatable transaction rather than the one-shot boot scan.
3. **Pull-up / signal integrity** on this specific FR2355-LaunchPad + sensor-board combo (scope SCL rise
   time at 100 kHz; the sniffer can't measure analog rise time).

Hypotheses still on the table: (a) FR2355 firmware ACK-sampling/timing bug (wire is healthy); (b) a real
bus signal-integrity problem; (c) a device holding SDA. Decide with the clean held-in-reset Pico-master
scan + a scope, not more one-shot boot-scan sniffs.
