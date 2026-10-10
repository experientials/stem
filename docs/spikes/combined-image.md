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

## Limits / next
- Spike proves the **mechanism** (dual-slot vectors + runtime base/pins/addr + LPM-wake), NOT the full
  cros_ec stack. The real combined target wires this into `bsp` (runtime eUSCI_B1 base) + a **combined PAC
  that emits the dual-slot `__INTERRUPTS` + a real `device.x`** + the cros_ec serve loop.
- So `modules/deepsleep` can be a **single FR2355+FR2476+FR2155 image** (the "3rd target"), not only
  per-family builds. This supersedes the "cross-family ⇒ separate build" reading for this specific pair.
