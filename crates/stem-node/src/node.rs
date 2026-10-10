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

/// A **live** node for the dual families (FR2355/FR2476): identity is static, but `status` is
/// **refreshed** each loop pass (enumeration/mode change it at runtime), and `set_mode` **records a
/// request** the node's `run_dual` loop drains and applies through the real mode machine (the switch
/// does sensor-bus acquire/release, which must not run inside dispatch — same deferral the regmap
/// `MODE_CTRL` used). `drain_events`/`set_threshold` stay stubbed until a real source/catalog exists.
pub struct LiveNode {
    identity: Identity,
    status: StatusData,
    crc: [u8; DBG_CRC_BYTES],
    mode_request: Option<u8>,
    skip_diag: bool,
}

impl LiveNode {
    pub fn new(identity: Identity, status: StatusData, crc: [u8; DBG_CRC_BYTES]) -> Self {
        Self { identity, status, crc, mode_request: None, skip_diag: false }
    }
    /// Refresh the status served by `Status`/introspection (call each loop pass, or on change).
    pub fn set_status(&mut self, status: StatusData) {
        self.status = status;
    }
    /// Take any pending SoM-requested mode code (from a `SetMode` command), clearing it. The loop decodes
    /// it via `mode::Mode::from_code` and applies the switch through the mode machine.
    pub fn take_mode_request(&mut self) -> Option<u8> {
        self.mode_request.take()
    }
    /// Take the pending **skip-diag** request (a master sent `SkipDiag`), clearing it. The boot-time diag
    /// window polls this each pass to decide whether to omit the extended POST (DESIGN.md "diag").
    pub fn take_skip_request(&mut self) -> bool {
        core::mem::take(&mut self.skip_diag)
    }
}

impl Node for LiveNode {
    fn identity(&self) -> Identity {
        self.identity
    }
    fn status(&self) -> StatusData {
        self.status
    }
    fn fram_crc(&self, sector: u8) -> u8 {
        let i = sector as usize;
        if i < self.crc.len() {
            self.crc[i]
        } else {
            0
        }
    }
    fn drain_events(&mut self, _out: &mut [Event]) -> (usize, bool) {
        (0, false)
    }
}

impl Operate for LiveNode {
    /// Record the mode-switch request; report the CURRENT mode (the switch is applied asynchronously by
    /// the loop, which re-publishes status after acquire/release). Matches the regmap `MODE_CTRL` deferral.
    fn set_mode(&mut self, mode: Mode) -> u8 {
        self.mode_request = Some(mode.code());
        self.status.mode
    }
    fn set_threshold(&mut self, _id: u8, _value: u16) -> bool {
        false
    }
    /// Record the skip-diag request; the boot diag window drains it via [`LiveNode::take_skip_request`].
    fn skip_diag(&mut self) {
        self.skip_diag = true;
    }
}
