#!/bin/sh
# Regenerate the vendored MSP430 PAC crates under pac/<device>/.
#
# WHY the non-obvious steps (full detail in the msp430-macos-dev skill's
# references/rust-pac.md):
#   - svd2rust is fetched as a PREBUILT binary. Do NOT `cargo install svd2rust` on this
#     Mac — it wedges for hours (rust-objcopy hangs in uninterruptible state).
#   - The stable toolchain is invoked by ABSOLUTE path with RUSTC pinned, because an old
#     /usr/local/bin/cargo (1.64) shadows rustup on PATH and cargo falls back to rustc 1.64.
#
# Usage: scripts/gen-pac.sh [device ...]
#        (default: all four vendored PACs — msp430fr2433 msp430fr2476 msp430fr2155 msp430fr2355)
set -eu

SVD2RUST_VERSION=v0.37.1
MSP430_SVD_REPO=https://github.com/pftbest/msp430_svd
DEVICES="${*:-msp430fr2433 msp430fr2476 msp430fr2155 msp430fr2355}"

ROOT=$(cd "$(dirname "$0")/.." && pwd)   # stem/
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

host_target() {
  case "$(uname -s)/$(uname -m)" in
    Darwin/arm64)   echo aarch64-apple-darwin ;;
    Darwin/x86_64)  echo x86_64-apple-darwin ;;
    Linux/aarch64)  echo aarch64-unknown-linux-gnu ;;
    Linux/x86_64)   echo x86_64-unknown-linux-gnu ;;
    *) echo "gen-pac: no prebuilt svd2rust for $(uname -s)/$(uname -m)" >&2; exit 1 ;;
  esac
}

echo "==> svd2rust $SVD2RUST_VERSION (prebuilt binary)"
SVD2RUST="$WORK/svd2rust"
curl -fsSL -o "$WORK/svd2rust.gz" \
  "https://github.com/rust-embedded/svd2rust/releases/download/$SVD2RUST_VERSION/svd2rust-$(host_target).gz"
gunzip -c "$WORK/svd2rust.gz" > "$SVD2RUST"
chmod +x "$SVD2RUST"

echo "==> stable cargo (absolute path; avoids the old PATH cargo)"
rustup toolchain install stable --profile minimal >/dev/null 2>&1 || true
CARGO="$(rustup which --toolchain stable cargo)"
RUSTC="$(rustup which --toolchain stable rustc)"; export RUSTC

echo "==> msp430_svd -> patched SVDs"
git clone --depth 1 "$MSP430_SVD_REPO" "$WORK/msp430_svd" >/dev/null 2>&1
cd "$WORK/msp430_svd"
for dev in $DEVICES; do
  echo "    generating $dev.svd"
  "$CARGO" run --quiet -- "$dev" >/dev/null   # writes $dev.svd (+ .svd.patched if an override exists)
done

echo "==> svd2rust -> vendor into pac/<device>/"
for dev in $DEVICES; do
  svd="$WORK/msp430_svd/$dev.svd.patched"
  [ -f "$svd" ] || svd="$WORK/msp430_svd/$dev.svd"
  out="$ROOT/pac/$dev"
  mkdir -p "$out/src"
  ( cd "$out" && "$SVD2RUST" -i "$svd" --target msp430 )
  mv "$out/lib.rs" "$out/src/lib.rs"          # build.rs + device.x land in $out directly
  echo "    wrote $out (Cargo.toml is hand-maintained, preserved)"
done

echo "Done. Compile-test with:  just pac check <device>"
