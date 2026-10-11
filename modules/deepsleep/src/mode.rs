//! Operating modes (DESIGN.md "Operating modes") — SKELETON.
//!
//! A build compiles in the modes it supports (`mode-passive` / `mode-sensing` features); the active
//! mode is a **runtime switch** among them. The B1 slave surface (`stem.rs`) runs in EVERY mode —
//! modes change only the **sensor-bus** behaviour. Switching is an ACTION, not just a flag:
//!   • **→ Sensing**: acquire the sensor bus (bring up eUSCI_B0 as master), then monitor.
//!   • **→ Passive**: release the sensor bus (hold B0 in reset, tri-state its pins) so the SoM owns it.
//!
//! What's scaffolded here: the mode set, the boot default, and the transition machinery with the
//! bus **acquire/release** handoff. What's still open (DESIGN.md): the PMIC wake signal + the
//! monitor CONDITIONS that raise it, the SoM-facing mode register, and FRAM persistence of the default.

#![allow(dead_code)] // some paths are unused depending on which mode features are compiled in.

use stembus::regmap; // shared mode-code constants (the regmap register-file is gone)
use node_hal::LeafBus;

// Sensor-bus acquire/release is the host-clean `node_hal::LeafBus::{acquire,release}` seam (Phase 6b):
// no concrete PAC/`bsp` type here, no raw P1/eUSCI pokes. The MSP430 impl (`bsp::StemHw`) delegates to
// `bsp::board::leaf_i2c_{acquire,release}`; the pin/instance specifics stay in bsp.

/// The MSP's operating posture. A variant exists only for a mode that was compiled in.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    #[cfg(feature = "mode-passive")]
    Passive,
    #[cfg(feature = "mode-sensing")]
    Sensing,
}

impl Mode {
    /// Boot default. TODO(open-q, DESIGN.md): FRAM (`INIT_CODE`) persistence of the real default.
    /// **Passive** by default: at power-on the SoM/host is active and owns the buses, so the MSP stays
    /// an I/O-extender (OFF the sensor bus) until the SoM commands Sensing (when it later suspends).
    /// This also prevents a boot-time **two-master fight** on the sensor bus (the MSP grabbing it while
    /// another master is attached — the bench failure mode). Falls back to Sensing only for a
    /// sensing-only build (no `mode-passive`).
    pub const fn default_boot() -> Self {
        #[cfg(feature = "mode-passive")]
        {
            Mode::Passive
        }
        #[cfg(not(feature = "mode-passive"))]
        {
            Mode::Sensing
        }
    }

    /// The other built-in mode (for the bench `m` toggle). If only one mode is built, returns self.
    pub fn toggled(self) -> Self {
        #[cfg(all(feature = "mode-passive", feature = "mode-sensing"))]
        {
            match self {
                Mode::Passive => Mode::Sensing,
                Mode::Sensing => Mode::Passive,
            }
        }
        #[cfg(not(all(feature = "mode-passive", feature = "mode-sensing")))]
        {
            self
        }
    }

    pub fn is_sensing(self) -> bool {
        #[cfg(feature = "mode-sensing")]
        {
            matches!(self, Mode::Sensing)
        }
        #[cfg(not(feature = "mode-sensing"))]
        {
            false
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            #[cfg(feature = "mode-passive")]
            Mode::Passive => "passive",
            #[cfg(feature = "mode-sensing")]
            Mode::Sensing => "sensing",
        }
    }

    /// Stable wire ABI code for the SoM-facing mode register (`regmap::MODE_CTRL`, 0x3E). Independent
    /// of which variants were compiled in, so the SoM reads consistent numbers across build variants.
    pub fn code(self) -> u8 {
        match self {
            #[cfg(feature = "mode-passive")]
            Mode::Passive => regmap::MODE_CODE_PASSIVE,
            #[cfg(feature = "mode-sensing")]
            Mode::Sensing => regmap::MODE_CODE_SENSING,
        }
    }

    /// Decode a wire ABI code to a mode — but ONLY if that mode was compiled into this image. Returns
    /// `None` for an unknown code or a mode this image doesn't support, so a SoM request for an absent
    /// mode is safely ignored rather than mis-switched.
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            #[cfg(feature = "mode-passive")]
            regmap::MODE_CODE_PASSIVE => Some(Mode::Passive),
            #[cfg(feature = "mode-sensing")]
            regmap::MODE_CODE_SENSING => Some(Mode::Sensing),
            _ => None,
        }
    }
}

/// Runtime mode machine: holds the current mode and performs the entry/exit bus actions on a switch.
pub struct ModeMachine {
    current: Mode,
}

impl ModeMachine {
    /// Boot into the default mode, performing its entry action (acquire the bus for Sensing; ensure
    /// released for Passive).
    pub fn boot<H: LeafBus>(hw: &H) -> Self {
        let m = Mode::default_boot();
        enter(hw, m);
        Self { current: m }
    }

    pub fn current(&self) -> Mode {
        self.current
    }
    pub fn is_sensing(&self) -> bool {
        self.current.is_sensing()
    }

    /// Switch to `new` (no-op if already there). Sequenced handoff: **exit the old mode's bus role
    /// before entering the new one's**, so the sensor bus never has two masters.
    pub fn switch<H: LeafBus>(&mut self, hw: &H, new: Mode) {
        if new == self.current {
            return;
        }
        exit(hw, self.current);
        enter(hw, new);
        self.current = new;
    }
}

/// Entry action for a mode.
fn enter<H: LeafBus>(hw: &H, m: Mode) {
    match m {
        #[cfg(feature = "mode-passive")]
        Mode::Passive => release_sensor_bus(hw), // make sure we're OFF the bus (SoM owns it)
        #[cfg(feature = "mode-sensing")]
        Mode::Sensing => acquire_sensor_bus(hw),
    }
}

/// Exit action (symmetric): leaving Sensing releases the bus; leaving Passive does nothing.
fn exit<H: LeafBus>(hw: &H, m: Mode) {
    match m {
        #[cfg(feature = "mode-passive")]
        Mode::Passive => {
            let _ = hw;
        }
        #[cfg(feature = "mode-sensing")]
        Mode::Sensing => release_sensor_bus(hw),
    }
}

/// ACQUIRE the sensor bus: route P1.2/P1.3 to eUSCI_B0 and bring it up as master (with the stuck-bus
/// recovery in `i2c::init`). "Attempt" (per the canonical statement): if a slave holds SDA the
/// recovery runs; a persistently wedged bus surfaces as a scan fault, it does not fight a 2nd master.
#[cfg(feature = "mode-sensing")]
fn acquire_sensor_bus<H: LeafBus>(hw: &H) {
    hw.acquire(); // route P1.2/3 → UCB0 + bring up the master (with stuck-bus recovery) via LeafBus
}

/// RELEASE the sensor bus: hold eUSCI_B0 in reset and return P1.2/P1.3 to GPIO inputs (high-Z), so
/// the SoM (or nobody) owns the bus. Idempotent — safe to call at boot before B0 was ever brought up.
fn release_sensor_bus<H: LeafBus>(hw: &H) {
    hw.release(); // driver reset + tri-state the pins (chip specifics in bsp::board) via LeafBus
}
