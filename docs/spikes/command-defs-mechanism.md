# Spike: command-definition mechanism (codegen enum + parser + help + catalog from ONE source)

**Status:** ✅ DONE 2026-10-09 — **recommend A (declarative `commands!` macro).** Simple spike per Henrik
("don't gold plate"). Decider: Henrik.

## Question
Help/introspection MUST be generated from the command defs (single source → no drift, because `thepia hwd
chat` acts on `help` to drive hardware; COMMANDS.md). What mechanism defines a command ONCE such that we
generate (a) the `Command` enum arm, (b) the console parser (verb→Command), (c) help/introspection text,
(d) the off-device catalog (`--dark`/MCP)? Applies to both the node vocabulary (`stembus::command`) and the
probe CLI (`pico-probe`).

## Options
- **A. Declarative `commands!` macro** (`macro_rules!`) — one invocation → enum + const meta table + parser.
- **B. Const table + exhaustiveness test** — hand enum + `&[CmdMeta]`; a test catches drift.
- **C. Proc-macro derive** — `#[derive(Command)]` + attrs.

## What the spike found (host prototype: scratchpad/cmd_spike.rs, `rustc -O`, runs green)
Prototyped **A** with 4 commands incl. an arg (`FramCrc{sector}`, `SetMode{mode}`). ONE `commands!{…}` block
expands to: `enum Command`, `const CATALOG: &[CmdMeta]` (verb/help/args — the help+catalog source), and
`parse(line)->Option<Command>`. Verified round-trips (`crc 3`→`FramCrc{3}`, `m`→None on missing arg) and
that the same list drives help, parse, and catalog.

Key findings (the differentiators — not size/feasibility, which are equivalent):
- **Drift-safety:** A = structural (one source → all artifacts; adding a command is one line). B = test-only.
  C = structural. → A and C tie; B weaker.
- **no_std / msp430 / thumbv6m:** A = native `macro_rules!`, always works, zero deps. C needs a proc-macro
  crate (syn/quote) — works (defmt precedent) but adds build cost. B = plain data, trivial.
- **Code size:** equivalent across A/B/C (all emit a match + a const table; proc-macro cost is compile-time
  only). Non-differentiator — not worth an MSP430 size matrix.
- **Build complexity:** A/B = none; C = a proc-macro crate + syn/quote in the build graph.
- **Ergonomics now vs later:** A covers today's needs (terse verb, help, positional typed args) with no
  dependency. C only pulls ahead if command metadata gets rich (named args, validation, per-arg help,
  derive-driven catalog schemas) — not the case yet.

## Recommendation
**A — declarative `commands!` macro.** Structural no-drift, zero deps, no_std/MSP430-clean, covers both
surfaces, and the `CATALOG` const is directly the help text AND the `--dark` catalog source. **Escalate to
C (proc-macro)** only if/when command metadata outgrows what `macro_rules!` expresses cleanly (rich named
args / validation); **B** is the fallback if the macro ever feels too clever. Prototype: `scratchpad/
cmd_spike.rs` (throwaway; the real impl lands in `stembus::command` during the command-system work).
