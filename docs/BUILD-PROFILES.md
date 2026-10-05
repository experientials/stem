# Build profiles, logging & debug — the axes (and how to invoke them)

> How firmware images are built, and what the `just build` flags actually control. Written because
> `--debug` is easy to misread as a cargo *profile* when today it is a logging *feature*. Companion:
> [TOOLCHAIN.md](../TOOLCHAIN.md) (toolchain install / Docker vs native).

## There are four orthogonal axes — don't conflate them

| Axis | Values | What it changes | Selected by |
|---|---|---|---|
| **Which firmware** | `prod` · `diag` · `probe` | the image/application | `--diag` (else prod); rp2040 → probe |
| **Log verbosity** (prod) | silent · `console` (lean) · `debug` (verbose) | UART output + interactive cmds | the `console`/`debug` **cargo feature** |
| **Build profile** | `release` (LTO, ships, gated) · `fast` (no-LTO, iterate) | optimization + link | the `mode` arg (`auto\|native\|fast\|docker`) |
| **Debug symbols** | on (both, since 2026-10-05) | DWARF `.debug_*` in the ELF | `[profile.*] debug = true` |

Key facts:
- **MSP430 firmware is always built optimized** (`opt-level = "s"`). There is no unoptimized/`dev`
  profile — `opt-level = 0` would not fit the 32 KB / 15 KB FRAM windows and isn't needed.
- `release` = `opt-level="s"`, `lto=true`; `fast` = inherits release, `lto=false` (LTO is the dominant
  cost under Rosetta, so `fast` is the quick edit→flash loop; larger, not gated).
- **Symbols are free on-device.** `size-check.sh` measures `msp430-elf-size` *loadable* text/data/bss;
  DWARF lands in non-loadable sections. Both `[profile.release]` now set `debug = true` — zero change
  to the FRAM/SRAM budget (verified: prod 4980 B, diag 23382 B before and after), only a larger host
  ELF (prod: ~8.6 KB loadable → ~1.6 MB on disk). Gains `addr2line` on panic PCs, backtraces, and the
  prerequisite for gdb-over-SBW source debugging.

## ⚠ `--debug` today is a LOGGING feature, not a debug build

`just build <chip> --debug` → `prod build <fam> auto debug` → the cargo feature `debug = ["console"]`
= verbose post-POST "what was found" + interactive `s/r/d/m/l` commands, built with the **normal
release profile**. It does **not** mean unoptimized or (previously) symbol'd. This is the main
footgun; the proposal below fixes the naming.

## Current invocations

```
just build fr2355            # prod, release, lean console  (ships-like dev default)
just build fr2355 --debug    # prod, release, VERBOSE logging + interactive cmds   (feature, not profile)
just build fr2355 --diag     # diag POST image, release
just prod build fr235x auto debug mode-sensing   # verbose, sensing-only, prod
just diag build fr2355 fast  # diag, fast profile (no-LTO) — quick iteration
```

## Proposed scheme (not yet implemented) — `--release` / `--log` / `--debug`

Decompose the conflated "log vs profile" axis so each flag means what it says:

| Flag | Profile | Symbols | Logging | Gated | Use |
|---|---|---|---|---|---|
| `--release` / default | release (LTO) | yes (free) | lean/silent | yes | the shipping image |
| `--log` | release (LTO) | yes | **verbose** + cmds | yes | logs on a representative optimized image (= today's `--debug`) |
| `--debug` | `fast` (no-LTO) | yes | verbose + cmds | no | **gdb-over-SBW stepping / backtraces** (the capability we lack) |
| `--diag` | (orthogonal) | — | — | — | the POST image; composes: `--diag --debug` |

Implementation when adopted: rename the `debug` cargo feature → `log`; add `debug = true` to `fast`
(already on `release`); map `--debug` → `fast` profile + `log` feature. Keep `opt-level = "s"` even for
`--debug` ("debug" here = symbols + no-LTO, **not** unoptimized). **Gated on first needing SBW source
debugging** — until then the free `debug = true` on release (done) delivers ~90% of the value
(backtraces/addr2line) at zero cost, so the rename is deferred, not urgent.

## Faster matrix builds — `cargo-batch` (under evaluation)

See the timing spike + decision in this doc's companion notes / commit log. Summary: embassy-rs
`cargo-batch` runs many `cargo build`s in one invocation with a unified build graph, compiling shared
deps once — promising for CI / `build-all` across the full variant matrix (esp. no-LTO `check`/`fast`),
less so for single-image dev builds. Limits here: `prod`/`diag`/`probe` are separate projects (no root
workspace) so sharing is within-project only; the per-binary LTO links and Rosetta emulation bound the
win. It's a cargo *fork* (version-lag/maintenance cost). Decision pending a measured spike.
</content>
