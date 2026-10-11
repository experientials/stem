//! FRAM integrity (ADR-0010) — the CRC algorithm + the per-sector golden / readout / verify helpers,
//! shared by all participants:
//! - the **MSP430 node** computes [`sector_crcs`] over its live program FRAM and serves the bytes at
//!   `regmap::DBG_CRC`;
//! - a **master** ([`stem-probe`], the **M7**) reads them back with [`read_sector_crcs`];
//! - the **M7** computes the GOLDEN values with [`sector_crcs`] over the image it flashed, then
//!   [`mismatched_sectors`] tells it which sectors to reflash.
//!
//! The node only *reports*; the M7 *judges* — so a bug here can only cause a harmless reflash, never a
//! false "OK" (ADR-0010).

use crate::regmap::DBG_CRC_BYTES;

/// Number of sectors the program FRAM is split into — one `u16` CRC each (= `DBG_CRC_BYTES / 2`).
pub const SECTORS: usize = DBG_CRC_BYTES / 2;

/// CRC16-CCITT (polynomial 0x1021, init 0xFFFF, MSB-first). The ONE algorithm every participant uses;
/// the MSP node, the bench host, and the M7 must all produce identical values for a given byte range.
pub fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
            bit += 1;
        }
    }
    crc
}

/// Per-sector CRC16 over `image`, split into [`SECTORS`] equal sectors (`image.len()` should be a
/// multiple of `SECTORS` — the program-FRAM window is). The node runs this over live FRAM; the M7 runs
/// it over the firmware image it flashed to get the GOLDEN values. Returns `[sector0, …]`.
pub fn sector_crcs(image: &[u8]) -> [u16; SECTORS] {
    let sec = image.len() / SECTORS;
    let mut out = [0u16; SECTORS];
    let mut s = 0;
    while s < SECTORS {
        out[s] = crc16_ccitt(&image[s * sec..(s + 1) * sec]);
        s += 1;
    }
    out
}

/// Assemble the per-sector CRCs from a node's `DBG_CRC` (0x3F) window. The caller's `crc_byte(idx)`
/// performs one windowed round-trip for byte index `0..DBG_CRC_BYTES` — i.e. *write* `DBG_CRC = idx`
/// then *read* `DBG_CRC`. Transport-agnostic (a closure) so stem-probe (embassy I²C) and the M7
/// (Linux i2cdev) use it identically. Bytes are `[sec0_hi, sec0_lo, sec1_hi, …]`.
pub fn read_sector_crcs<E>(
    mut crc_byte: impl FnMut(u8) -> Result<u8, E>,
) -> Result<[u16; SECTORS], E> {
    let mut out = [0u16; SECTORS];
    let mut s = 0;
    while s < SECTORS {
        let hi = crc_byte((2 * s) as u8)?;
        let lo = crc_byte((2 * s + 1) as u8)?;
        out[s] = ((hi as u16) << 8) | lo as u16;
        s += 1;
    }
    Ok(out)
}

/// Bitmask of sectors whose CRC differs (bit `s` ⇒ sector `s` mismatched). The M7's judge: a nonzero
/// result means reflash those sectors. Compares up to the shorter slice length.
pub fn mismatched_sectors(golden: &[u16], actual: &[u16]) -> u16 {
    let n = if golden.len() < actual.len() { golden.len() } else { actual.len() };
    let mut mask = 0u16;
    let mut s = 0;
    while s < n {
        if golden[s] != actual[s] {
            mask |= 1 << s;
        }
        s += 1;
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_known_vector() {
        // CRC16-CCITT (0xFFFF init) of "123456789" is 0x29B1 (the standard check value).
        assert_eq!(crc16_ccitt(b"123456789"), 0x29B1);
    }

    #[test]
    fn mismatch_mask() {
        assert_eq!(mismatched_sectors(&[1, 2, 3], &[1, 9, 3]), 0b010);
        assert_eq!(mismatched_sectors(&[1, 2, 3], &[1, 2, 3]), 0);
    }
}
