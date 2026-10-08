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
#[cfg_attr(feature = "postcard", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "postcard", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "postcard", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "postcard", derive(serde::Serialize, serde::Deserialize))]
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

/// Identity payload (the `Identify` response) — COMMANDS.md §5.7.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "postcard", derive(serde::Serialize, serde::Deserialize))]
pub struct Identity {
    pub iface: u8,     // iface magic (deepsleep: 0xD0)
    pub model: u16,    // silicon model id
    pub role: u8,      // role / Stem address
    pub build_id: u32, // per-image build stamp hash
    pub ver: [u8; 3],  // fw version major/minor/patch
}

/// Status payload (the `Status` response) — COMMANDS.md §5.7. `wake` is the `WakeAttribution` byte.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "postcard", derive(serde::Serialize, serde::Deserialize))]
pub struct StatusData {
    pub flags: u8,
    pub fault: u8,
    pub dev_count: u8,
    pub mode: u8, // current mode code
    pub wake: WakeReason,
}

/// The semantic result of [`execute`]. Bulk event data goes to the caller's scratch slice (see
/// [`execute`]); `Events` carries only the count + overflow flag so `Response` stays small + alloc-free.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "postcard", derive(serde::Serialize, serde::Deserialize))]
pub enum Response {
    Identity(Identity),
    Status(StatusData),
    Crc(u8),
    Events { n: usize, overflowed: bool },
    ModeNow(u8),
    Ack,
    Unsupported, // command not implemented by this node / invalid param
}

/// The **universal introspection seam** (COMMANDS.md §1/§5): the who/how/what-happened/integrity
/// surface that *every* MCU node shares — identity, status, event history, FRAM CRC. This is the one
/// genuinely cross-module trait; [`execute_core`] dispatches it. Hardware-free (plain data in/out) so it
/// — and dispatch — unit-test on the host with a mock, which is the whole reason it's a trait with a
/// single impl today: it's the pure boundary over each node's hardware-touching state.
///
/// **Operation / debug / flash commands are NOT here** — those are per-module (sensor nodes differ from a
/// bluetooth or vision node), expressed by a module-specific trait like [`Operate`]. Don't widen `Node`
/// with one module's operation surface; that was the original mistake (COMMANDS.md §1).
pub trait Node {
    fn identity(&self) -> Identity;
    fn status(&self) -> StatusData;
    /// One CRC byte for the windowed `FramCrc` read (ADR-0010); 0 if `sector` is out of range.
    fn fram_crc(&self, sector: u8) -> u8;
    /// Drain the event ring into `out`, oldest→newest; returns `(count, overflowed)` (COMMANDS.md §5.6).
    fn drain_events(&mut self, out: &mut [Event]) -> (usize, bool);
}

/// A **per-module operation surface** — the sensor-supervisor style (mode + thresholds) that `deepsleep`
/// has. **NOT universal**: a module implements this only if it fits (a bluetooth/vision node would have a
/// different operation trait, or none). Kept in `stembus` so the `Command → Response` *semantics* for
/// these arms (e.g. bad `id` → `Unsupported`) are pinned once and host-tested, not re-derived per module.
pub trait Operate {
    /// Request an operating-mode switch; return the mode code now reported.
    fn set_mode(&mut self, mode: Mode) -> u8;
    /// Set threshold `id` → `value`; `false` if `id` isn't in this node's catalog.
    fn set_threshold(&mut self, id: u8, value: u16) -> bool;
}

/// Dispatch the **universal introspection** arms of a [`Command`] against a [`Node`]. Returns `None` for
/// commands outside the introspection subset (operation/debug/flash) — those are a module's own concern
/// (compose via [`execute`], or a module's bespoke dispatch). **Wire-agnostic**: a codec (deferred, §0)
/// decodes bytes→`Command` and encodes `Response`→bytes around this. `events_out` is the caller's scratch
/// for `EventHistory` bulk data — size it ≥ [`EVENT_RING_LEN`].
pub fn execute_core<N: Node>(node: &mut N, cmd: Command, events_out: &mut [Event]) -> Option<Response> {
    match cmd {
        Command::Identify => Some(Response::Identity(node.identity())),
        Command::Status => Some(Response::Status(node.status())),
        Command::FramCrc { sector } => Some(Response::Crc(node.fram_crc(sector))),
        Command::EventHistory => {
            let (n, overflowed) = node.drain_events(events_out);
            Some(Response::Events { n, overflowed })
        }
        // operation/debug/flash — not the universal core's job
        Command::SetMode { .. } | Command::SetThreshold { .. } => None,
    }
}

