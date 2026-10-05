//! Debug / identity / status — the state model behind the `regmap` debug window (0x30–0x3F).
//!
//! ONE model, TWO readers: [`Status::read_reg`] is the single source the eUSCI_B1 I2C-slave ISR
//! (the SoM, later) and the bench [`Status::dump`] (UART, `console`-gated) both read — so the SoM and
//! the bench see identical bytes. The model + `read_reg` are ALWAYS built (they feed the I2C-slave
//! surface); only the UART dump is `console`-gated. Populated from the sensor-bus enumeration + the
//! compiled build stamp + the detected model. Additive to the PCA9698 facade (nothing touches 0x00–0x2A).

#![allow(dead_code)] // some fields/paths serve the SoM's I2C-slave reads (stem::RegFile) not the bench dump.

#[cfg(feature = "_dual")]
use crate::enumerate::Scan;
use crate::model::Model;
use crate::{regmap, FW_BUILD, FW_VER_MAJOR, FW_VER_MINOR, FW_VER_PATCH};
#[cfg(feature = "console")]
use crate::uart;
#[cfg(feature = "console")]
use crate::pac::Peripherals;

// `DBG_STATUS` (0x37) / `DBG_FAULT` (0x3A) flag bits — the shared definitions (one source: the
// `stembus::flags` codec, so a master decodes exactly what this server sets). Re-exported under the
// existing `flags::` / `fault::` paths so the call sites below are unchanged.
pub use stembus::flags::{fault, status as flags};

/// The debug/status snapshot exposed at regmap 0x30–0x3F.
pub struct Status {
    model_id: u16,
    build_id: u32,
    status: u8,
    dev_count: u8,
    known_present: u8,
    fault: u8,
    mode: u8, // current operating-mode ABI code (regmap::MODE_CODE_*), reported at MODE_CTRL (0x3E)
}

/// FNV-1a 32-bit hash of the build stamp → a stable, machine-readable build id the SoM/hwd can match
/// against what it flashed (verify-by-stamp without parsing the string).
fn build_id() -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in FW_BUILD.as_bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

impl Status {
    /// Assemble the snapshot from the model + a sensor-bus scan. A faulted (wedged) scan sets the
    /// fault flag and withholds BUS_OK + the (bogus) presence data, so the SoM sees an honest fault
    /// rather than 112 phantom devices. DUAL-only: the single-I²C fr24xx (Passive) node never masters
    /// a sensor bus, so it publishes `booted()` instead.
    #[cfg(feature = "_dual")]
    pub fn from_scan(model: Model, scan: &Scan) -> Self {
        let mut status = flags::BOOTED | flags::ENUMERATED;
        let mut fault = 0u8;
        if !model.matches_build_family() {
            status |= flags::WRONG_FAMILY;
        }
        if scan.faulted {
            fault |= fault::BUS_WEDGED;
            return Self {
                model_id: model.device_id(),
                build_id: build_id(),
                status,
                dev_count: 0,
                known_present: 0,
                fault,
                mode: regmap::MODE_CODE_UNKNOWN, // main sets the live code via set_mode()
            };
        }
        status |= flags::BUS_OK;
        // known-present bitmap: bit i set if devices::KNOWN[i] ACKed on the last (clean) scan.
        let mut known = 0u8;
        for (i, k) in devices::KNOWN.iter().enumerate() {
            if i < 8 && scan.present.get(k.addr) {
                known |= 1 << i;
            }
        }
        Self {
            model_id: model.device_id(),
            build_id: build_id(),
            status,
            dev_count: scan.present.count() as u8,
            known_present: known,
            fault,
            mode: regmap::MODE_CODE_UNKNOWN, // main sets the live code via set_mode()
        }
    }

