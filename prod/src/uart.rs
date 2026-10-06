//! Backchannel UART, 9600 8N1 — the report channel (+ RX for the on-demand command trigger).
//! All families delegate to `bsp::board::console_uart_*` — board-aware: UCA0/P1.4-5 on FR2476 *and*
//! FR2433, UCA1/P4.2-3 on FR2355 (the reason FR2355's console was silent before). The fr2433 console
//! now lives in `bsp::board::fr2433_launchpad` (no longer inline here) — CONV-1 done (ADR-0014). Text
//! formatting (puts/hex/dec/fixed2) is shared via `bsp::fmt`.

use crate::pac::Peripherals;

pub fn init(p: &Peripherals) {
    bsp::board::console_uart_init(p);
}

pub fn putc(p: &Peripherals, c: u8) {
    bsp::board::console_uart_tx(p, c);
}

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

/// Print a hundredths value as a fixed-point `D.DD` (e.g. 2519 → "25.19"). Integer-only, no float.
/// Ported from diag/src/uart.rs — for the Si7021 T/RH centi values.
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

/// A byte is waiting in the RX buffer (non-blocking) — used to poll for the on-demand trigger.
pub fn rx_ready(p: &Peripherals) -> bool {
    bsp::board::console_uart_rx_ready(p)
}

/// Read the pending RX byte (reading RXBUF clears UCRXIFG). Call only when `rx_ready`.
pub fn getc(p: &Peripherals) -> u8 {
    bsp::board::console_uart_getc(p)
}
