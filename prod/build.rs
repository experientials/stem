//! Emits `PROD_BUILD` — the firmware's identity stamp, compiled in and printed at boot. Same scheme
//! as diag/build.rs (see it for the full rationale): the board announces exactly what it's running,
//! so "is this the image I just flashed, or a stale one?" is answerable by reading the UART.
//!
//! Format: `<target>/<year>.<release>-<short-hash>[-dirty][.<secs>]`
//! e.g. `fr2476/2026.dev-59ad941-dirty.1725540000`
//!   <target>  chip/config variant, from `PROD_TARGET`; defaults to `fr2476` (primary dev target).
//!   <year>    build year (UTC).
//!   <release> `PROD_RELEASE` when set (a real release), else `dev`. No release system yet.
//!   -dirty    the working tree had uncommitted changes.
//!   .<secs>   dev builds only, so a rebuilt (dirty) tree still yields a distinct stamp.
//!
//! `just prod run` / CI may pin a unique suffix via `PROD_BUILD`; we prefix target+version onto it.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    emit_memory_x();

    println!("cargo:rerun-if-env-changed=PROD_BUILD");
    println!("cargo:rerun-if-env-changed=PROD_RELEASE");
    println!("cargo:rerun-if-env-changed=PROD_TARGET");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    // Re-derive the stamp when the sources change, so a rebuilt image gets a fresh stamp (the earlier
    // staleness: editing src/ did NOT re-run build.rs, so PROD_BUILD's secs froze across rebuilds).
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");

    let version = version(); // <year>.<release>
    let is_dev = version.ends_with(".dev");
    let prefix = format!("{}/{version}", target()); // <target>/<year>.<release>
    let stamp = match std::env::var("PROD_BUILD") {
        Ok(s) if !s.trim().is_empty() => format!("{prefix}-{s}"),
        _ => derive(&prefix, is_dev),
    };
    println!("cargo:rustc-env=PROD_VERSION={version}");
    println!("cargo:rustc-env=PROD_BUILD={stamp}");
}

/// Build target (chip/config variant) for the stamp. Defaults from the active family feature
/// (`fr2476` for fr247x, `fr2155`/`fr2355` for fr215x/fr235x); override with `PROD_TARGET`.
fn target() -> String {
    if let Some(t) = std::env::var("PROD_TARGET").ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        return t;
    }
    if std::env::var_os("CARGO_FEATURE_FR247X").is_some() {
        "fr2476".into()
    } else if std::env::var_os("CARGO_FEATURE_FR215X").is_some() {
        "fr2155".into()
    } else if std::env::var_os("CARGO_FEATURE_FR235X").is_some() {
        "fr2355".into()
    } else {
        "unknown".into()
    }
}

/// `<year>.<release>` — CalVer. Year from the build clock; release from `PROD_RELEASE` or `dev`.
fn version() -> String {
    let year = run("date", &["-u", "+%Y"]).unwrap_or_else(|| "0000".into());
    let release = match std::env::var("PROD_RELEASE") {
        Ok(r) if !r.trim().is_empty() => r,
        _ => "dev".into(),
    };
    format!("{year}.{release}")
}

/// Full derived stamp when no `PROD_BUILD` suffix is pinned: prefix + commit + dirty (+ time on dev).
fn derive(prefix: &str, is_dev: bool) -> String {
    let hash = run("git", &["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "nogit".into());
    let dirty = match run("git", &["status", "--porcelain"]) {
        Some(s) if !s.trim().is_empty() => "-dirty",
        _ => "",
    };
    let mut stamp = format!("{prefix}-{hash}{dirty}");
    if is_dev {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        stamp = format!("{stamp}.{secs}");
    }
    stamp
}

/// Select the active family's linker memory map and place it where `msp430-rt`'s `link.x`
/// (`INCLUDE memory.x`) will find it: copy `memory-<family>.x` to OUT_DIR as `memory.x` and add
/// OUT_DIR to the linker search path. Exactly one family feature must be enabled (Cargo `[features]`).
fn emit_memory_x() {
    let has = |f: &str| std::env::var_os(f).is_some();
    // Per-chip memory maps, GENERATED from TI's .ld by scripts/gen-memory-x.sh (validated by
    // scripts/lint-memory-x.sh). Exactly one family feature is active (Cargo `[features]` + the
    // compile_error guards in main.rs). fr215x→fr2155 and fr235x→fr2355 are now DISTINCT files (even
    // though byte-identical) — no more silent sharing.
    let src = if has("CARGO_FEATURE_FR247X") {
        "memory-fr2476.x"
    } else if has("CARGO_FEATURE_FR215X") {
        "memory-fr2155.x"
    } else if has("CARGO_FEATURE_FR235X") {
        "memory-fr2355.x"
    } else {
        panic!("prod: no family feature — build with --features fr247x|fr215x|fr235x");
    };
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::copy(src, out.join("memory.x")).unwrap_or_else(|e| panic!("prod: copy {src} → OUT_DIR: {e}"));
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed={src}");
}

fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