    /// Booted, but the sensor bus was NOT scanned — used in **Passive** mode, where the MSP never
    /// masters the sensor bus. BOOTED only (no ENUMERATED / BUS_OK), so the SoM doesn't read a false
    /// enumeration. Identity/build still populated.
    pub fn booted(model: Model) -> Self {
        let mut status = flags::BOOTED;
        if !model.matches_build_family() {
            status |= flags::WRONG_FAMILY;
        }
        Self {
            model_id: model.device_id(),
            build_id: build_id(),
            status,
            dev_count: 0,
            known_present: 0,
            fault: 0,
            mode: regmap::MODE_CODE_UNKNOWN, // main sets the live code via set_mode()
        }
    }

    /// Set the current operating-mode code reported at `MODE_CTRL` (0x3E). Called by main after a mode
    /// switch (and at boot) so the bench dump and the SoM read the SAME live mode.
    pub fn set_mode(&mut self, code: u8) {
        self.mode = code;
    }

    /// Read one debug register (0x30–0x3F). The shared read path for the slave ISR and the UART dump;
    /// reserved offsets read 0.
    pub fn read_reg(&self, reg: u8) -> u8 {
        match reg {
            regmap::DBG_IFACE => regmap::DBG_IFACE_MAGIC,
            regmap::DBG_MODEL_L => self.model_id as u8,
            regmap::DBG_MODEL_H => (self.model_id >> 8) as u8,
            regmap::DBG_BUILD_0 => self.build_id as u8,
            regmap::DBG_BUILD_1 => (self.build_id >> 8) as u8,
            regmap::DBG_BUILD_2 => (self.build_id >> 16) as u8,
            regmap::DBG_BUILD_3 => (self.build_id >> 24) as u8,
            regmap::DBG_STATUS => self.status,
            regmap::DBG_DEV_COUNT => self.dev_count,
            regmap::DBG_KNOWN_PRESENT => self.known_present,
            regmap::DBG_FAULT => self.fault,
            regmap::DBG_FW_VER_MAJOR => FW_VER_MAJOR,
            regmap::DBG_FW_VER_MINOR => FW_VER_MINOR,
            regmap::DBG_FW_VER_PATCH => FW_VER_PATCH,
            regmap::MODE_CTRL => self.mode,
            _ => 0,
        }
    }

    /// Bench dump of the debug window over UART — the escape hatch until an I2C master reads it.
    /// Prints the raw 0x30–0x3F bytes (exactly what the SoM would read) plus a decoded one-liner.
    #[cfg(feature = "console")]
    pub fn dump(&self, p: &Peripherals) {
        uart::puts(p, "dbg 30-3F:");
        let mut r = regmap::EXT_BASE;
        while r <= regmap::EXT_END {
            uart::putc(p, b' ');
            uart::hex8(p, self.read_reg(r));
            r += 1;
        }
        uart::puts(p, "\n  model=");
        uart::hex8(p, (self.model_id >> 8) as u8);
        uart::hex8(p, self.model_id as u8);
        uart::puts(p, " build=");
        uart::hex8(p, (self.build_id >> 24) as u8);
        uart::hex8(p, (self.build_id >> 16) as u8);
        uart::hex8(p, (self.build_id >> 8) as u8);
        uart::hex8(p, self.build_id as u8);
        uart::puts(p, " devs=");
        uart::dec(p, self.dev_count as u16);
        uart::puts(p, " known=0x");
        uart::hex8(p, self.known_present);
        uart::puts(p, " status=0x");
        uart::hex8(p, self.status);
        uart::puts(p, " fault=0x");
        uart::hex8(p, self.fault);
        uart::puts(p, " ver=");
        uart::dec(p, FW_VER_MAJOR as u16);
        uart::putc(p, b'.');
        uart::dec(p, FW_VER_MINOR as u16);
        uart::putc(p, b'.');
        uart::dec(p, FW_VER_PATCH as u16);
        uart::puts(p, " mode=0x");
        uart::hex8(p, self.mode);
        uart::putc(p, b'\n');
    }
}
