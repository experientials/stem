//! Operating-mode codes (`regmap::MODE_CTRL` 0x3E) — shared so a master interprets a node's mode the
//! same way the node reports it. (The node's `prod::mode` keeps its feature-gated variant machine; this
//! is the transport-neutral codec every reader uses.)

use crate::regmap::{MODE_CODE_PASSIVE, MODE_CODE_SENSING};

/// The operating mode a `MODE_CTRL` byte encodes. `Unknown` carries the raw code (a mode this reader
/// doesn't know, or that the node doesn't support).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "postcard", derive(serde::Serialize, serde::Deserialize))]
pub enum Mode {
    Passive, // SoM owns the buses; the node is an I/O-extender, off the sensor bus
    Sensing, // SoM suspended; the node masters + monitors the sensor bus
    Unknown(u8),
}

impl Mode {
    pub const fn from_code(code: u8) -> Self {
        match code {
            MODE_CODE_PASSIVE => Mode::Passive,
            MODE_CODE_SENSING => Mode::Sensing,
            other => Mode::Unknown(other),
        }
    }
    /// Wire code for this mode (inverse of [`from_code`]). `Unknown` returns its raw code.
    pub const fn code(self) -> u8 {
        match self {
            Mode::Passive => MODE_CODE_PASSIVE,
            Mode::Sensing => MODE_CODE_SENSING,
            Mode::Unknown(c) => c,
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Mode::Passive => "passive",
            Mode::Sensing => "sensing",
            Mode::Unknown(_) => "?",
        }
    }
}

/// Decode a `MODE_CTRL` byte straight to a name (convenience for logging/dumps).
pub const fn mode_name(code: u8) -> &'static str {
    Mode::from_code(code).name()
}
