# `just probe <verb>` — the RP2040 bench-host firmware (debug/stem-probe; the SoM/M7 stand-in).
#
# Cortex-M (thumbv6m): links with **rust-lld, built into Rust** → HOST cargo, **no container**, and
# identical on Linux + macOS. That's the only reason it differs from the MSP430 firmware (prod/diag),
# which needs TI's msp430-elf-gcc and therefore the Docker image. Same justfile, two link toolchains.
# Usually reached chip-first via `just build rp2040`; this module is the implementation + direct access.
#
#   just probe build     host cargo build (release)
#   just probe flash     build + picotool program over USB (BOOTSEL)
#   just probe run       alias for flash

crate  := "debug/stem-probe"
triple := "thumbv6m-none-eabi"
bin    := "stem-probe"

# Build the RP2040 firmware. The crate's own rust-toolchain.toml pins the channel + target (rustup
# auto-installs thumbv6m) — so this runs the same on Linux and macOS, no Docker, no external linker.
build:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "── {{bin}}: {{crate}} ({{triple}}) — host cargo + rust-lld (no container) ──"
    ( cd {{crate}} && cargo build --release )
    ls -la {{crate}}/target/{{triple}}/release/{{bin}}

# Build + program over USB on the HOST (no USB in containers). Needs the board in BOOTSEL — or force it
# with `picotool reboot -f -u` (the firmware's reset interface). picotool: brew on macOS, pkg/build on Linux.
flash: build
    #!/usr/bin/env bash
    set -euo pipefail
    elf="{{crate}}/target/{{triple}}/release/{{bin}}"
    echo "flashing $elf via picotool"
    picotool load "$elf" && picotool reboot

# Alias: build + flash.
run: flash
