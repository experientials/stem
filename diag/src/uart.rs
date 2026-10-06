//! Backchannel UART, 9600 8N1 — chip-agnostic text layer. The eUSCI instance (A0 on FR2476,
//! A1 on FR2355), its baud config, and the TX pins all live in `board`; the text formatters
//! (puts/putc/hex/dec/fixed2) are now shared in `bsp::fmt` (re-exported below — one copy with prod).
//! This file keeps only the chip-touching init/rx/getc board delegates. Pins routed in main via `board`.

use crate::pac::Peripherals;

// Shared formatters over board::console_uart_tx — putc, puts, hex8, hex16, dec, dec_i16, fixed2.
pub use bsp::fmt::*;

pub fn init(p: &Peripherals) {
    crate::board::console_uart_init(p);
}

/// A byte is waiting in the console RX buffer (non-blocking). Same shared board primitive prod uses
/// for its command console — so diag's menu is console-drivable (not only the S1 button).
pub fn rx_ready(p: &Peripherals) -> bool {
    crate::board::console_uart_rx_ready(p)
}

/// Read the pending RX byte (clears UCRXIFG). Call only when [`rx_ready`].
pub fn getc(p: &Peripherals) -> u8 {
    crate::board::console_uart_getc(p)
}
