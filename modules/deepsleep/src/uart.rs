//! Backchannel UART, 9600 8N1 — the report channel (+ RX for the on-demand command trigger).
//! All families delegate to `bsp::board::console_uart_*` — board-aware: UCA0/P1.4-5 on FR2476 *and*
//! FR2433, UCA1/P4.2-3 on FR2355 (the reason FR2355's console was silent before). The fr2433 console
//! now lives in `bsp::board::fr2433_launchpad` (no longer inline here) — CONV-1 done (ADR-0014). Text
//! formatting (puts/hex/dec/fixed2) is shared via `bsp::fmt`.

use crate::pac::Peripherals;

// Shared formatters over board::console_uart_tx — putc, puts, hex8, hex16, dec, dec_i16, fixed2.
// One copy with diag (was duplicated here). Which formatters a given build actually calls varies by
// feature (console/debug, and whether the diag crate is linked), so the glob may be wholly unused in a
// lean build — re-export it regardless (the unused names are LTO-stripped); silence the glob lint.
#[allow(unused_imports)]
pub use bsp::fmt::*;

pub fn init(p: &Peripherals) {
    bsp::board::console_uart_init(p);
}

/// A byte is waiting in the RX buffer (non-blocking) — used to poll for the on-demand trigger.
pub fn rx_ready(p: &Peripherals) -> bool {
    bsp::board::console_uart_rx_ready(p)
}

/// Read the pending RX byte (reading RXBUF clears UCRXIFG). Call only when `rx_ready`.
pub fn getc(p: &Peripherals) -> u8 {
    bsp::board::console_uart_getc(p)
}
