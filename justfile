# stem command runner.
#
# Build recipes run inside the toolchain container. On your Mac they shell into
# Docker; in CI (already running inside the image) they build natively — the
# recipe detects /.dockerenv, so local and CI run the exact same code. No drift.
#
# Flash/probe run on the macOS HOST (USB) and need the one-time mspdebug setup
# from the msp430-macos-dev skill (x86_64 mspdebug + signed libmsp430.dylib).
#
#   just                       list recipes
#   just build <chip>          build a board's firmware (fr2355|fr2476|fr2433|fr2155|rp2040; --diag = POST)
#   just build-all             build every default firmware image
#   just msp-id                read the connected MSP430 device id (no flash)
#   just check deps            verify the toolchain
#   just example build <name>  build one example (C make / Rust cargo, auto-detected)
#   just examples              build every example (smoke test)

# Default to the locally-built image. Only the HOST uses this (CI builds inside the
# image, so it never pulls). Once toolchain-image.yml pushes to GHCR, you can set
# MSP430_IMAGE=ghcr.io/experientials/msp430-toolchain:latest to skip `just bootstrap`.
image    := env_var_or_default("MSP430_IMAGE", "msp430-c-rust:local")
mspdebug := env_var_or_default("MSPDEBUG_BIN", "/usr/local/bin/mspdebug")
libdir   := env_var_or_default("MSP430_LIBDIR", "/usr/local/lib")

# subcommand groups:  just check deps  |  just example build <name>
mod check
mod example
mod pac
mod memory     # per-chip linker memory maps (prod/memory-<chip>.x) — generate from TI .ld + lint
mod diag       # MSP430 POST/diagnostic firmware (Docker toolchain)
mod prod       # MSP430 product Stembus node firmware (Docker toolchain; family axis)
mod probe      # RP2040 bench-host firmware (modules/pico-probe) — host cargo, Linux+macOS
mod usb
mod bsp

default:
    @just --list

# Build a BOARD's firmware — chip-primary. `just build <chip>` builds its default image; `--diag` /
# `--debug` select the POST or debug class instead. Routes to the prod/diag/probe module per chip+class
# (no logic duplicated). MSP430 chips build via Docker; rp2040 via host cargo — both Linux + macOS.
#   just build fr2355            FR2355 product (prod)                 → Docker toolchain
#   just build fr2355 --diag     FR2355 POST/self-test (diag)          → Docker toolchain
#   just build rp2040            RP2040 bench host (stem-probe, debug) → host cargo
#   chips: fr2355 | fr2476 | fr2433 | fr2155 | rp2040   (default class = prod; rp2040 = debug)
build target *flags:
    #!/usr/bin/env bash
    set -euo pipefail
    # cros_ec is now the DEFAULT command interface (COMMANDS.md §0 — regmap register-poke is being retired).
    # `--regmap` is a TRANSITION escape hatch to build the old regmap slave until that path is deleted (Rb).
    cls=default; crosec=cros_ec; fam=""
    for f in {{flags}}; do case "${f#--}" in
        diag) cls=diag;; debug) cls=debug;; prod) cls=prod;;
        cros-ec|cros_ec) crosec=cros_ec;;  # explicit (now the default); kept for compatibility
        regmap) crosec="";;                # TRANSITION-ONLY: build the legacy regmap slave (to be removed)
        *) echo "unknown flag '$f' — use --diag | --debug | --regmap(transition)" >&2; exit 1;; esac; done
    case "{{target}}" in
        fr2355|fr235x) fam=fr235x; diagt=fr2355; kind=msp ;;
        fr2476|fr247x) fam=fr247x; diagt=fr2476; kind=msp ;;
        fr2433|fr24xx) fam=fr24xx; diagt="";     kind=msp ;;  # no diag image for FR2433
        fr2155|fr215x) fam=fr215x; diagt=fr2155; kind=msp ;;
        rp2040)        kind=cortex ;;
        *) echo "unknown chip '{{target}}' — fr2355|fr2476|fr2433|fr2155|rp2040" >&2; exit 1 ;;
    esac
    # cros_ec is an MSP430 EC-slave path (the default command interface on the nodes). It does not apply to
    # the RP2040 (it's the bus MASTER / stem-probe, not a node) — so clear the default crosec for non-MSP.
    [ "$kind" = msp ] || crosec=""
    if [ "$kind" = msp ]; then
        [ "$cls" = default ] && cls=prod
        case "$cls" in
            prod)  just prod build "$fam" auto console "" "$crosec" ;;
            diag)  [ -n "$diagt" ] || { echo "no diag image for {{target}}" >&2; exit 1; }; just diag build "$diagt" ;;  # diag ignores crosec for now (converges onto the unified vocabulary in Rd)
            debug) just prod build "$fam" auto debug "" "$crosec" ;;  # prod + `debug` feature (post-POST "what was found")
        esac
    else  # cortex (rp2040): its only firmware is the debug bench host
        case "$cls" in
            default|debug|prod) just probe build ;;
            diag) echo "no diag image for {{target}}" >&2; exit 1 ;;
        esac
    fi

# Build every DEFAULT firmware image: MSP430 products (Docker) + the RP2040 bench host (host cargo).
# One command, same justfile, Linux + macOS. Slow on macOS (MSP430 builds run under Rosetta amd64).
build-all:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "== MSP430 products (Docker toolchain) =="
    for chip in fr2355 fr2476 fr2433; do just build "$chip"; done
    echo "== RP2040 bench host (host cargo, rust-lld) =="
    just build rp2040

# build the toolchain Docker image locally (run once)
bootstrap:
    docker build --platform linux/amd64 -t {{image}} docker/

# build every example (smoke test; in the container on a host, natively in CI)
examples:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ -f /.dockerenv ]; then
        for d in examples/*/; do just example build "$(basename "$d")"; done
    else
        docker run --rm --platform linux/amd64 -v "$PWD":/work -w /work {{image}} just examples
    fi

# read the connected MSP430 device id (no flash)
msp-id:
    DYLD_FALLBACK_LIBRARY_PATH={{libdir}} {{mspdebug}} tilib exit

# watch the UART console (eZ-FET backchannel, 9600 8N1). Ctrl-C to stop.
# No arg = auto-pick the highest-numbered usbmodem (the eZ-FET backchannel; the numbers
# change on every re-enumeration). If that one is silent, pass the other explicitly:
#   just monitor /dev/cu.usbmodem23601
# One-way read (firmware only prints) — plain `cat`, so no screen / detached port holders.
monitor port="":
    #!/usr/bin/env bash
    set -euo pipefail
    p="{{port}}"
    if [ -z "$p" ]; then
        p=$(ls -1 /dev/cu.usbmodem* 2>/dev/null | sort | tail -1)
        [ -n "$p" ] || { echo "no /dev/cu.usbmodem* found — run 'just usb status'" >&2; exit 1; }
    fi
    echo "monitoring $p @9600 — Ctrl-C to stop"
    stty -f "$p" 9600 cs8 -cstopb -parenb raw -echo
    exec cat "$p"

# remove all build artifacts
clean:
    rm -rf examples/*/build examples/*/target
