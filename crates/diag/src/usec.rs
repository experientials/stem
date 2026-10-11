//! Microsecond time base for stress instrumentation — TA0 free-running off SMCLK (1 MHz), so TA0R
//! ticks at 1 µs and wraps every 65.536 ms. Used to time individual I²C transactions; every
//! measured interval is far under one wrap. Separate from the `clock` ms base (TB0/ACLK) because
//! transaction latencies are tens–hundreds of µs — below the 30.5 µs ACLK resolution.
//!
//! Only compiled into the `stress` build.

use crate::pac::Peripherals;

/// Start the µs timer counting continuously at 1 µs/tick. SMCLK must be 1 MHz (board clock_init).
/// The timer instance is board-specific (TA0 on FR2476; TB1 on FR2355, which has no Timer_A).
pub fn start(p: &Peripherals) {
    crate::board::usec_start(p);
}

/// Current µs counter (wraps at 65536). Diff two samples with `wrapping_sub` for an elapsed µs.
#[inline]
pub fn now(p: &Peripherals) -> u16 {
    crate::board::usec_now(p)
}

/// Busy-wait `us` microseconds (≤ 65535, i.e. under one TA0 wrap). Polls tightly so it never
/// misses the wrap. Used for sensor conversion waits (e.g. the Si7021 no-hold RH conversion).
pub fn delay_us(p: &Peripherals, us: u16) {
    let start = now(p);
    while now(p).wrapping_sub(start) < us {}
}

/// Busy-wait `ms` milliseconds, in 1 ms chunks so each stays under the 65.5 ms TA0 wrap.
pub fn delay_ms(p: &Peripherals, ms: u16) {
    for _ in 0..ms {
        delay_us(p, 1000);
    }
}
