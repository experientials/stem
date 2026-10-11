#!/bin/sh
# Validate the per-chip linker memory maps  prod/memory-<chip>.x  against in-repo sources of truth.
# Runs in CI with NO external dependency (uses the vendored PAC + the msp430-rt invariant). If TI's
# .ld files are reachable (MSP430_LD_DIR or the usual install paths), it ALSO diffs RAM/ROM to catch
# transcription/drift — otherwise it skips that check and says so.
#
# Checks:
#   1. VECTORS must end at 0x10000            — msp430-rt's link.x ASSERTs this; a bad value fails to link.
#   2. VECTORS vs the chip's PAC              — reports the origin `rt` will require (readiness), and
#                                               FAILS if VECTORS is placed BELOW the PAC's lowest vector.
#   3. fr2155 and fr2355 maps are identical   — the "shared map" claim, verified not assumed.
#   4. (optional) RAM/ROM == TI .ld           — drift guard when the .ld is available.
#
# Usage: scripts/lint-memory-x.sh
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
fail=0
note() { printf '  %s\n' "$1"; }
err()  { printf '  ✗ %s\n' "$1"; fail=1; }

hexfield() { # <file> <REGION> <ORIGIN|LENGTH>
  sed -nE "s/.*$2[ \t]*:.*$3 *= *(0x[0-9A-Fa-f]+).*/\1/p" "$1" | head -1
}

# locate TI .ld (optional)
LD_DIR="${MSP430_LD_DIR:-}"
if [ -z "$LD_DIR" ]; then
  for d in \
    "/Applications/ti/ccs2101/ccs/ccs_base/msp430/include_gcc" \
    "$HOME/Library/Mobile Documents/com~apple~CloudDocs/Downloads/msp430-gcc/include" \
    "/opt/msp430-gcc/include"; do
    [ -f "$d/msp430fr2355.ld" ] && { LD_DIR="$d"; break; }
  done
fi

for chip in fr2155 fr2355 fr2476 fr2433; do
  # fr2433's map lives with its binary (modules/expander/, S2a reorg); the dual families stay in prod/.
  if [ "$chip" = fr2433 ]; then f="$ROOT/modules/expander/memory-$chip.x"; else f="$ROOT/prod/memory-$chip.x"; fi
  pac="$ROOT/pac/msp430$chip/src/lib.rs"
  echo "== memory-$chip.x =="
  [ -f "$f" ] || { err "missing $f"; continue; }

  # 1. VECTORS ends at 0x10000
  vo=$(hexfield "$f" VECTORS ORIGIN); vl=$(hexfield "$f" VECTORS LENGTH)
  [ -n "$vo" ] && [ -n "$vl" ] || { err "cannot parse VECTORS"; continue; }
  end=$(( vo + vl ))
  if [ "$end" -eq 65536 ]; then
    note "VECTORS $vo + $vl ends at 0x10000 ✓ (msp430-rt link ASSERT)"
  else
    err "VECTORS $vo + $vl ends at $(printf 0x%X "$end"), not 0x10000 — msp430-rt link.x will reject this"
  fi

  # 2. cross-check against the PAC's interrupt vector table
  if [ -f "$pac" ]; then
    low=$(grep -oE '0xFF[0-9A-F]{2}' "$pac" | sort -u | head -1)
    if [ -n "$low" ]; then
      rtlen=$(( 65536 - low ))
      if [ "$(( vo ))" -le "$(( low ))" ]; then
        note "PAC lowest vector $low — VECTORS covers the full table (rt-ready)"
      else
        note "PAC lowest vector $low — VECTORS is the 16-word no-rt default; when the PAC 'rt' feature + ISRs land, set ORIGIN=$low LENGTH=$(printf 0x%04X "$rtlen")"
      fi
    else
      note "PAC vector table not found in $pac (skip cross-check)"
    fi
  else
    note "no PAC at $pac (skip cross-check)"
  fi

  # 4. optional TI .ld drift check
  if [ -n "$LD_DIR" ] && [ -f "$LD_DIR/msp430$chip.ld" ]; then
    ld="$LD_DIR/msp430$chip.ld"
    # Anchor region names so TINYRAM/HIFRAM don't shadow RAM/FRAM.
    lram_o=$(awk '/^MEMORY/,/^}/' "$ld" | sed -nE 's/^[ \t]*RAM[ \t]*:.*ORIGIN *= *(0x[0-9A-Fa-f]+).*/\1/p' | head -1)
    lram_l=$(awk '/^MEMORY/,/^}/' "$ld" | sed -nE 's/^[ \t]*RAM[ \t]*:.*LENGTH *= *(0x[0-9A-Fa-f]+).*/\1/p' | head -1)
    lfr_o=$(awk '/^MEMORY/,/^}/' "$ld" | sed -nE 's/^[ \t]*FRAM *\(.*ORIGIN *= *(0x[0-9A-Fa-f]+).*/\1/p' | head -1)
    lfr_l=$(awk '/^MEMORY/,/^}/' "$ld" | sed -nE 's/^[ \t]*FRAM *\(.*LENGTH *= *(0x[0-9A-Fa-f]+).*/\1/p' | head -1)
    ro=$(hexfield "$f" RAM ORIGIN); rl=$(hexfield "$f" RAM LENGTH)
    fo=$(hexfield "$f" ROM ORIGIN); fl=$(hexfield "$f" ROM LENGTH)
    if [ "$(( ro ))" = "$(( lram_o ))" ] && [ "$(( rl ))" = "$(( lram_l ))" ] &&
       [ "$(( fo ))" = "$(( lfr_o ))" ] && [ "$(( fl ))" = "$(( lfr_l ))" ]; then
      note "RAM/ROM match TI msp430$chip.ld ✓"
    else
      err "RAM/ROM DRIFT vs TI msp430$chip.ld — RAM file=$ro/$rl ld=$lram_o/$lram_l · ROM file=$fo/$fl ld=$lfr_o/$lfr_l — regenerate: scripts/gen-memory-x.sh"
    fi
  else
    note "TI .ld not reachable — skipped RAM/ROM drift check (set MSP430_LD_DIR to enable)"
  fi
done

echo "== shared-map invariant =="
same=1
for reg in RAM ROM VECTORS; do
  for fld in ORIGIN LENGTH; do
    a=$(hexfield "$ROOT/prod/memory-fr2155.x" "$reg" "$fld")
    b=$(hexfield "$ROOT/prod/memory-fr2355.x" "$reg" "$fld")
    [ "$a" = "$b" ] || same=0
  done
done
if [ "$same" -eq 1 ]; then
  note "fr2155 and fr2355 memory blocks are identical ✓"
else
  err "fr2155 and fr2355 memory blocks differ — they must share the same map (TI .ld is identical)"
fi

echo
[ "$fail" -eq 0 ] && echo "memory-x lint: OK" || { echo "memory-x lint: FAILED"; exit 1; }
