# Proposal: `thepia hwd` support for RP2040/RP2350 (Pico) boards

Status: **proposal** for the separate `thepia hwd` work (the stem repo does not implement it). Derived
from [../RP2040-BENCH-HOST.md](../RP2040-BENCH-HOST.md) §1 (the loading design). Self-contained so it
can be handed off without the rest of the stem docs.

## 1. Summary

Add a `pico` (a.k.a. `rp2`) command family to `thepia hwd` that drives **RP2040/RP2350** boards over
**USB** using **`picotool`**, mirroring the existing `msp` family that drives MSP430 over `mspdebug`.
`hwd` shells out to `picotool` for program/verify/reset and keeps its **own** console, board-addressing,
and verify-by-stamp layers — exactly as it already does for MSP430. **No external debug probe** is
required: the RP2040's in-silicon USB bootloader is the programmer.

The immediate consumer is the **RP2040 bench dev-host** (an i.MX8 **M7 stand-in** that masters the stem
system for fully-automated ad-hoc/soak tests with no UCM attached). The bench board is already on the
rig: an **RP2040 rev B2**, alias `rp2040`, BOOTSEL serial **`E0C9125B0D9B`** (see `hwd.toml`).

## 2. The model — `picotool` is to `hwd` what `mspdebug` is

| `hwd` concept | MSP430 (`mspdebug`) | RP2040 (`picotool`) |
|---|---|---|
| select a board | `--board <ezfet-serial>` | `--ser <flash-uid>` (also `--bus/--address`, `--vid/--pid`) |
| program + byte-verify | mspdebug program+verify | `picotool load -x -v fw.uf2 --ser <s>` |
| enter programming mode | SBW (always available) | `picotool reboot -f -u --ser <s>` (force running→BOOTSEL) **or** HW BOOTSEL+RUN |
| standalone verify | `mspdebug verify` | `picotool verify fw.uf2 --ser <s>` |
| reset | SBW reset | `picotool reboot -a --ser <s>` (or HW RUN) |
| read memory | `msp read <addr>` | `picotool save -r <a> <b> out.bin --ser <s>` |
| identify | eZ-FET serial + JTAG id | `picotool info -a --ser <s>` + USB descriptors |
| console | backchannel UART tty | **USB CDC tty** (same console layer + verify-stamp watch) |

All `picotool` flags above are verified against the picotool v2.3.x README/BUILDING.md.

### Bench findings (v0 stem-probe firmware, 2026-09-30) — two things the implementer must know

Both confirmed on the bench RP2040 (rev B2) with the v0 `stem-probe` firmware + `picotool` v2.3.1:

1. **`picotool reboot -f` needs the firmware's reset interface.** Against v0 (which does **not** yet
   link it) it errored: *"Unable to locate reset interface on the device."* So the hands-free
   "enter BOOTSEL" step **only** works once the firmware links the picotool-reset interface (§7). Until
   then, the firmware's own command (v0: a `boot` line over CDC → `reset_to_usb_boot`) or the HW
   BOOTSEL/RUN lines are the only way in. **v0 `boot`→BOOTSEL was verified working.**
2. **The board key is MODE-DEPENDENT — the BOOTSEL serial ≠ the running iSerial.** On this board:
   BOOTSEL/`RP2 Boot` serial = **`E0C9125B0D9B`** (what `picotool --ser` matches when flashing), but the
   running firmware's `flash_unique_id` (its USB iSerial) = **`E6614C311B471322`**. They do **not**
   match. So `hwd` cannot assume one serial identifies the board in both states — it must **map both to
   one board** (record the pair, or translate). This is the single most important identity caveat.

## 3. Identity & discovery

- **Board key = the RP2040 serial.** The firmware sets USB `iSerial` = the **QSPI flash unique id**
  (ROM `flash_get_unique_id`); in BOOTSEL the `RP2 Boot` device carries the same serial. `picotool`
  targets it with **`--ser <uid>`** — the direct analogue of `msp`'s `--board`. Give it an `@alias` in
  `hwd.toml` like the MSP430 boards (already added: `rp2040` = `E0C9125B0D9B`).
