//! The **command vocabulary** — the semantic command/response types shared by every participant
//! (`docs/COMMANDS.md`). This is the *meaning* layer, deliberately **decoupled from the wire encoding**
//! (cros_ec / postcard / regmap — deferred, COMMANDS.md §0): these are plain Rust types with no transport
//! or serialization baked in, so the choice of codec can be made/swapped late without touching the
//! command set. Pure logic, `no_std`, no alloc — compiles for msp430 / cortex-m / std alike.
//!
//! v0 scope = the `deepsleep` worked example (COMMANDS.md §5.7): introspection + operation. Debugging
//! and the row-based firmware-update (`FwUpdate`, §3) are added as the other sections land.

use crate::mode::Mode;

/// Why an MCU woke the SoM — a **hardcoded enum byte** shared SoM↔MCU (COMMANDS.md §5.6). Read via the
/// `Status` response (`WakeAttribution`): the SoM scans every node and the non-`None` one is the culprit,
/// *and* says why. Extend per each MCU's real triggers; the numbers are the wire contract, keep them stable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum WakeReason {
    None = 0,          // this node did not wake the SoM
    SoundTrigger = 1,  // analog/eCOMP threshold (e.g. presence/sound)
    ThresholdCross = 2, // a configured SetThreshold level crossed
    GpioEdge = 3,      // a sideband/trigger GPIO edge
    Timer = 4,         // periodic/RTC wake
    Fault = 5,         // a fault condition
    Unknown = 0xFF,    // reader saw a code it doesn't know
}

impl WakeReason {
    pub const fn code(self) -> u8 {
        self as u8
    }
    pub const fn from_code(c: u8) -> Self {
        match c {
            0 => WakeReason::None,
            1 => WakeReason::SoundTrigger,
            2 => WakeReason::ThresholdCross,
            3 => WakeReason::GpioEdge,
            4 => WakeReason::Timer,
            5 => WakeReason::Fault,
            _ => WakeReason::Unknown,
        }
    }
    /// Did this node wake the SoM? (any reason but `None`).
    pub const fn woke(self) -> bool {
        !matches!(self, WakeReason::None)
    }
}

/// The kind of a recorded [`Event`] (COMMANDS.md §5.6) — the `type` field. Trigger events reuse the
/// `WakeReason` space conceptually; `id` disambiguates *which* trigger/threshold.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum EventKind {
    Trigger = 0,       // a trigger fired (id = which; see WakeReason-style ids)
    ThresholdCross = 1, // a SetThreshold level crossed (id = threshold id; flag = direction)
    ModeChange = 2,    // operating mode changed (id = new mode code)
    Fault = 3,         // a fault was raised (id = fault code)
    Unknown = 0xFF,
}

impl EventKind {
    pub const fn code(self) -> u8 {
        self as u8
    }
    pub const fn from_code(c: u8) -> Self {
        match c {
            0 => EventKind::Trigger,
            1 => EventKind::ThresholdCross,
            2 => EventKind::ModeChange,
            3 => EventKind::Fault,
            _ => EventKind::Unknown,
        }
    }
}

/// One recorded event — a fixed **4 bytes** on the wire: `{ kind, id, flag, seq }` (COMMANDS.md §5.6).
/// `seq` is a sequence number (NO clock/RTC); ordering is by read-drain, not timestamp.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Event {
    pub kind: u8, // EventKind::code()
    pub id: u8,   // which trigger / threshold / mode / fault
    pub flag: u8, // direction / aux
    pub seq: u8,  // monotonic sequence (wraps)
}

impl Event {
    pub const fn new(kind: EventKind, id: u8, flag: u8, seq: u8) -> Self {
        Self { kind: kind.code(), id, flag, seq }
    }
    pub const fn to_bytes(self) -> [u8; 4] {
        [self.kind, self.id, self.flag, self.seq]
    }
    pub const fn from_bytes(b: [u8; 4]) -> Self {
        Self { kind: b[0], id: b[1], flag: b[2], seq: b[3] }
    }
}

/// Depth of the per-node event ring (COMMANDS.md §5.6: "16 deep, drop-oldest + overflowed bit").
pub const EVENT_RING_LEN: usize = 16;

/// A node's recent-event ring (the data behind `EventHistory`). **Drop-oldest + `overflowed` flag**, so a
/// node that fills the ring while the SoM is asleep tells the SoM (on `drain`) that it missed some.
/// Ordering is sequence-numbered, read by **draining** (SoM reads → gets all since last read → cleared).
/// `no_std`, no alloc — a fixed buffer owned by the node. Generic over `N`; use [`EventRing::new`].
pub struct EventRing<const N: usize> {
    buf: [Event; N],
    head: usize, // index of the OLDEST event
    len: usize,  // number of valid events
    next_seq: u8,
    overflowed: bool,
}