/// Full dispatch for a node that is both introspectable ([`Node`]) and has the sensor-supervisor
/// [`Operate`] surface (`deepsleep` is both). Composes [`execute_core`] with the operation arms. A module
/// with a *different* operation surface does not use this — it calls [`execute_core`] and then dispatches
/// its own commands. (COMMANDS.md §5.7.)
pub fn execute<N: Node + Operate>(node: &mut N, cmd: Command, events_out: &mut [Event]) -> Response {
    if let Some(r) = execute_core(node, cmd, events_out) {
        return r;
    }
    match cmd {
        Command::SetMode { mode } => Response::ModeNow(node.set_mode(mode)),
        Command::SetThreshold { id, value } => {
            if node.set_threshold(id, value) {
                Response::Ack
            } else {
                Response::Unsupported
            }
        }
        // introspection already handled by execute_core
        _ => Response::Unsupported,
    }
}

/// postcard payload codec for the command vocabulary (COMMANDS.md §0 decision): the bytes carried inside
/// a cros_ec vendor command (`EC_CMD_BOARD_SPECIFIC_BASE + n`). Gated on the `postcard` feature so
/// size-sensitive builds (diag) opt out entirely; the FR2433 cros_ec build enables it. The SAME codec
/// runs on every Rust end (FR2433 EC, Pico/stem-probe master, M7) and backs the future Python wheel, so
/// no end can drift from another.
#[cfg(feature = "postcard")]
pub mod wire {
    use super::{Command, Response};
    pub use postcard::Error;

    /// Encode a `Command` into `out` (postcard); returns the byte length written.
    pub fn encode_command(cmd: &Command, out: &mut [u8]) -> Result<usize, Error> {
        Ok(postcard::to_slice(cmd, out)?.len())
    }
    /// Decode a `Command` from `bytes` (postcard).
    pub fn decode_command(bytes: &[u8]) -> Result<Command, Error> {
        postcard::from_bytes(bytes)
    }
    /// Encode a `Response` into `out` (postcard); returns the byte length written.
    pub fn encode_response(resp: &Response, out: &mut [u8]) -> Result<usize, Error> {
        Ok(postcard::to_slice(resp, out)?.len())
    }
    /// Decode a `Response` from `bytes` (postcard).
    pub fn decode_response(bytes: &[u8]) -> Result<Response, Error> {
        postcard::from_bytes(bytes)
    }
}

#[cfg(all(test, feature = "postcard"))]
mod wire_tests {
    use super::*;

    #[test]
    fn command_round_trips_through_postcard() {
        let cmds = [
            Command::Identify,
            Command::Status,
            Command::FramCrc { sector: 3 },
            Command::EventHistory,
            Command::SetMode { mode: Mode::Sensing },
            Command::SetThreshold { id: 1, value: 0x1234 },
        ];
        for cmd in cmds {
            let mut buf = [0u8; 32];
            let n = wire::encode_command(&cmd, &mut buf).unwrap();
            assert_eq!(wire::decode_command(&buf[..n]).unwrap(), cmd);
        }
    }

    #[test]
    fn response_round_trips_through_postcard() {
        let resps = [
            Response::Identity(Identity { iface: 0xD0, model: 0x8240, role: 0x22, build_id: 0xDEAD_BEEF, ver: [0, 1, 0] }),
            Response::Status(StatusData { flags: 1, fault: 0, dev_count: 0, mode: 0, wake: WakeReason::SoundTrigger }),
            Response::Crc(0xA5),
            Response::ModeNow(1),
            Response::Ack,
            Response::Unsupported,
            Response::Events { n: 3, overflowed: true },
        ];
        for r in resps {
            let mut buf = [0u8; 32];
            let n = wire::encode_response(&r, &mut buf).unwrap();
            assert_eq!(wire::decode_response(&buf[..n]).unwrap(), r);
        }
    }
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

