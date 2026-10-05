//! Boot POST integrity + logging (DESIGN.md console/--debug logging; ADR-0010 M7-judges integrity).
//!
//! [`fram_crcs`] computes a **per-sector CRC16 over the program FRAM** on EVERY boot of EVERY build
//! (incl. the silent production image) — NOT console-gated. It uses the SHARED [`stembus::crc`] codec
//! (the same algorithm the M7 runs over the image it flashed), so golden and reported CRCs match by
//! construction. The MSP is a *dumb reporter*: it stores the raw CRCs for the M7 to read over I²C
//! (`regmap::DBG_CRC` 0x3F, windowed) and does NOT judge — the M7 compares + reflashes (ADR-0010).
//!
//! [`boot_log`] (console) and [`debug_leaf`] (--debug) are the dev UART echo only.

#[cfg(feature = "console")] // only the UART echo fns take Peripherals; fram_crcs needs nothing
use crate::pac::Peripherals;

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
pub fn boot_log(
    p: &Peripherals,
    model: crate::model::Model,
    mode_name: &str,
    crc: &[u8; stembus::crc::SECTORS * 2],
) {
    crate::uart::puts(p, "\n== prod v");
    crate::uart::puts(p, crate::FW_VERSION);
    crate::uart::putc(p, b' ');
    crate::uart::puts(p, crate::FW_BUILD);
    crate::uart::puts(p, " (");
    crate::uart::puts(p, crate::model_name(model));
    crate::uart::puts(p, ") mode=");
    crate::uart::puts(p, mode_name);
    crate::uart::puts(p, " ==\n");
    if !model.matches_build_family() {
        crate::uart::puts(p, "!! WRONG-FAMILY FLASH: detected part is not in this image's family\n");
    }
    let o = fram_start();
    crate::uart::puts(p, "fram crc16 x8 @0x");
    crate::uart::hex8(p, (o >> 8) as u8);
    crate::uart::hex8(p, o as u8);
    crate::uart::puts(p, " (read over I2C @0x3F):");
    let mut s = 0;
    while s < stembus::crc::SECTORS {
        crate::uart::putc(p, b' ');
        crate::uart::hex8(p, crc[2 * s]);
        crate::uart::hex8(p, crc[2 * s + 1]);
        s += 1;
    }
    crate::uart::putc(p, b'\n');
}

/// `--debug` Leaf/eUSCI-B0 diagnostic — the "more info" for a scan that found nothing (e.g. the FR2355
/// Leaf investigation): idle bus levels + the eUSCI_B0 config/flags. Call after a Sensing scan (B0 is
/// the master then). Dual-only. Not a full diag — DESIGN.md.
#[cfg(all(feature = "debug", feature = "_dual"))]
pub fn debug_leaf(p: &Peripherals) {
    let (sda, scl) = crate::i2c::bus_levels(p);
    let ctlw0 = p.e_usci_b0.ucb0ctlw0().read().bits();
    let ifg = p.e_usci_b0.ucb0ifg().read().bits();
    crate::uart::puts(p, "leaf-dbg: SDA=");
    crate::uart::putc(p, if sda { b'H' } else { b'L' });
    crate::uart::puts(p, " SCL=");
    crate::uart::putc(p, if scl { b'H' } else { b'L' });
    crate::uart::puts(p, "  UCB0CTLW0=0x");
    crate::uart::hex8(p, (ctlw0 >> 8) as u8);
    crate::uart::hex8(p, ctlw0 as u8);
    crate::uart::puts(p, " (MST="); // UCMST (master) bit 0x0800
    crate::uart::putc(p, if ctlw0 & 0x0800 != 0 { b'1' } else { b'0' });
    crate::uart::puts(p, " SWRST="); // UCSWRST bit 0x0001 (held in reset = off the bus)
    crate::uart::putc(p, if ctlw0 & 0x0001 != 0 { b'1' } else { b'0' });
    crate::uart::puts(p, ")  UCB0IFG=0x"); // bit5 UCNACKIFG = last addr NACKed (no device)
    crate::uart::hex8(p, (ifg >> 8) as u8);
    crate::uart::hex8(p, ifg as u8);
    crate::uart::puts(p, " (NACK=");
    crate::uart::putc(p, if ifg & 0x0020 != 0 { b'1' } else { b'0' });
    crate::uart::puts(p, ")\n");
}
