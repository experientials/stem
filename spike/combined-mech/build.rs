//! Place `memory.x` where msp430-rt's `link.x` (`INCLUDE memory.x`) will find it — same mechanism as
//! prod/build.rs, but a SINGLE map (this spike's one image uses the FR2476∩FR2355 common memory map:
//! 4 KB RAM, 0x8000/0x7F80 ROM, VECTORS 0xFF80/0x80). Copy to OUT_DIR and add it to the link search path.
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    std::fs::copy("memory.x", out.join("memory.x")).expect("copy memory.x → OUT_DIR");
    // device.x is now supplied by the COMBINED PAC's build.rs (feature `rt` → emits device.x + its own
    // link-search), so we no longer emit one here (two device.x on the search path would conflict).
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
    println!("cargo:rerun-if-changed=build.rs");
}
