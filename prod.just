# `just prod <verb>` — build/flash the PRODUCT Stembus node firmware (prod/, Rust; family axis).
#
#   just prod build [family]   build for a family + GATE against its part budget (default fr247x)
#   just prod build fr24xx     build the FR2433 single-I²C node instead
#   just prod flash            program the connected board (host; needs msp430-macos-dev setup)
#
# TWO production families are first-class (see DESIGN.md / MCU_SELECTION.md): fr247x (FR2476/FR2475
# dual-I²C — the on-hand dev boards, DEFAULT) and fr24xx (FR2433 single-I²C slave-only). The FR2155
# production dual-I²C part joins as a future fr215x family. `build` fails if the footprint exceeds the
# family budget (scripts/size-check.sh). Same build-path selection as `just diag build`: native if set
# up, else the Docker toolchain image, native in-container on CI.

image    := env_var_or_default("MSP430_IMAGE", "msp430-c-rust:local")
mspdebug := env_var_or_default("MSPDEBUG_BIN", "/usr/local/bin/mspdebug")
libdir   := env_var_or_default("MSP430_LIBDIR", "/usr/local/lib")
cargo_cache := "-v msp430-cargo-registry:/opt/cargo/registry -v msp430-cargo-git:/opt/cargo/git"

# Build prod for a FAMILY and gate its footprint against that family's part budget.
#   family: fr247x (default; FR2476/FR2475 dual-I²C, 32 KB window / 8 KB / 1 KB floor)
#           fr24xx (FR2433 single-I²C slave-only, 15 KB / 4 KB / 512 B floor)
#   mode:   auto (default) | native | fast | docker | docker-fast
#   console: "console" (default; dev UART logging) | "" for a SILENT production image (smaller FRAM)
# `-e PROD_BUILD` forwards a pinned stamp into the container; native path picks it up from the env.
build family="fr247x" mode="auto" console="console" modes="" extra="":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{family}}" in
        fr247x)        FRAM_BUDGET=32768; RAM_BUDGET=8192; FLOOR=1024;;  # FR2476 dev (8 KB SRAM)
        fr215x|fr235x) FRAM_BUDGET=32768; RAM_BUDGET=4096; FLOOR=1024;;  # FR2155/FR2355 (4 KB SRAM)
        fr24xx)        FRAM_BUDGET=15360; RAM_BUDGET=4096; FLOOR=512;;   # FR2433 single-I²C
        *) echo "unknown family '{{family}}' — use fr247x|fr215x|fr235x|fr24xx" >&2; exit 1;;
    esac
    # Operating modes to compile in (DESIGN.md). Default per family: dual-I²C gets both (runtime-
    # switchable); fr24xx is Passive-only (sensing needs a 2nd I²C). Override with the `modes` arg,
    # e.g. `just prod build fr247x auto console mode-sensing` for a sensing-only image.
    if [ -n "{{modes}}" ]; then MODES="{{modes}}"; else
        case "{{family}}" in fr247x|fr215x|fr235x) MODES="mode-passive,mode-sensing";; fr24xx) MODES="mode-passive";; esac
    fi
    # `console` arg: "console" (lean UART log) | "" (silent) | "debug" (verbose: post-POST found-info).
    # The feature name IS the arg value (`debug` implies `console` in Cargo.toml), so it flows into FEATS.
    FEATS="{{family}},$MODES"; [ -n "{{console}}" ] && FEATS="$FEATS,{{console}}"
    [ -n "{{extra}}" ] && FEATS="$FEATS,{{extra}}"  # extra features, e.g. cros_ec (COMMANDS.md §0 milestone)
    feats="--no-default-features --features $FEATS"
    label="{{family}}$([ -n "{{console}}" ] && echo "+{{console}}" || echo ' silent')$([ -n "{{extra}}" ] && echo "+{{extra}}")"
    # VARIANT = the family (+ '-silent' when console is off). cargo always writes the same binary name
    # (`prod`) regardless of features, so both families — and console/silent — clobber one path. We copy
    # the built ELF to a variant-unique name `prod-<variant>` so a pinned artifact (hwd.toml image, CI
    # upload, `just prod flash`) CANNOT be silently overwritten by a later build of the other family.
    # This is what makes thepia's declaration-based cross-family guard reliable in practice (the guard
    # trusts a declared family; a stable per-family artifact keeps declaration and bytes from drifting).
    # VARIANT suffix per console arg: ''→'-silent', 'console'→'' (prod-<fam>), 'debug'→'-debug'. Keeps
    # the lean image (prod-<fam>) and the verbose one (prod-<fam>-debug) as distinct, non-clobbering artifacts.
    case "{{console}}" in
        "")      variant="{{family}}-silent" ;;
        console) variant="{{family}}" ;;
        *)       variant="{{family}}-{{console}}" ;;  # e.g. debug → prod-<fam>-debug
    esac
    [ -n "{{extra}}" ] && variant="$variant-{{extra}}"  # distinct artifact, e.g. prod-fr24xx-cros_ec
    finish() {  # $1 = cargo profile dir; copies to the variant artifact and gates THAT file
        local prof="$1" built art
        built="prod/target/msp430-none-elf/$prof/prod"
        art="prod/target/msp430-none-elf/$prof/prod-$variant"
        cp "$built" "$art"
        scripts/size-check.sh "$art" "$FRAM_BUDGET" "$RAM_BUDGET" "prod ($prof, $label) → $(basename "$art")" "$FLOOR"
        # BENCHMARK the fr247x image against the PRODUCTION dual-I²C parts (FR2155/FR2355): 32 KB FRAM /
        # 4 KB SRAM — the FR2476 dev board has 8 KB SRAM, HALF of which the shipped part won't have.
        # Informational (never fails the dev build) — confirms the image fits what we actually ship.
        # (fr235x and fr215x ARE compile targets now — both PACs exist and build; this fr247x check is
        # just the size benchmark against the 32K/4K production envelope.)
        if [ "{{family}}" = fr247x ]; then
            scripts/size-check.sh "$art" 32768 4096 "  ↳ benchmark vs FR2155/FR2355 (production 32K/4K)" 1024 || true
        fi
    }
    if [ -f /.dockerenv ]; then
        case "{{mode}}" in *fast) prof=fast; flag="--profile fast";; *) prof=release; flag="--release";; esac
        ( cd prod && cargo build $flag $feats )
        finish "$prof"
        exit 0
    fi
    run_docker() { docker run --rm --platform linux/amd64 -e PROD_BUILD {{cargo_cache}} -v "$PWD":/work -w /work {{image}} just prod build {{family}} "$1" "{{console}}" "{{modes}}" "{{extra}}"; }
    mode="{{mode}}"
    if [ "$mode" = auto ]; then
        if scripts/native-available.sh; then mode=native
        else mode=docker; echo "· native toolchain not found → Docker (set it up: scripts/setup-native-macos.sh)"; fi
    fi
    case "$mode" in
        native)      scripts/native-build.sh release prod "$feats"; finish release ;;
        fast)        scripts/native-build.sh fast prod "$feats";    finish fast ;;
        docker)      run_docker docker ;;
        docker-fast) run_docker docker-fast ;;
        *) echo "unknown mode '$mode' — use auto|native|fast|docker|docker-fast" >&2; exit 1 ;;
    esac

