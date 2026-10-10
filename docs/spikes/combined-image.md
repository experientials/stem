# Spike: one firmware image for FR2476 + FR2355/FR2155 (combined / "dynamic" PAC)

**Status:** ✅ **SUCCESS — HW-PROVEN 2026-10-10.** A single MSP430 binary, flashed unchanged, runs the
eUSCI_B1 I²C slave and wakes from LPM on **both** FR2476 and FR2355. Spike crate: `spike/combined-mech/`
(excluded from the workspace). Asked by Henrik: *"can a PAC be made that supports both models… a combined
dynamic PAC that can be treated as a 3rd target."* Answer: **yes.**

## Question
STEM-DIRECTION §489 classed FR2476 (fr247x) and FR2355 (fr235x) as different families → separate builds.
Henrik's push: generate a **combined PAC** so one image covers both. Is it feasible?

## What differs (from the generated PACs — verified, not recalled)
Among the peripherals this firmware uses, only **eUSCI_B1** differs, in two ways:
- **Register base:** `0x0580` (FR2476) vs `0x05C0` (FR2355) — but the `e_usci_b1::RegisterBlock` layout is
  **byte-identical**. (eUSCI_B0, ports, Cs/Frctl/Pmm/Sys/WdtA all match; eUSCI_A1/Tb0 differ but are unused.)
- **IRQ vector address:** `0xFFDA` (FR2476, `__INTERRUPTS` idx 45 @ 0xFF80) vs `0xFFDE` (FR2355, idx 47).
Plus Stem pins (P3.2/3.6 vs P4.6/4.7) and RAM (8 KB vs 4 KB) — both runtime/intersection-handled.

## Technique (proven)
1. **`B1X`/`B1Y`** — expose eUSCI_B1 at both bases (same RegisterBlock); pick the base pointer at runtime
   by `bsp::model::detect()`. Typed access preserved.
2. **Custom dual-slot vector table** — PAC `rt` OFF; own `#[used] static __INTERRUPTS` (VECTORS 0xFF80/0x80)
   with the SAME EUSCI_B1 handler at BOTH idx 45 (0xFFDA) and idx 47 (0xFFDE); reset @ 0xFFFE; all other
   slots a default trap. No conflict because the firmware uses only eUSCI_B1.
3. **Runtime pins + 4 KB RAM floor.**

## Gotchas (found building it)
- The generated PAC's `Vector` union has **private fields** → not constructible from another crate. The
  real combined PAC must **emit its own `__INTERRUPTS`**; the spike defines its own identical `union Vector`.
- `msp430-rt` with `device` OFF emits a fallback `__INTERRUPTS` that **collides** → use `device` ON + an
  **empty `device.x`** from build.rs.

## Evidence (bench, 2026-10-10)
One binary `spike/combined-mech/…/combined-mech-spike` (text 424 B) flashed to @fr2476 and @fr2355:
```
scan: 0x20 0x21                 # both chips up, at their runtime-selected role addrs
0x21[0x00] = 0x5A   FR2476 — woke via 0xFFDA slot, base 0x0580, pins P3
0x20[0x00] = 0x5A   FR2355 — woke via 0xFFDE slot, base 0x05C0, pins P4
```
(Console note: FR2355 wakes slightly slower; a short `console interact --expect` clipped the read →
false-negative. `console send`+`tail`+generous wait gave a clean `0x5A`. See [[prove-firmware-on-hardware]].)

## Productized — the combined image is now a REAL prod target (S2b, HW-PROVEN 2026-10-10)
The spike's mechanism is now the shipped path, not a side experiment:
- **S2b.1** — combined PAC `pac/msp430fr2xxx-dual` (FR2476 register copy + `EUsciB1Y`@0x05C0 overlay) emits
  its OWN dual-slot `__INTERRUPTS` (EUSCI_B1 at idx 45 **and** 47) + a real `device.x` (feature `rt`).
  Committed `b544fc0`; re-pointed spike flashed unchanged on both parts.
- **S2b.2** — `bsp` gained the `fr2xxx` chip feature: `pac = msp430fr2xxx_dual`, plus a `_dynb1`
  runtime-base `stem_i2c` path (overlay `e_usci_b1::RegisterBlock` on `model::stem_i2c_base()` →
  0x0580/0x05C0) and a combined board (`board/fr2xxx_combined.rs`) that runtime-dispatches the **three**
  real divergences prod touches: eUSCI_B1 base/pins, **CSCTL1 clock trim** (manual vs factory), and the
  console eUSCI_A (A0@0x0500 / A1@0x0580) — dev builds only. µs/ms timers are prod-unused → the TA/TB
  instance divergence never arises. Per-family builds unchanged.
- **S2b.3 (partial)** — `prod` gained a `fr2xxx` family feature + `memory-fr2xxx.x` (4 KB RAM intersection,
  0xFF80 superset vectors). Builds a real artifact `prod-fr2xxx-silent-cros_ec` (FRAM 6836 B / 20 %, fits
  the 4 KB floor). Latent bug fixed en route: `deepsleep::enumerate` gated the `Console` import but not its
  (unconditional) bound → every console-OFF build was broken; import is now unconditional.
- **S2b.4 — HW proof (bench, 2026-10-10).** The SAME byte-verified `prod-fr2xxx-silent-cros_ec` flashed to
  @fr2476 and @fr2355; both serve their Stem slave at the chip-correct role addr:
  ```
  scan: 0x20 0x21 0x22            # 0x21=combined on FR2476, 0x20=combined on FR2355, 0x22=fr2433 control
  0x21[0x00] = 0xEC   FR2476 — base 0x0580, pins P3, addr 0x21, manual trim
  0x20[0x00] = 0xEC   FR2355 — base 0x05C0, pins P4, addr 0x20, factory trim
  ```
  Each node ACKing + servicing a read at its chip-correct address proves every runtime selection fired
  (clock trim → eUSCI clock; base; pin mux; own-addr). objdump of the real artifact confirms EUSCI_B1 ISR
  @0x8030 at **both** 0xFFDA and 0xFFDE; DefaultHandler elsewhere; reset @0x8000.

## Limits / not yet
- The proof exercises the **slave bring-up + read servicing** on both chips (the thing S2b adds). A full
  **cros_ec framed HELLO** exchange was NOT driven — the RP2040 probe's `r`/`w` are single-register, not a
  cros_ec frame; `0xEC` is the slave's byte to a raw read, not a decoded cros_ec response. The cros_ec
  framer itself is the same `deepsleep` path the per-family cros_ec images already run.
- **Console on the combined image** is implemented (runtime eUSCI_A overlay + P1/P4 pins) and compiles, but
  is NOT yet HW-exercised on both chips; the dark prod artifact builds console-OFF. `deepsleep`'s per-chip
  `LOOPBACK_JUMPER_HINT` is still compile-time (console+chip-marker) — a console-combined build would need a
  runtime hint. Deferred (S2b.2b) — dark prod doesn't need it.
- Remaining S2b.3: fold `prod` → `modules/deepsleep` and make the combined image a first-class `just build
  fr2xxx` route (the budget/feature plumbing exists; the top-level `build` recipe case is still TODO).
