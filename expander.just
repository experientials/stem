# `just expander <verb>` — build/flash the FR2433 I/O-EXPANDER firmware (modules/expander/, Rust).
#
#   just expander build            build + GATE against the FR2433 budget (15 KB / 4 KB / 512 B floor)
#   just expander build auto ""    build the SILENT image (no console UART)
#   just expander flash            program the connected FR2433 (host; needs msp430-macos-dev setup)
#
# This is the single-I²C slave-only node, extracted from prod/ (STEM-DIRECTION Repo-layout reorg S2a).
# cros_ec is the ONLY command interface (COMMANDS.md §0) — it is unconditional in the crate, so there is
# no feature to pass. Usually reached chip-first via `just build fr2433`; this module is direct access.
# Same build-path selection as `just prod build`: native if set up, else the Docker toolchain image,
# native in-container on CI (the recipe detects /.dockerenv, so local and CI run the exact same code).

image    := env_var_or_default("MSP430_IMAGE", "msp430-c-rust:local")
mspdebug := env_var_or_default("MSPDEBUG_BIN", "/usr/local/bin/mspdebug")
libdir   := env_var_or_default("MSP430_LIBDIR", "/usr/local/lib")
cargo_cache := "-v msp430-cargo-registry:/opt/cargo/registry -v msp430-cargo-git:/opt/cargo/git"

# Build the FR2433 expander and gate its footprint against the FR2433 part budget.
#   mode:    auto (default) | native | fast | docker | docker-fast
#   console: "console" (default; dev UART logging) | "" for a SILENT production image | "debug" (verbose)
# `-e PROD_BUILD` forwards a pinned stamp into the container; native path picks it up from the env.
build mode="auto" console="console":
    #!/usr/bin/env bash
    set -euo pipefail
    FRAM_BUDGET=15360; RAM_BUDGET=4096; FLOOR=512   # FR2433 single-I²C (15 KB program FRAM / 4 KB SRAM)
    # FR2433 is Passive-only; cros_ec is unconditional (no feature). The `console` arg feature name IS the
    # arg value ("console" lean | "" silent | "debug" verbose — `debug` implies `console` in Cargo.toml).
    FEATS="fr2433,mode-passive"; [ -n "{{console}}" ] && FEATS="$FEATS,{{console}}"
    feats="--no-default-features --features $FEATS"
    label="expander+cros_ec$([ -n "{{console}}" ] && echo "+{{console}}" || echo ' silent')"
    # VARIANT suffix per console arg: ''→'silent', 'console'→'cros_ec', 'debug'→'debug'. cargo always
    # writes the same binary name (`expander`); we copy the ELF to a variant-unique name so a pinned
    # artifact (hwd.toml image, CI upload, flash) CANNOT be silently overwritten by a later build.
    case "{{console}}" in
        "")      variant="silent" ;;
        console) variant="cros_ec" ;;
        *)       variant="{{console}}" ;;  # e.g. debug → expander-debug
    esac
    finish() {  # $1 = cargo profile dir; copies to the variant artifact and gates THAT file
        local prof="$1" built art
        built="modules/expander/target/msp430-none-elf/$prof/expander"
        art="modules/expander/target/msp430-none-elf/$prof/expander-$variant"
        cp "$built" "$art"
        scripts/size-check.sh "$art" "$FRAM_BUDGET" "$RAM_BUDGET" "expander ($prof, $label) → $(basename "$art")" "$FLOOR"
    }
    if [ -f /.dockerenv ]; then
        case "{{mode}}" in *fast) prof=fast; flag="--profile fast";; *) prof=release; flag="--release";; esac
        ( cd modules/expander && cargo build $flag $feats )
        finish "$prof"
        exit 0
    fi
    run_docker() { docker run --rm --platform linux/amd64 -e PROD_BUILD {{cargo_cache}} -v "$PWD":/work -w /work {{image}} just expander build "$1" "{{console}}"; }
    mode="{{mode}}"
    if [ "$mode" = auto ]; then
        if scripts/native-available.sh; then mode=native
        else mode=docker; echo "· native toolchain not found → Docker (set it up: scripts/setup-native-macos.sh)"; fi
    fi
    case "$mode" in
        native)      scripts/native-build.sh release modules/expander "$feats"; finish release ;;
        fast)        scripts/native-build.sh fast modules/expander "$feats";    finish fast ;;
        docker)      run_docker docker ;;
        docker-fast) run_docker docker-fast ;;
        *) echo "unknown mode '$mode' — use auto|native|fast|docker|docker-fast" >&2; exit 1 ;;
    esac

# flash the expander's built ELF to the connected FR2433 (host; needs msp430-macos-dev setup).
# `console`: "console" (default; flashes expander-cros_ec) | "" (expander-silent) | "debug".
# `profile`: "release" (default) or "fast". Make sure the connected board is an FR2433.
flash console="console" profile="release":
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{console}}" in "") variant="silent" ;; console) variant="cros_ec" ;; *) variant="{{console}}" ;; esac
    elf="modules/expander/target/msp430-none-elf/{{profile}}/expander-$variant"
    [ -f "$elf" ] || { echo "no built ELF at $elf — run 'just expander build {{profile}} {{console}}' first" >&2; exit 1; }
    echo "flashing $elf"
    if pgrep -x mspdebug >/dev/null 2>&1; then echo "  freeing stale mspdebug"; pkill -x mspdebug || true; sleep 1; fi
    DYLD_FALLBACK_LIBRARY_PATH={{libdir}} {{mspdebug}} tilib "prog $elf" exit
