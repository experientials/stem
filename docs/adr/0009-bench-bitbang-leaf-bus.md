# ADR-0009 · The bench host (stem-probe) owns the Leaf bus as bit-banged open-drain GPIO

Status: Accepted
Date: 2026-10-03   Source: OPEN-QUESTIONS Q-SENSING-ENUM (bench fix B)

## Context
The RP2040 `stem-probe` is the SoM/M7 stand-in. On the bench the Leaf (sensor) bus is shared: the MSP
masters it in Sensing, the Pico masters it while playing the Passive-mode owner. A permanently-enabled
hardware I²C **controller** on the Pico (embassy `I2c` on I2C0) is a *second controller* on that shared
bus — it violates the one-master invariant and was suspected of faulting the MSP's Sensing scan. embassy
ownership also makes it awkward to toggle a hardware controller on/off the pins while still providing the
bus pull-ups the bench needs (the bench has **no external Leaf pull-ups** — MCU-BENCH.md).

## Decision
The Pico owns GP4/GP5 as long-lived **`Flex`** pins, **not** a hardware I²C controller:
- **Parked** by default — Hi-Z inputs with pull-ups (the bench's only Leaf pull-ups; Pico is off as a
  driver but keeps the bus idling high for whoever masters it).
- **Driven** as **bit-banged open-drain I²C** only inside `leaf_scan` (the Passive-owner window), then
  immediately re-parked. SDA is true open-drain (release→pull-up high, 0→drive low); SCL push-pull (the
  Pico is the sole clock during its own scan).

So the Pico is never a second *controller*; one master at a time holds on the Leaf bus.

## Consequences
- Upholds the one-master invariant structurally; the Stem bus stays hardware I²C (I2C1) — unchanged.
- The Pico's Passive-owner scan works (finds the device via bit-bang).
- **Did NOT fix** the FR2355 Sensing-scan fault — proving the fault was never the Pico-as-controller (the
  fault persists with the Pico parked, and on a clean first scan after reset). Root cause is Leaf-side
  wiring/pull-up topology (Q-SENSING-ENUM) — a physical issue, out of firmware scope.
- Bit-bang is slow (~tens of kHz) — fine for a presence scan; not for high-rate sensor reads (the Pico
  never needs those; it's a bench stand-in).
