//! Free-running millisecond clock — the time base the cooperative [`sched`] loop needs.
//!
//! The raw 16-bit tick source (TB0 free-running from ACLK/REFO, 32768 Hz) is the `board` role
//! `ms_timer_start`/`ms_timer_ticks` — so this module names the ROLE, never the timer instance
//! (chip specifics live in `board`, STEM-DIRECTION.md). We never take the timer interrupt; `now_ms`
//! just polls the raw ticks and folds the 16-bit delta into a 32-bit millisecond count. That keeps
//! the "no ISR" property of diag today (the port-interrupt / wake path is a later step) and is robust
//! to long tasks: the only requirement is that the loop reads the clock at least once per wrap (~2 s
//! at 32768 Hz), which the POST loop beats by orders of magnitude even while a full I²C scan runs.
//!
//! The delta is accumulated with a sub-millisecond carry so there is **no rounding drift**:
//! d ticks contribute `d * 1000 / 32768` ms exactly, remainder carried to the next call.

use crate::pac::Peripherals;

pub struct Clock {
    last: u16, // previous raw tick sample
    ms: u32,   // whole milliseconds since start()
    sub: u32,  // fractional-ms carry, in units of (1/32768 s), pre-scaled by 1000
}

impl Clock {
    /// Start the ms tick source and return a zeroed clock. ACLK must already be live (main's
    /// `clock_init_1mhz` routes ACLK to REFO before this is called).
    pub fn start(p: &Peripherals) -> Self {
        crate::board::ms_timer_start(p);
        Self { last: 0, ms: 0, sub: 0 }
    }

    /// Monotonic milliseconds since [`start`]. Cheap; call it every scheduler pass.
    ///
    /// [`start`]: Clock::start
    pub fn now_ms(&mut self, p: &Peripherals) -> u32 {
        let t = crate::board::ms_timer_ticks(p);
        let d = t.wrapping_sub(self.last) as u32; // 0..=65535 ticks since last read
        self.last = t;
        // d*1000 <= 6.55e7, plus a remainder < 32768 -> no u32 overflow.
        self.sub += d * 1000;
        self.ms += self.sub >> 15; // / 32768 -> whole ms
        self.sub &= 0x7FFF; // keep the sub-ms remainder (< 32768)
        self.ms
    }
}