# flash prod's built ELF to the connected board (host; needs msp430-macos-dev setup).
# `family`: fr247x (default) | fr24xx — picks the variant-unique artifact `prod-<family>` so you flash
#           the family you mean, not whatever last clobbered the shared `prod` path.
# `console`: "console" (default) | "" — flashes the '-silent' artifact when empty.
# `profile`: "release" (default) or "fast".
# Make sure the connected board matches the family (`fr247x` → an FR2476; `fr24xx` → an FR2433).
# Cross-flashing is a mistake (different register map); boot's matches_build_family() flags it, and
# `thepia hwd msp flash` refuses it when the family is declared. Prefer `thepia hwd msp flash` (hwd.toml).
flash family="fr247x" profile="release" console="console":
    #!/usr/bin/env bash
    set -euo pipefail
    variant="{{family}}$([ -z "{{console}}" ] && echo '-silent' || true)"  # `|| true`: keep set -e happy when the test is false
    elf="prod/target/msp430-none-elf/{{profile}}/prod-$variant"
    [ -f "$elf" ] || { echo "no built ELF at $elf — run 'just prod build {{family}} {{profile}} {{console}}' first" >&2; exit 1; }
    echo "flashing $elf"
    if pgrep -x mspdebug >/dev/null 2>&1; then echo "  freeing stale mspdebug"; pkill -x mspdebug || true; sleep 1; fi
    DYLD_FALLBACK_LIBRARY_PATH={{libdir}} {{mspdebug}} tilib "prog $elf" exit
