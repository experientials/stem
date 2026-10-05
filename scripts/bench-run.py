#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["pyserial>=3.5"]
# ///
"""stem-probe bench runner — drive the RP2040 USB-CDC console from the host.

The RP2040 `stem-probe` firmware (debug/stem-probe) is the Stem-bus master / SoM stand-in. It exposes a
line console (id | scan | r | w | selftest | vstem | vleaf | pass | sens | sound0/1 | por0/1 | pins).
This script auto-finds that CDC port (USB 2e8a:000a, product "stem-probe"), issues a command, prints the
reply, and — for `selftest` — parses the final `RESULT pass=N fail=M` line and exits non-zero on any
failure. That exit code is what `.github/workflows/firmware.yml` gates on.

Usage (via uv — auto-installs pyserial, no venv to manage; project rule: always uv, never pip):
    uv run scripts/bench-run.py                 # default: selftest, exit 0 iff fail=0
    uv run scripts/bench-run.py scan            # send one arbitrary command, print the reply
    uv run scripts/bench-run.py r 0x20 0x30     # read a register
    uv run scripts/bench-run.py w 0x20 0x3e 1   # write a register
    uv run scripts/bench-run.py --port /dev/ttyACM0 selftest
    uv run scripts/bench-run.py --list          # list candidate ports and exit

Exit codes: 0 = ok (selftest fail=0, or a non-selftest command completed); 1 = test failures;
2 = no board / port error; 3 = bad usage.
"""
from __future__ import annotations

import argparse
import re
import sys
import time

try:
    import serial  # pyserial
    from serial.tools import list_ports
except ImportError:  # pragma: no cover - uv installs it; this is the bare-python fallback message
    print("pyserial missing — run via `uv run scripts/bench-run.py` (do NOT pip install).", file=sys.stderr)
    sys.exit(2)

# USB identity the firmware advertises (main.rs: Config::new(0x2e8a, 0x000a), product "stem-probe").
VID = 0x2E8A
PID = 0x000A
PRODUCT_HINT = "stem-probe"

PROMPT = b"> "
RESULT_RE = re.compile(rb"RESULT pass=(\d+) fail=(\d+)")


def find_port() -> str | None:
    """Pick the stem-probe CDC port: match VID:PID first, then a product/description hint."""
    cands = list(list_ports.comports())
    for p in cands:
        if p.vid == VID and p.pid == PID:
            return p.device
    for p in cands:
        hay = " ".join(filter(None, (p.product, p.description, p.manufacturer))).lower()
        if PRODUCT_HINT in hay:
            return p.device
    return None


def list_candidates() -> None:
    rows = list(list_ports.comports())
    if not rows:
        print("(no serial ports found)")
        return
    for p in rows:
        vidpid = f"{p.vid:04x}:{p.pid:04x}" if p.vid is not None else "----:----"
        mark = "  <- stem-probe" if (p.vid == VID and p.pid == PID) else ""
        print(f"{p.device}\t{vidpid}\t{p.product or p.description or ''}{mark}")


def drain_until_prompt(ser: serial.Serial, timeout: float) -> bytes:
    """Read until the `> ` prompt reappears or `timeout` elapses. Returns everything read."""
    end = time.monotonic() + timeout
    out = bytearray()
    while time.monotonic() < end:
        chunk = ser.read(256)
        if chunk:
            out += chunk
            if out.rstrip().endswith(PROMPT.strip()):
                break
        else:
            # idle gap after we've already seen some output → assume the reply is complete
            if out:
                break
    return bytes(out)


def run(port: str, command: str, timeout: float) -> int:
    try:
        # CDC ignores the baud rate; 115200 is conventional. Short read timeout so the drain loop ticks.
        ser = serial.Serial(port, 115200, timeout=0.2)
    except serial.SerialException as e:
        print(f"cannot open {port}: {e}", file=sys.stderr)
        return 2

    with ser:
        time.sleep(0.2)
        ser.reset_input_buffer()
        ser.write(command.encode() + b"\r\n")
        ser.flush()
        # selftest runs vstem+vleaf with ~1s of I²C delays inside; give it room.
        reply = drain_until_prompt(ser, timeout)

    text = reply.decode(errors="replace")
    print(text, end="" if text.endswith("\n") else "\n")

    if command.strip().lower().startswith("selftest"):
        m = RESULT_RE.search(reply)
        if not m:
            print("\n[runner] no RESULT line — did the firmware run selftest? treating as failure.", file=sys.stderr)
            return 1
        npass, nfail = int(m.group(1)), int(m.group(2))
        print(f"\n[runner] pass={npass} fail={nfail}")
        return 0 if nfail == 0 else 1
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description="Drive the stem-probe bench console over USB-CDC.")
    ap.add_argument("command", nargs="*", help="console command (default: selftest)")
    ap.add_argument("--port", help="serial device (default: auto-detect 2e8a:000a / 'stem-probe')")
    ap.add_argument("--timeout", type=float, default=8.0, help="seconds to wait for the reply (default 8)")
    ap.add_argument("--list", action="store_true", help="list candidate serial ports and exit")
    args = ap.parse_args()

    if args.list:
        list_candidates()
        return 0

    port = args.port or find_port()
    if not port:
        print("no stem-probe found (looked for USB 2e8a:000a / product 'stem-probe').", file=sys.stderr)
        print("is it plugged in and running (not in BOOTSEL)? try --list.", file=sys.stderr)
        return 2

    command = " ".join(args.command) if args.command else "selftest"
    return run(port, command, args.timeout)


if __name__ == "__main__":
    sys.exit(main())
