//! Emits `PROD_BUILD` — the firmware's identity stamp, compiled in and printed at boot. Same scheme
//! as prod/diag build.rs (see them for the full rationale): the board announces exactly what it's
//! running, so "is this the image I just flashed, or a stale one?" is answerable by reading the UART.
//!
//! The env-var names are kept as `PROD_*` (not `EXPANDER_*`) on purpose — so the shared build infra
//! (the Docker `-e PROD_BUILD` forward, the CI stamp pin) stays identical to prod's; this is the single
//! FR2433 variant, so there is no ambiguity. Format: `<target>/<year>.<release>-<short-hash>[-dirty][.<secs>]`.
//!
//! This module is FR2433-only, so `emit_memory_x` just places `memory-fr2433.x` (no family switch).

use std::path::PathBuf;
use std::process::Command;

fn main() {
    emit_memory_x();

    println!("cargo:rerun-if-env-changed=PROD_BUILD");
    println!("cargo:rerun-if-env-changed=PROD_RELEASE");
    println!("cargo:rerun-if-env-changed=PROD_TARGET");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
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

/// Build target for the stamp. This crate is FR2433-only; override with `PROD_TARGET` for a finer variant.
fn target() -> String {
    std::env::var("PROD_TARGET")
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "fr2433".into())
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

/// Place the FR2433 linker memory map where `msp430-rt`'s `link.x` (`INCLUDE memory.x`) will find it:
/// copy `memory-fr2433.x` to OUT_DIR as `memory.x` and add OUT_DIR to the linker search path.
fn emit_memory_x() {
    let src = "memory-fr2433.x";
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::copy(src, out.join("memory.x")).unwrap_or_else(|e| panic!("expander: copy {src} → OUT_DIR: {e}"));
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