- **State model** `hwd` should report (RP2040 has one `mspdebug` doesn't):
  - `running` — the application is up: app USB VID:PID + a CDC interface present.
  - `bootloader` — in BOOTSEL: PICOBOOT interface, VID:PID `2e8a:0003`.
- `hwd pico list` = enumerate USB `2e8a:*`, resolve each to a board by serial, print state + alias.

## 4. Command family (`hwd pico …`)

- `hwd pico list` — discover + identify (serial, state, alias, `picotool info -a`).
- `hwd pico flash <fw.uf2> --board <ser> [--verify-stamp <s>]` — the load/update cycle (§5).
- `hwd pico verify <fw.uf2> --board <ser>` — `picotool verify`.
- `hwd pico reset --board <ser>` — `picotool reboot -a` (→ application), or HW RUN.
- `hwd pico bootsel --board <ser>` — force BOOTSEL (`reboot -f -u`, or HW BOOTSEL+RUN).
- `hwd console tail/send/annotate --board <ser>` — over the USB **CDC** tty (§6), reusing the existing
  console layer.

## 5. The flash/update cycle — `hwd pico flash`

1. **Ensure BOOTSEL.** If the board is `running`: `picotool reboot -f -u --ser <ser>`, then wait for the
   PICOBOOT interface (`2e8a:0003`, same serial) to enumerate. If it is wedged/blank and won't accept
   `-f`, fall back to the **hardware BOOTSEL+RUN** lines (§8) — the one recovery USB can't do itself.
2. **Program + byte-verify.** `picotool load -x -v fw.uf2 --ser <ser>` — writes flash, `-v` byte-verifies
   the write (the `mspdebug` byte-verify analogue), `-x` executes.
3. **Verify-by-stamp (optional).** With `--verify-stamp <s>`, watch the **CDC** console for the build
   stamp within a timeout (reuse the existing verify-stamp mechanism; the RP2040 prints its stamp +
   flash-uid on boot). Report `programmed + byte-verified + confirmed running`.

Note: the banner lands during the USB re-enumeration gap after the reset — attach the CDC watcher
**before** issuing the reboot/execute, same lesson as the MSP430 console (see the console-events
proposal EXT-4).

## 6. Console — USB CDC

Treat the host firmware's **USB CDC** interface as the console (the RP2040 analogue of the MSP430
backchannel UART): `/dev/cu.usbmodem*` on macOS, `/dev/ttyACM*` on Linux. Same read-only-capture
discipline; `console tail/send/annotate` work unchanged. This is **run-time control** — distinct from
`picotool`, which only does program/verify/reset.

## 7. Firmware requirements (on the RP2040 side, NOT hwd)

For the above to work, the host firmware must:
- **Link a `picotool`-compatible USB reset interface** (Pico-SDK `stdio_usb`/reset; in Rust/embassy, an
  `embassy-usb` reset class / `usbd-picotool-reset`-style interface) — else `reboot -f` cannot force
  BOOTSEL and only the hardware lines work.
- Set **USB `iSerial` = the flash unique id** so `--ser` + `hwd` addressing are stable.
- Emit a **build stamp** (+ the flash-uid) on the CDC console at boot, for verify-by-stamp.

## 8. Cold recovery / control lines (optional, later)

For unbrickable recovery of a blank/wedged board, wire **BOOTSEL** and **RUN** to `hwd`-addressable
outputs (whatever bench GPIO source `hwd` uses). Then `hwd pico bootsel` / `hwd pico reset` can force
BOOTSEL / reset without USB cooperation. Not needed for normal updates (`reboot -f` covers those).

## 9. Dependencies & environment

- **`picotool`** binary — Homebrew (`brew install picotool`) on macOS, prebuilt (pico-sdk-tools) or
  source on Linux. Plus `libusb`.
- **Linux CI non-root USB:** install the picotool udev rule — `sudo cp udev/60-picotool.rules
  /etc/udev/rules.d/` — so `hwd`/CI can run `picotool` without sudo.
- **No external programmer hardware** (unlike MSP430's eZ-FET). USB access only.

## 10. Differences from `msp`/`mspdebug` (for the implementer)

- **USB-only, no probe** — the mask-ROM USB bootloader is the programmer; it's **unbrickable** (BOOTSEL
  always recovers).
- **Running↔BOOTSEL state model** — `mspdebug` has no analogue; the `reboot -f` "enter programming mode"
  step is new.
- **Two channels** — `picotool` (program/verify/reset) + a separate **CDC** tty (console/control). Don't
  conflate them.
- **RP2350 works the same** — `picotool` supports it; the flow generalizes RP2040→RP2350 unchanged. Name
  the family `pico`/`rp2` (chip-family), not `rp2040`.

## 11. Suggested phasing

- **v0:** `list` + `flash` (ensure-BOOTSEL via `reboot -f` → `load -x -v`) + CDC `console` + verify-stamp.
  Enough to load/update the bench host and read it — the stated first challenge.
- **Later:** `bootsel`/`reset` hardware-line control (cold recovery), `read`/`save`, multi-board routing,
  RP2350.

## 12. Open questions for the `thepia hwd` side

1. Command family name: `pico` vs `rp2` (recommend `pico`, but `rp2` matches `msp` terseness).
2. How `hwd` addresses BOOTSEL vs running states in one alias (the serial is stable across both — is
   `picotool --ser` selection reliable in both states on the target OSes?).
3. Whether to wrap the `picotool` binary or link `libpicotool`/PICOBOOT directly (recommend: shell out to
   the binary first, like `mspdebug`).
4. The bench GPIO source for BOOTSEL/RUN (only if/when cold-recovery control lines are wanted).
