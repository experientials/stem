//! Boot POST integrity + logging (DESIGN.md console/--debug logging; ADR-0010 M7-judges integrity).
//!
//! [`fram_crcs`] computes a **per-sector CRC16 over the program FRAM** on EVERY boot of EVERY build
//! (incl. the silent production image) — NOT console-gated. It uses the SHARED [`stembus::crc`] codec
//! (the same algorithm the M7 runs over the image it flashed), so golden and reported CRCs match by
//! construction. The MSP is a *dumb reporter*: it stores the raw CRCs for the M7 to read over I²C
//! (`regmap::DBG_CRC` 0x3F, windowed) and does NOT judge — the M7 compares + reflashes (ADR-0010).
//!
//! [`boot_log`] (console) and [`debug_leaf`] (--debug) are the dev UART echo only.

// Only the console echo fns touch hardware — and they do so through the host-clean `node_hal` seams
// (Phase 6c), so this file names no concrete PAC type. `fram_crcs` needs nothing.
#[cfg(feature = "console")]
use node_hal::{ChipId, Console};
#[cfg(all(feature = "debug", feature = "_dual"))]
use node_hal::LeafBus;

// Program-FRAM window, sourced from the active family's `memory-*.x` via linker symbols (NOT hardcoded —
// so it can't drift from the memory map). `&__fram_start` = ORIGIN(ROM); `&__fram_size` = LENGTH(ROM)
// (the absolute-symbol trick). See the PROVIDE() lines in prod/memory-*.x.
extern "C" {
    static __fram_start: u8;
    static __fram_size: u8;
}
fn fram_start() -> usize {
    (unsafe { &__fram_start as *const u8 }) as usize
}
fn fram_size() -> usize {
    (unsafe { &__fram_size as *const u8 }) as usize
}

/// Per-sector CRC16 over the program FRAM, as bytes `[sec0_hi, sec0_lo, sec1_hi, sec1_lo, …]`. Always
/// compiled / always run at boot (ADR-0010). Serialized from the shared [`stembus::crc::sector_crcs`]
/// so the node and the M7 use one algorithm + one sectoring. Served over I²C at `regmap::DBG_CRC` (0x3F).
pub fn fram_crcs() -> [u8; stembus::crc::SECTORS * 2] {
    // The program FRAM is readable, 'static memory — a slice over it feeds the shared codec.
    let fram = unsafe { core::slice::from_raw_parts(fram_start() as *const u8, fram_size()) };
    let crcs = stembus::crc::sector_crcs(fram); // [u16; SECTORS]
    let mut out = [0u8; stembus::crc::SECTORS * 2];
    let mut s = 0;
    while s < stembus::crc::SECTORS {
        out[2 * s] = (crcs[s] >> 8) as u8;
        out[2 * s + 1] = crcs[s] as u8;
        s += 1;
    }
    out
}

/// LEAN boot log (console): version banner + the per-sector CRCs (passed in — computed once in main).
#[cfg(feature = "console")]
pub fn boot_log<H: Console>(
    hw: &H,
    chip: ChipId, // host-clean chip identity (device id + family-match), built once in main (§6c)
    mode_name: &str,
    crc: &[u8; stembus::crc::SECTORS * 2],
    build: &str, // per-image build stamp, injected by the bin (§B) — the lib can't env! it
) {
    hw.puts("\n== v");
    hw.puts(crate::VERSION); // deepsleep capability version (CARGO_PKG_VERSION)
    hw.putc(b' ');
    hw.puts(build);
    hw.puts(" (");
    hw.puts(stembus::role::model_name(chip.device_id));
    hw.puts(") mode=");
    hw.puts(mode_name);
    hw.puts(" ==\n");
    if !chip.family_ok {
        hw.puts("!! WRONG-FAMILY FLASH: detected part is not in this image's family\n");
    }
    let o = fram_start();
    hw.puts("fram crc16 x8 @0x");
    hw.hex8((o >> 8) as u8);
    hw.hex8(o as u8);
    hw.puts(" (read over I2C @0x3F):");
    let mut s = 0;
    while s < stembus::crc::SECTORS {
        hw.putc(b' ');
        hw.hex8(crc[2 * s]);
        hw.hex8(crc[2 * s + 1]);
        s += 1;
    }
    hw.putc(b'\n');
}

/// `--debug` Leaf/eUSCI-B0 diagnostic — the "more info" for a scan that found nothing (e.g. the FR2355
/// Leaf investigation): idle bus levels + the eUSCI_B0 config/flags. Call after a Sensing scan (B0 is
/// the master then). Dual-only. Not a full diag — DESIGN.md.
#[cfg(all(feature = "debug", feature = "_dual"))]
pub fn debug_leaf<H: Console + LeafBus>(hw: &H) {
    let (sda, scl) = hw.bus_levels();
    let (ctlw0, ifg) = hw.leaf_debug_regs();
    hw.puts("leaf-dbg: SDA=");
    hw.putc(if sda { b'H' } else { b'L' });
    hw.puts(" SCL=");
    hw.putc(if scl { b'H' } else { b'L' });
    hw.puts("  UCB0CTLW0=0x");
    hw.hex8((ctlw0 >> 8) as u8);
    hw.hex8(ctlw0 as u8);
    hw.puts(" (MST="); // UCMST (master) bit 0x0800
    hw.putc(if ctlw0 & 0x0800 != 0 { b'1' } else { b'0' });
    hw.puts(" SWRST="); // UCSWRST bit 0x0001 (held in reset = off the bus)
    hw.putc(if ctlw0 & 0x0001 != 0 { b'1' } else { b'0' });
    hw.puts(")  UCB0IFG=0x"); // bit5 UCNACKIFG = last addr NACKed (no device)
    hw.hex8((ifg >> 8) as u8);
    hw.hex8(ifg as u8);
    hw.puts(" (NACK=");
    hw.putc(if ifg & 0x0020 != 0 { b'1' } else { b'0' });
    hw.puts(")\n");
}
