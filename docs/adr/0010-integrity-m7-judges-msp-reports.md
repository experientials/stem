# ADR-0010 · Firmware integrity: the MSP reports per-sector CRCs over I²C; the M7 judges (fail-safe)

Status: Accepted
Date: 2026-10-04   Source: Henrik 2026-10-04 (verbatim below); refines DESIGN.md console/checksum log + OPEN-QUESTIONS Q-POST

## Context
Prod POST computes per-sector FRAM checksums (DESIGN.md). Henrik, verbatim:

> Main problem is the code that verifies checksums cannot itself be buggy. Reporting could be UART or
> I2C. I suppose I2C would be a great option since it's easy for the M7 to read as needed.

The hazard: a self-checking firmware whose **verifier lives in the same FRAM it checks** cannot be
trusted to declare itself healthy — a bug or the very corruption it's meant to catch could defeat it
and yield a false "OK." A false "OK" is the dangerous outcome (a corrupt node believed good).

## Decision
Split the roles so the verifier's correctness is **not safety-critical**:
- **The MSP is a dumb reporter.** POST computes a simple, well-tested **per-sector CRC16** over the
  program-FRAM window (region from `memory-*.x` via linker symbols) and **exposes the raw CRCs in the
  Stem I²C debug-register window**. The MSP does **not** judge pass/fail.
- **The M7 is the judge** (the root of trust — [[msp-fw-ota-host-root-of-trust]]): it holds the golden
  per-sector CRCs for the image it flashed, **reads the MSP's reported CRCs over I²C as needed**, compares,
  and **reflashes any mismatched sector** (over BSL/STEM-MSG).
- **Reporting channel = I²C** (Stem debug registers), so the M7 reads on demand; UART logging of the CRCs
  stays a **dev-only echo** (console/--debug), never the channel integrity depends on.

## Consequences
- **Fails safe.** A buggy or corrupt MSP verifier can only produce a CRC that *mismatches* the M7's
  golden value → a false "corrupt" → an unnecessary (harmless) reflash. It can **never** produce a false
  "OK," because the MSP never asserts OK — the comparison happens on the M7 against values the MSP can't
  influence. Verifier correctness affects *efficiency* (spurious reflashes), not *safety*.
- A dangerous false-OK would require corruption that preserves all per-sector CRC16s simultaneously —
  negligible; and the M7 can escalate to a **BSL read-back + external CRC** (verifier entirely outside
  the MSP) for a deep audit when it wants certainty.
- POST must compute the CRCs **in the always-run path** (even the silent production image), not only in
  the console/--debug path — otherwise the shipped firmware exposes nothing for the M7 to read. (Current
  code computes them only under `console` — a gap to close; see Q-POST.)
- Needs new Stem registers for the per-sector CRCs (extend regmap.rs / I2C-API.md; Q-ADDR window).
