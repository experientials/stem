//! The `Node`+`Operate` adapter — the deepsleep implementation of the shared `stembus::command` seam
//! (COMMANDS.md §1 / firmware-architecture.md §11, the purity-seam trait). `command::execute` dispatches
//! the unified `Command` vocabulary against this; the vendor+postcard glue (`deepsleep::ec`) decodes a
//! `Command`, runs `execute`, and re-encodes the `Response`.
//!
//! [`StaticNode`] is the FR2433 case: its identity/status/CRC are **static after boot** (Passive-only, no
//! sensor bus → no mode machine, no runtime dev-count change, no event source, no threshold catalog yet),
//! so a boot-time snapshot is the honest, minimal implementation. The stubbed methods say so explicitly;
//! a live adapter (mode switches, events, thresholds) lands when the dual families / real sources exist.

use stembus::command::{Event, Identity, Node, Operate, StatusData};
use stembus::mode::Mode;
use stembus::regmap::DBG_CRC_BYTES;

/// A boot-snapshot node: serves fixed identity/status/CRC. Honest for FR2433 (static post-boot).
pub struct StaticNode {
    identity: Identity,
    status: StatusData,
    crc: [u8; DBG_CRC_BYTES],
}

impl StaticNode {
    pub fn new(identity: Identity, status: StatusData, crc: [u8; DBG_CRC_BYTES]) -> Self {
        Self { identity, status, crc }
    }
}

impl Node for StaticNode {
    fn identity(&self) -> Identity {
        self.identity
    }
    fn status(&self) -> StatusData {
        self.status
    }
    /// Per-sector CRC byte (windowed readout, ADR-0010): `sector` indexes the `DBG_CRC_BYTES` array.
    fn fram_crc(&self, sector: u8) -> u8 {
        let i = sector as usize;
        if i < self.crc.len() {
            self.crc[i]
        } else {
            0
        }
    }
    /// No event source on FR2433 yet → always empty. (The event ring lands with a real trigger source.)
    fn drain_events(&mut self, _out: &mut [Event]) -> (usize, bool) {
        (0, false)
    }
}

impl Operate for StaticNode {
    /// FR2433 is Passive-only (no mode machine) → no switch; report the current mode unchanged.
    fn set_mode(&mut self, _mode: Mode) -> u8 {
        self.status.mode
    }
    /// No threshold catalog on FR2433 yet → unsupported.
    fn set_threshold(&mut self, _id: u8, _value: u16) -> bool {
        false
    }
}
