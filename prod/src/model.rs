//! Runtime MSP430 model detection + pin mapping — the seam that lets **one image cover a whole
//! register-/memory-map-compatible family** (see DESIGN.md "Permutation strategy"). The compiled
//! family is chosen by the Cargo family feature (`fr247x` default = FR2476/FR2475 dual-I²C; `fr24xx`
//! = FR2433 single-I²C); FR215x (FR2155) is a future family. `detect()` reads the Device ID at
//! runtime and `matches_build_family()` catches a wrong-family flash.
//!
//! Detection is datasheet-grounded; the model table + pin maps stay thin until the BOM/bring-up fills them.

#![allow(dead_code)] // Seam consumed incrementally as bring-up lands.

/// MSP430 device-descriptor (TLV) — the 16-bit **Device ID** is the little-endian word at 0x1A04.
/// Source: MSP430FR2476 datasheet Table 9-29/9-30 (base 0x1A00; 1A04h low / 1A05h high). Same TLV
/// layout across the FR2xx families; confirm the offset against the target datasheet per family.
const DEVICE_ID_ADDR: u16 = 0x1A04;

/// Known MSP430 Device IDs (datasheet TLV tables: 1A05h high / 1A04h low → LE word). Listed across
/// families because the read itself is PAC-agnostic — this also lets boot detect a WRONG-FAMILY flash
/// (an image built for family X running on part Y): the detected model won't match the compiled PAC.
const DEVICE_ID_FR2476: u16 = 0x832A; // FR247x (dual-I²C)
const DEVICE_ID_FR2475: u16 = 0x832B; // FR247x (dual-I²C)
const DEVICE_ID_FR2433: u16 = 0x8240; // FR24xx (single-I²C)
// FR2355 CONFIRMED on hardware (2026-09-30): the JTAG-confirmed FR2355 (id 0x01ff) reports TLV 0x830C
// (read over SBW at 0x1A04, same as diag's regs::chip_name). FR2155 is still a placeholder — there is
// no FR2155 board (it's validated on FR2355 hardware, which reads FR2355's TLV, not FR2155's), so its
// own id can't be read until a real 2155 board exists. Distinct sentinel so the match arm compiles.
const DEVICE_ID_FR2155: u16 = 0xF215; // PLACEHOLDER — no FR2155 board to confirm
const DEVICE_ID_FR2355: u16 = 0x830C; // confirmed FR2355 TLV device-id (bench 2026-09-30)

/// A recognised MSP430 model. `Unknown(id)` surfaces unrecognised silicon so boot can flag it rather
/// than mis-map pins.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Model {
    Fr2476,
    Fr2475,
    Fr2155,
    Fr2355,
    Fr2433,
    Unknown(u16),
}

impl Model {
    pub const fn from_device_id(id: u16) -> Self {
        match id {
            DEVICE_ID_FR2476 => Model::Fr2476,
            DEVICE_ID_FR2475 => Model::Fr2475,
            DEVICE_ID_FR2155 => Model::Fr2155,
            DEVICE_ID_FR2355 => Model::Fr2355,
            DEVICE_ID_FR2433 => Model::Fr2433,
            other => Model::Unknown(other),
        }
    }

    /// The 16-bit Device ID for this model (inverse of [`from_device_id`]; `Unknown` carries its raw id).
    pub const fn device_id(self) -> u16 {
        match self {
            Model::Fr2476 => DEVICE_ID_FR2476,
            Model::Fr2475 => DEVICE_ID_FR2475,
            Model::Fr2155 => DEVICE_ID_FR2155,
            Model::Fr2355 => DEVICE_ID_FR2355,
            Model::Fr2433 => DEVICE_ID_FR2433,
            Model::Unknown(id) => id,
        }
    }

    /// The node's Stem-bus I²C slave address, chosen by this model's **role** (see
    /// [`../I2C-API.md`](../../I2C-API.md) "Stem address is ROLE-BASED"). The master reads *what it is
    /// talking to* from the address alone; a dev stand-in cannot pose as the production supervisor.
    /// Values live in the PCA9698 strap group 0x20–0x27. (0x21/0x22 are a derivation — adjust with the
    /// I2C-API.md role table if the map changes.)
    pub const fn stem_addr(self) -> u16 {
        match self {
            // Production sleep-supervisor
            Model::Fr2355 | Model::Fr2155 => 0x20,
            // DEV stand-in — SAME firmware as FR2355, distinct addr so it can't pose as the real one
            Model::Fr2476 | Model::Fr2475 => 0x21,
            // I/O-expander role — PLACEHOLDER until defined
            Model::Fr2433 => 0x22,
            // Unrecognised silicon: answer the supervisor base (boot already raises WRONG-FAMILY)
            Model::Unknown(_) => 0x20,
        }
    }

    /// True if this model belongs to the family this image was compiled for (the active Cargo family
    /// feature). A mismatch = wrong-family flash → boot should refuse/flag rather than mis-drive pins.
    pub fn matches_build_family(self) -> bool {
        match self {
            Model::Fr2476 | Model::Fr2475 => cfg!(feature = "fr247x"),
            Model::Fr2155 => cfg!(feature = "fr215x"),
            Model::Fr2355 => cfg!(feature = "fr235x"),
            Model::Fr2433 => cfg!(feature = "fr24xx"),
            Model::Unknown(_) => false,
        }
    }

    /// The pin/bus assignment for this model. See [`PinMap`].
    pub fn pin_map(self) -> PinMap {
        match self {
            // FR247x: 2× eUSCI_B → MCU-bus SLAVE + sensor-bus MASTER (dual-I²C node). 43 I/O over ports
            // P1–P6; reserve I²C/UART/STEM pins → ~5 GPIO banks exposed. Refine with connections.toml.
            Model::Fr2476 | Model::Fr2475 => PinMap { bank_count: 5, mcu_i2c: true, sensor_i2c: true },
            // FR2155/FR2355: dual-I²C like FR247x (2× eUSCI_B). Bank count refined with the BOM.
            Model::Fr2155 | Model::Fr2355 => PinMap { bank_count: 5, mcu_i2c: true, sensor_i2c: true },
            // FR2433: 1× eUSCI_B → MCU-bus SLAVE only, no sensor master. VQFN-24 ports P1–P3 → 3 banks.
            Model::Fr2433 => PinMap { bank_count: 3, mcu_i2c: true, sensor_i2c: false },
            // Unrecognised silicon: expose only the mandatory MCU slave until a real map is known.
            Model::Unknown(_) => PinMap { bank_count: 0, mcu_i2c: true, sensor_i2c: false },
        }
    }
}

/// Read the Device ID from the TLV descriptor and classify. Pure volatile read, no side effects.
pub fn detect() -> Model {
    let id = unsafe { core::ptr::read_volatile(DEVICE_ID_ADDR as *const u16) };
    Model::from_device_id(id)
}

/// Which buses/pins each function uses on the detected model. Thin for now — concrete port/pin/ADC
/// assignments land with the BOM + [../crates/bsp/connections.toml]. `bank_count` = PCA9698 GPIO
/// banks physically backed; `sensor_i2c` = whether a 2nd I²C (sensor master) exists (true on FR247x).
pub struct PinMap {
    pub bank_count: u8,   // physically-backed GPIO banks (P1.. → regmap banks 0..)
    pub mcu_i2c: bool,    // MCU-bus slave surface — the reason to exist; always present
    pub sensor_i2c: bool, // sensor-master bus — true on 2×-I²C parts (FR247x), false on 1×-I²C (FR2433)
}