impl<const N: usize> EventRing<N> {
    pub const fn new() -> Self {
        Self { buf: [Event { kind: 0, id: 0, flag: 0, seq: 0 }; N], head: 0, len: 0, next_seq: 0, overflowed: false }
    }

    /// Record an event (assigns the next `seq`). Drops the OLDEST if full and sets `overflowed`.
    pub fn push(&mut self, kind: EventKind, id: u8, flag: u8) {
        let ev = Event::new(kind, id, flag, self.next_seq);
        self.next_seq = self.next_seq.wrapping_add(1);
        if self.len < N {
            let idx = (self.head + self.len) % N;
            self.buf[idx] = ev;
            self.len += 1;
        } else {
            // full → overwrite oldest, advance head, flag the loss
            self.buf[self.head] = ev;
            self.head = (self.head + 1) % N;
            self.overflowed = true;
        }
    }

    /// Drain oldest→newest into `out`, clearing the ring. Returns `(count_written, overflowed)` and
    /// resets `overflowed`. If `out` is shorter than `len`, the remainder is dropped (and real code
    /// should size `out` ≥ `N`); the returned count reflects what was written.
    pub fn drain(&mut self, out: &mut [Event]) -> (usize, bool) {
        let n = core::cmp::min(self.len, out.len());
        for (i, slot) in out.iter_mut().enumerate().take(n) {
            *slot = self.buf[(self.head + i) % N];
        }
        let overflowed = self.overflowed;
        self.head = 0;
        self.len = 0;
        self.overflowed = false;
        (n, overflowed)
    }

    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub const fn overflowed(&self) -> bool {
        self.overflowed
    }
}

impl<const N: usize> Default for EventRing<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// The semantic command set — what a caller (SoM / PD / dev via `hwd`) sends to a node (COMMANDS.md §5.7).
/// Wire-agnostic: a codec (deferred, §0) turns bytes ↔ `Command`/`Response`; a node's `execute` maps
/// `Command` → `Response` against its state. v0 = introspection + operation (the `deepsleep` set).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    /// Introspection: identity (iface magic, model, role, build-id, version).
    Identify,
    /// Introspection: status flags, fault, dev-count, current mode, `WakeReason`.
    Status,
    /// Introspection: per-sector FRAM CRC16 (windowed) — `sector` selects which (ADR-0010).
    FramCrc { sector: u8 },
    /// Introspection/operation: drain the recent-event ring (`EventHistory`).
    EventHistory,
    /// Operation: request an operating-mode switch (Passive/Sensing).
    SetMode { mode: Mode },
    /// Operation: set a sensor/trigger threshold — `id` is the node's threshold-catalog index.
    SetThreshold { id: u8, value: u16 },
    // TODO (other sections, COMMANDS.md): Scan/Dump/SelfTest (debugging, feature-gated);
    // FwUpdate row-stream (flashing, §3).
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_reason_round_trips() {
        for c in [0u8, 1, 2, 3, 4, 5, 9, 0xFF] {
            let r = WakeReason::from_code(c);
            // known codes round-trip; unknown maps to Unknown (0xFF)
            if matches!(r, WakeReason::Unknown) {
                assert!(c == 0xFF || c > 5);
            } else {
                assert_eq!(r.code(), c);
            }
        }
        assert!(!WakeReason::None.woke());
        assert!(WakeReason::SoundTrigger.woke());
    }

    #[test]
    fn ring_fills_and_drains_in_order() {
        let mut r: EventRing<16> = EventRing::new();
        for i in 0..5 {
            r.push(EventKind::Trigger, i, 0);
        }
        assert_eq!(r.len(), 5);
        assert!(!r.overflowed());
        let mut out = [Event::default(); 16];
        let (n, of) = r.drain(&mut out);
        assert_eq!(n, 5);
        assert!(!of);
        // oldest→newest, seq 0..5
        for i in 0..5 {
            assert_eq!(out[i].seq, i as u8);
            assert_eq!(out[i].id, i as u8);
        }
        assert!(r.is_empty());
    }

    #[test]
    fn ring_overflow_drops_oldest_and_flags() {
        let mut r: EventRing<16> = EventRing::new();
        for i in 0..20u8 {
            r.push(EventKind::Trigger, i, 0);
        }
        assert_eq!(r.len(), 16);
        assert!(r.overflowed());
        let mut out = [Event::default(); 16];
        let (n, of) = r.drain(&mut out);
        assert_eq!(n, 16);
        assert!(of);
        // first 4 (seq 0..4) dropped; oldest retained is seq 4 (id 4)
        assert_eq!(out[0].seq, 4);
        assert_eq!(out[15].seq, 19);
        assert!(!r.overflowed()); // cleared on drain
    }

    #[test]
    fn event_bytes_round_trip() {
        let e = Event::new(EventKind::ThresholdCross, 2, 1, 7);
        assert_eq!(Event::from_bytes(e.to_bytes()), e);
        assert_eq!(e.to_bytes(), [EventKind::ThresholdCross.code(), 2, 1, 7]);
    }
}
