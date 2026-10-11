# Bus recovery & mastery — the topic

> Bus recovery is broader than un-wedging a stuck bus: it is about **who owns the bus and how
> ownership is held, lost, and handed over**, across increasingly complex scenarios. This doc is the
> anchor for that topic; the wedge work ([I2C-WEDGE-INVESTIGATION.md](I2C-WEDGE-INVESTIGATION.md)) is
> level 1 of it.

## CANONICAL (verbatim — Henrik, 2026-10-05)

Bus recovery is a topic

1) single MCUs state
2) Transfer of mastery when a SoM resumes or suspends
3) Multi-master scenarios with well defined transfer patterns known to both parties
4) True multi-master when they have the right hardware support

---

## Derivation (subordinate to the canonical block above)

The four levels are a ladder of increasing coordination. Each is a distinct design problem; the lower
ones must be solid before the higher ones are safe. Mapping to what exists today (current state, not
canon — if this disagrees with the block above, the block wins):

### 1) Single-MCU state
One master, one MCU owns the bus; recovery = returning *its own* controller + the lines to a known-good
idle. This is where the current work lives: SDA-low (slave mid-byte) and SCL-low (our own eUSCI
self-hold) wedge detection + recovery in `crates/bsp/src/i2c.rs`, both-line fault reporting in
`prod/src/enumerate.rs`, and the open cross-hardware trigger investigation. Status: fixes landed,
root electrical trigger still open. See [I2C-WEDGE-INVESTIGATION.md](I2C-WEDGE-INVESTIGATION.md).

### 2) Transfer of mastery when a SoM resumes or suspends
Not multi-master — **sequential** single-mastership that moves between parties as power state changes.
The SoM/M7 masters the Stem bus while awake; when it suspends, mastership passes to the supervisor /
PD-controller; on resume it takes it back. The hand-off points are where a bus is most likely to be
left mid-transaction → recovery must make ownership transitions clean (quiesce, release, re-acquire
from idle). Related today: `I2C-API.md` "Masters on the Stem bus" (SoM→PD-controller off-regime
hand-off), memory `mcus-independent-no-handshake`. Open: the explicit quiesce/handoff sequence.

### 3) Multi-master with well-defined transfer patterns known to both parties
More than one party *may* master, but they coordinate by a **shared, agreed protocol/schedule** so
they never actually contend — ownership is passed by convention (tokens, time windows, request/grant),
not by hardware arbitration. Recovery here includes detecting a party that failed to release and
re-establishing the agreed turn order. Relates to the `stembus` shared-contract crate (both sides
agree by construction). Open: define the transfer patterns.

### 4) True multi-master with the right hardware support
Multiple masters that may start transactions independently and rely on **hardware bus arbitration**
(collision/arbitration-lost detection) to resolve contention. Requires parts whose I²C supports it —
e.g. the MSP430 eUSCI_B exposes arbitration-lost (`UCALIFG`) and multi-master operation. Recovery
includes arbitration-loss handling, retry/backoff, and still handling a genuinely wedged line.
Open: whether/where the product actually needs true multi-master vs. levels 2–3 sufficing.

## Why the ladder matters

- Levels 2–4 **assume level 1 works**: you cannot safely hand off or arbitrate a bus you cannot first
  return to a known-good idle. The wedge fixes are the foundation.
- The observation that production/per-board faults are expected (cheap boards) means recovery must be
  **defensive at every level** — a faulty or marginal party must be detected and not allowed to wedge
  the shared bus for the others.
- Each level needs its own **test strategy**: level 1 → the signature battery + wedge injection; level
  2 → suspend/resume hand-off exercised with the SoM stand-in (RP2040 `stem-probe`); levels 3–4 →
  two-master bench rigs with the agreed protocol / arbitration enabled.

## Status

Living topic. Level 1 is in active work (fixes landed, trigger open). Levels 2–4 are captured here as
the framing to design against; none has an implemented transfer/arbitration mechanism yet. Append
design and bench results per level as they develop.
</content>