    // A minimal in-memory Node to exercise `execute` on the host (stands in for a real MCU module).
    struct MockNode {
        ring: EventRing<16>,
        mode: u8,
        thresholds: [u16; 3], // ids 0..3 valid; others rejected
    }
    impl MockNode {
        fn new() -> Self {
            Self { ring: EventRing::new(), mode: Mode::Passive.code(), thresholds: [0; 3] }
        }
    }
    impl Node for MockNode {
        fn identity(&self) -> Identity {
            Identity { iface: 0xD0, model: 0x8240, role: 0x22, build_id: 0xDEADBEEF, ver: [0, 1, 0] }
        }
        fn status(&self) -> StatusData {
            StatusData { flags: 0x01, fault: 0, dev_count: 0, mode: self.mode, wake: WakeReason::SoundTrigger }
        }
        fn fram_crc(&self, sector: u8) -> u8 {
            if sector < 16 { sector ^ 0xA5 } else { 0 }
        }
        fn drain_events(&mut self, out: &mut [Event]) -> (usize, bool) {
            self.ring.drain(out)
        }
    }
    impl Operate for MockNode {
        fn set_mode(&mut self, mode: Mode) -> u8 {
            self.mode = mode.code();
            self.mode
        }
        fn set_threshold(&mut self, id: u8, value: u16) -> bool {
            if (id as usize) < self.thresholds.len() {
                self.thresholds[id as usize] = value;
                true
            } else {
                false
            }
        }
    }

    #[test]
    fn execute_core_declines_operation_commands() {
        // The universal core answers introspection and leaves operation/debug/flash to the module.
        let mut n = MockNode::new();
        let mut scratch = [Event::default(); EVENT_RING_LEN];
        assert!(execute_core(&mut n, Command::Identify, &mut scratch).is_some());
        assert!(execute_core(&mut n, Command::Status, &mut scratch).is_some());
        assert!(execute_core(&mut n, Command::EventHistory, &mut scratch).is_some());
        assert!(execute_core(&mut n, Command::FramCrc { sector: 0 }, &mut scratch).is_some());
        // operation arms are NOT the core's job
        assert!(execute_core(&mut n, Command::SetMode { mode: Mode::Sensing }, &mut scratch).is_none());
        assert!(execute_core(&mut n, Command::SetThreshold { id: 0, value: 1 }, &mut scratch).is_none());
    }

    #[test]
    fn execute_dispatches_each_command() {
        let mut n = MockNode::new();
        let mut scratch = [Event::default(); EVENT_RING_LEN];

        assert!(matches!(
            execute(&mut n, Command::Identify, &mut scratch),
            Response::Identity(Identity { iface: 0xD0, model: 0x8240, .. })
        ));
        assert!(matches!(
            execute(&mut n, Command::Status, &mut scratch),
            Response::Status(StatusData { wake: WakeReason::SoundTrigger, .. })
        ));
        assert_eq!(execute(&mut n, Command::FramCrc { sector: 3 }, &mut scratch), Response::Crc(3 ^ 0xA5));

        // SetMode changes the reported mode.
        assert_eq!(
            execute(&mut n, Command::SetMode { mode: Mode::Sensing }, &mut scratch),
            Response::ModeNow(Mode::Sensing.code())
        );

        // SetThreshold: valid id → Ack, out-of-catalog id → Unsupported.
        assert_eq!(execute(&mut n, Command::SetThreshold { id: 1, value: 0x1234 }, &mut scratch), Response::Ack);
        assert_eq!(n.thresholds[1], 0x1234);
        assert_eq!(execute(&mut n, Command::SetThreshold { id: 9, value: 0 }, &mut scratch), Response::Unsupported);

        // EventHistory drains what the node recorded.
        n.ring.push(EventKind::Trigger, 1, 0);
        n.ring.push(EventKind::ModeChange, Mode::Sensing.code(), 0);
        match execute(&mut n, Command::EventHistory, &mut scratch) {
            Response::Events { n: cnt, overflowed } => {
                assert_eq!(cnt, 2);
                assert!(!overflowed);
                assert_eq!(scratch[0].id, 1);
            }
            other => panic!("expected Events, got {other:?}"),
        }
    }
}
