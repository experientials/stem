//! Console text formatting — ONE copy shared by diag and prod (was duplicated byte-for-byte in both
//! `uart.rs` files). Integer-only, no float (MSP430). Every formatter bottoms out in
//! [`board::console_uart_tx`](crate::board), so it is board-aware for free across all families; the
//! chip-specific console instance/pins live in `bsp::board`. Consumers re-export these as their
//! `uart::{puts,hex8,…}` and keep only the chip-touching `init`/`rx_ready`/`getc` locally.

use crate::pac::Peripherals;

/// Emit one raw byte to the backchannel console (no newline translation — see [`puts`]).
#[inline]
pub fn putc(p: &Peripherals, c: u8) {
    crate::board::console_uart_tx(p, c);
}

/// Print a string, translating `\n` → `\r\n` for a raw serial terminal.
pub fn puts(p: &Peripherals, s: &str) {
    for &b in s.as_bytes() {
        if b == b'\n' {
            putc(p, b'\r');
        }
        putc(p, b);
    }
}

pub fn hex8(p: &Peripherals, v: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    putc(p, HEX[(v >> 4) as usize & 0xF]);
    putc(p, HEX[(v & 0xF) as usize]);
}

pub fn hex16(p: &Peripherals, v: u16) {
    hex8(p, (v >> 8) as u8);
    hex8(p, v as u8);
}

pub fn dec(p: &Peripherals, mut v: u16) {
    if v == 0 {
        putc(p, b'0');
        return;
    }
    let mut buf = [0u8; 5];
    let mut i = 0;
    while v > 0 {
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        putc(p, buf[i]);
    }
}

/// Signed decimal (`dec` is unsigned). Accel axes are ±8191, so `-v` never overflows i16.
pub fn dec_i16(p: &Peripherals, v: i16) {
    if v < 0 {
        putc(p, b'-');
        dec(p, (-v) as u16);
    } else {
        dec(p, v as u16);
    }
}

/// Signed hundredths as a fixed-point decimal, e.g. `-1234 -> "-12.34"`. For sensor values kept as
/// centi-units (no float on msp430): temperature in centi-°C, humidity in centi-%RH.
pub fn fixed2(p: &Peripherals, centi: i32) {
    let mut v = centi;
    if v < 0 {
        putc(p, b'-');
        v = -v;
    }
    dec(p, (v / 100) as u16); // integer part (a few hundred at most here) fits u16
    putc(p, b'.');
    let f = (v % 100) as u16;
    putc(p, b'0' + (f / 10) as u8);
    putc(p, b'0' + (f % 10) as u8);
}
