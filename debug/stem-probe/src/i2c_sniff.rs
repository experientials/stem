//! Portable, `no_std`, HAL-agnostic passive I²C bus sniffer core.
//!
//! The decoder reads the two line levels through **closures**, so the same code drops into any MCU/HAL
//! — RP2040/Cortex-M0, M4, M7, … — without depending on embassy, a specific GPIO type, I/O, or heap.
//! The caller supplies `|| sda.is_high()` / `|| scl.is_high()` (or raw register reads) and owns both the
//! timing policy (when to yield) and the output transport (how to print the result). This file is the
//! reusable unit; wiring it to a given board's pins + console is the app's job (see `run_sniff` in main).
//!
//! Framing it reconstructs, purely from level sampling (no I²C peripheral):
//!   • START / STOP  = an SDA edge **while SCL is high**
//!   • data bit      = SDA sampled on each SCL **rising** edge (8 bits, MSB-first)
//!   • ACK bit       = the 9th rising edge (SDA **low** = ACK)
//! so it reveals what is actually on the wire vs what a master *thinks* it saw (e.g. a false-ACK that
//! lives in the master's sampling, not the bus).

/// One decoded bus event. The `bool` on a byte = the ACK sampled on the 9th clock (true = SDA low = ACK).
#[derive(Clone, Copy)]
pub enum Ev {
    Start,
    Stop,
    Byte(u8, bool),
}

/// Running tallies over a whole capture (full counts, even past the event ring's capacity).
#[derive(Clone, Copy)]
pub struct Stats {
    pub starts: u16,
    pub stops: u16,
    pub scl_edges: u16,
    pub bytes: u16,
    pub ack: u16,
    pub nak: u16,
}

impl Stats {
    pub const fn new() -> Self {
        Self { starts: 0, stops: 0, scl_edges: 0, bytes: 0, ack: 0, nak: 0 }
    }
}

/// On-the-fly I²C framing decoder. Seed with the idle levels, then feed simultaneous (scl, sda) samples.
pub struct Decoder {
    p_scl: bool,
    p_sda: bool,
    in_frame: bool,
    bits: u8,
    cur: u8,
}

impl Decoder {
    /// Seed with the current (idle) line levels so the first real edge isn't misread.
    pub fn new(scl: bool, sda: bool) -> Self {
        Self { p_scl: scl, p_sda: sda, in_frame: false, bits: 0, cur: 0 }
    }

    /// Feed one (scl, sda) sample; pushes at most one event into `log` (if room) and updates `stats`.
    #[inline(always)]
    pub fn step(&mut self, scl_now: bool, sda_now: bool, log: &mut [Ev], n: &mut usize, s: &mut Stats) {
        // START/STOP: an SDA transition while SCL stays high.
        if scl_now && self.p_scl {
            if self.p_sda && !sda_now {
                if *n < log.len() {
                    log[*n] = Ev::Start;
                    *n += 1;
                }
                self.in_frame = true;
                self.bits = 0;
                self.cur = 0;
                s.starts = s.starts.saturating_add(1);
            } else if !self.p_sda && sda_now {
                if *n < log.len() {
                    log[*n] = Ev::Stop;
                    *n += 1;
                }
                self.in_frame = false;
                self.bits = 0;
                self.cur = 0;
                s.stops = s.stops.saturating_add(1);
            }
        }
        // Data/ACK bit: sample SDA on each SCL rising edge.
        if scl_now && !self.p_scl {
            s.scl_edges = s.scl_edges.saturating_add(1);
            if self.in_frame {
                if self.bits < 8 {
                    self.cur = (self.cur << 1) | (sda_now as u8);
                    self.bits += 1;
                } else {
                    let ack = !sda_now;
                    if *n < log.len() {
                        log[*n] = Ev::Byte(self.cur, ack);
                        *n += 1;
                    }
                    s.bytes = s.bytes.saturating_add(1);
                    if ack {
                        s.ack = s.ack.saturating_add(1);
                    } else {
                        s.nak = s.nak.saturating_add(1);
                    }
                    self.bits = 0;
                    self.cur = 0;
                }
            }
        }
        self.p_scl = scl_now;
        self.p_sda = sda_now;
    }
}

/// Busy-poll until SCL shows a **sustained burst** — `min_edges` transitions within `max_iters` samples —
/// not just one edge. This rejects isolated glitches (e.g. a master's bus-recovery pulses or init toggles)
/// and arms only on real clocking (a scan is hundreds of edges). Returns true when armed, false if the
/// budget expired. `prev` carries the last level across slices. Call in short slices between USB yields so
/// the (possibly long) wait for traffic doesn't starve the host — then capture the burst gap-free.
pub fn wait_for_burst<FC: Fn() -> bool>(
    read_scl: FC,
    prev: &mut bool,
    max_iters: u32,
    min_edges: u16,
) -> bool {
    let mut i = 0u32;
    let mut edges = 0u16;
    while i < max_iters {
        i += 1;
        let scl = read_scl();
        if scl != *prev {
            *prev = scl;
            edges = edges.saturating_add(1);
            if edges >= min_edges {
                return true;
            }
        }
    }
    false
}

/// Tight **synchronous** capture — NO yielding, so a transaction burst is never fragmented (yield gaps
/// drop bits and desync the decode). Runs up to `iters` samples but self-terminates once at least one
/// event has been seen AND the bus then stays idle for `idle_limit` consecutive samples (so it returns
/// right after a scan ends instead of blocking for the full budget). Stops early if the ring fills.
pub fn capture_burst<FS: Fn() -> bool, FC: Fn() -> bool>(
    dec: &mut Decoder,
    read_sda: FS,
    read_scl: FC,
    log: &mut [Ev],
    n: &mut usize,
    s: &mut Stats,
    iters: u32,
    idle_limit: u32,
) {
    let mut i = 0u32;
    let mut idle = 0u32;
    while i < iters && *n < log.len() {
        i += 1;
        let scl = read_scl();
        let sda = read_sda();
        let before = s.scl_edges;
        dec.step(scl, sda, log, n, s);
        if s.scl_edges != before {
            idle = 0;
        } else {
            idle += 1;
            if *n > 0 && idle > idle_limit {
                return; // activity seen, then sustained idle → the burst is over
            }
        }
    }
}
