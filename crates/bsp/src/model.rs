//! Runtime MSP430 model detection + hardware-capability map — the seam that lets **one image cover a
//! whole product module across a register-/memory-map-compatible family** (STEM-DIRECTION.md "One
//! image per product module"). The compiled family is chosen by the bsp chip feature
//! (`fr2476`/`fr2355`/`fr2155`/`fr2433`, forwarded by the consumer's family feature); `detect()` reads
//! the Device ID at runtime so the image adapts to the concrete member, and `matches_build_family()`
//! catches a wrong-family flash.
//!
//! **Layering:** this module is pure **silicon identity + hardware capability** and is deliberately
//! free of any bus-protocol knowledge — it does NOT depend on `stembus`. The **wire-facing** name and
//! role→address codecs live in `stembus::role` (`model_name`, `role_for_model().addr()`), shared with
//! the masters; a caller that needs the Stem address does
//! `stembus::role::role_for_model(model.device_id()).addr()`. Keeping bsp protocol-free is why the
//! board crate stays linkable without the wire contract.
//!
//! Lives in `bsp` (not `prod`) so **both** harnesses share one detector: prod's boot + diag's POST /
//! `chip` command read the SAME silicon identity instead of each re-reading the TLV.

#![allow(dead_code)] // Seam consumed incrementally as bring-up lands (pin_map in particular).

/// MSP430 device-descriptor (TLV) — the 16-bit **Device ID** is the little-endian word at 0x1A04.
/// Source: MSP430FR2476 datasheet Table 9-29/9-30 (base 0x1A00; 1A04h low / 1A05h high). Same TLV
/// layout across the FR2xx families; confirm the offset against the target datasheet per family.
const DEVICE_ID_ADDR: u16 = 0x1A04;

/// Known MSP430 Device IDs (datasheet TLV tables: 1A05h high / 1A04h low → LE word). Listed across
/// families because the read itself is PAC-agnostic — this also lets boot detect a WRONG-FAMILY flash
/// (an image built for family X running on part Y): the detected model won't match the compiled PAC.
///
/// **These mirror `stembus::regmap::MODEL_*`** (the master-side map needs the same ids); the two must
/// agree, and the datasheet TLV is the single source of truth. They are kept separate so the board
/// crate does not depend on the bus-protocol crate for five constants.
const DEVICE_ID_FR2476: u16 = 0x832A; // FR247x (dual-I²C)
const DEVICE_ID_FR2475: u16 = 0x832B; // FR247x (dual-I²C)
const DEVICE_ID_FR2433: u16 = 0x8240; // FR24xx (single-I²C)
// FR2355 CONFIRMED on hardware (2026-09-30): the JTAG-confirmed FR2355 (id 0x01ff) reports TLV 0x830C
// (read over SBW at 0x1A04, same as diag's regs::chip_name). FR2155 is still a placeholder — there is
// no FR2155 board (it's validated on FR2355 hardware, which reports FR2355's TLV, not FR2155's), so its
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

    /// True if this model belongs to the family this image was compiled for (the active **bsp** chip
    /// feature). A mismatch = wrong-family flash → boot should refuse/flag rather than mis-drive pins.
    ///
    /// Note the bsp features are PAC-level (`fr2476`/`fr2355`/`fr2155`/`fr2433`) — the consumer's
    /// family feature forwards to exactly one. FR2476/FR2475 share the `fr2476` PAC/family. (FR2155 is
    /// validated on FR2355 silicon, which reports FR2355's TLV → a `fr2155` build on that bench reads
    /// `Fr2355` and flags a "mismatch"; expected until a real FR2155 board exists.)
    ///
    /// The COMBINED image (`fr2xxx`) is in-family for EVERY dual member it covers (FR2476/FR2475 **and**
    /// FR2355/FR2155) — that is the whole point of the single image — so each of those arms also accepts
    /// `fr2xxx`. A combined image on an FR2433 or unknown part still correctly flags a mismatch.
    pub fn matches_build_family(self) -> bool {
        match self {
            Model::Fr2476 | Model::Fr2475 => cfg!(any(feature = "fr2476", feature = "fr2xxx")),
            Model::Fr2155 => cfg!(any(feature = "fr2155", feature = "fr2xxx")),
            Model::Fr2355 => cfg!(any(feature = "fr2355", feature = "fr2xxx")),
            Model::Fr2433 => cfg!(feature = "fr2433"),
            Model::Unknown(_) => false,
        }
    }

    /// The eUSCI_B1 (Stem-slave) peripheral **base address** for this model — the ONE register-level
    /// divergence that makes the combined `fr2xxx` image need runtime dispatch. The register-block
    /// *layout* is identical across the family (verified per-datasheet: eUSCI_B is one IP block); only
    /// the base moved. FR247x: 0x0580 (confirmed, pac/msp430fr2476). FR2355/FR2155: 0x05C0 (confirmed,
    /// pac/msp430fr2355 + pac/msp430fr2155). `Fr2433`/`Unknown` fall back to 0x0580 — they are never the
    /// combined target (FR2433 is the single-I²C `modules/expander`, built per-family), so the value is
    /// inert there; it exists only so the match is total.
    pub const fn stem_i2c_base(self) -> usize {
        match self {
            Model::Fr2476 | Model::Fr2475 => 0x0580,
            Model::Fr2355 | Model::Fr2155 => 0x05C0,
            Model::Fr2433 | Model::Unknown(_) => 0x0580,
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
/// assignments land with the BOM + [../../connections.toml]. `bank_count` = PCA9698 GPIO banks
/// physically backed; `sensor_i2c` = whether a 2nd I²C (sensor master) exists (true on FR247x).
///
/// **One-image-per-product-module (STEM-DIRECTION.md) raises the bar here:** because the image must
/// drive the *correct* hardware when a chip is second-sourced, these must be filled out and tested per
/// family member before multi-model portability is trusted in the field — today they are placeholders.
pub struct PinMap {
    pub bank_count: u8,   // physically-backed GPIO banks (P1.. → regmap banks 0..)
    pub mcu_i2c: bool,    // MCU-bus slave surface — the reason to exist; always present
    pub sensor_i2c: bool, // sensor-master bus — true on 2×-I²C parts (FR247x), false on 1×-I²C (FR2433)
}
