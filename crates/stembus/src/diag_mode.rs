//! Diag-mode codes — the continual/long-running diag test a `--diag` node runs, set by the `SetDiagMode`
//! command (DESIGN.md "CONTINUAL diag is a MODE controlled by command/register", Henrik 2026-10-11).
//! Shared so a master (SoM/stem-probe) sets the same code the node acts on. **DEFAULT = `Off`**: a
//! continual bus-hammering test left on would make the device unusable, so a node that has never been told
//! otherwise (and the `Unknown`/undefined case) runs normally.
//!
//! This is the *transport-neutral codec*; the node's `--diag` loop maps the active mode to the re-homed
//! diagnostic (margin sweep / soak / signature battery). Mirrors `mode::Mode` (operating mode) in shape.

/// The continual diag test selected on a node. `Unknown` carries the raw code (a mode this reader doesn't
/// know, or that the node doesn't implement) — treated as `Off` by the node (default-off safety).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(feature = "postcard", derive(serde::Serialize, serde::Deserialize))]
pub enum DiagMode {
    Off,       // default — node runs normally (no continual diag)
    Margin,    // I²C clock-margin sweep (bounded: 100 kHz → 1 MHz, verified reads)
    Soak,      // continual I²C error-rate soak (runs until disabled — cooperative)
    Signature, // I²C signature-pattern battery (bounded; emits scope patterns for a logic analyzer)
    Unknown(u8),
}

impl DiagMode {
    pub const fn from_code(code: u8) -> Self {
        match code {
            0 => DiagMode::Off,
            1 => DiagMode::Margin,
            2 => DiagMode::Soak,
            3 => DiagMode::Signature,
            other => DiagMode::Unknown(other),
        }
    }
    /// Wire code (inverse of [`from_code`]). `Unknown` returns its raw code.
    pub const fn code(self) -> u8 {
        match self {
            DiagMode::Off => 0,
            DiagMode::Margin => 1,
            DiagMode::Soak => 2,
            DiagMode::Signature => 3,
            DiagMode::Unknown(c) => c,
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            DiagMode::Off => "off",
            DiagMode::Margin => "margin",
            DiagMode::Soak => "soak",
            DiagMode::Signature => "signature",
            DiagMode::Unknown(_) => "?",
        }
    }
    /// Parse a CLI token (`off|margin|soak|signature`) → a mode. Returns `None` on an unrecognized token
    /// (the probe/host maps that to a usage error rather than silently selecting a mode).
    pub fn from_token(tok: &str) -> Option<Self> {
        match tok {
            "off" | "0" => Some(DiagMode::Off),
            "margin" | "m" | "1" => Some(DiagMode::Margin),
            "soak" | "s" | "2" => Some(DiagMode::Soak),
            "signature" | "sig" | "g" | "3" => Some(DiagMode::Signature),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DiagMode;

    #[test]
    fn code_roundtrips_and_default_off() {
        for m in [DiagMode::Off, DiagMode::Margin, DiagMode::Soak, DiagMode::Signature] {
            assert_eq!(DiagMode::from_code(m.code()), m);
        }
        // code 0 is the default (Off) — matches LiveNode's diag_mode: 0 init.
        assert_eq!(DiagMode::from_code(0), DiagMode::Off);
        // Unknown carries its raw code (round-trips), and is NOT any known mode.
        assert_eq!(DiagMode::from_code(0x7E), DiagMode::Unknown(0x7E));
        assert_eq!(DiagMode::Unknown(0x7E).code(), 0x7E);
    }

    #[test]
    fn from_token_parses_aliases_and_rejects_junk() {
        assert_eq!(DiagMode::from_token("off"), Some(DiagMode::Off));
        assert_eq!(DiagMode::from_token("margin"), Some(DiagMode::Margin));
        assert_eq!(DiagMode::from_token("s"), Some(DiagMode::Soak));
        assert_eq!(DiagMode::from_token("sig"), Some(DiagMode::Signature));
        assert_eq!(DiagMode::from_token("3"), Some(DiagMode::Signature));
        assert_eq!(DiagMode::from_token("bogus"), None);
    }
}
