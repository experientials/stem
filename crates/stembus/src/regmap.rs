//! The Stem-bus register contract — the SINGLE definition of the wire interface, shared by the server
//! (`prod::regmap` re-exports these) and every master (`stem-probe`, M7). Server-internal decode logic
//! (`RegSel`, `decode`, `ShadowState`) stays in `prod::regmap`; this is the address space + codes only.
//!
//! Derived from I2C-API.md (the canonical API) + the PCA9698 datasheet register summary.

// ── Role-based Stem I²C addresses (I2C-API.md). The address identifies the node's ROLE. ──
pub const ADDR_SUPERVISOR: u8 = 0x20; // FR2355/FR2155 — production sleep-supervisor
pub const ADDR_DEV_STANDIN: u8 = 0x21; // FR2476/FR2475 — dev stand-in (same fw, "not the real thing")
pub const ADDR_EXPANDER: u8 = 0x22; // FR2433 — I/O-expander role (placeholder)
pub const ADDR_POWER: u8 = 0x23; // CYPM1111 PD controller — full node; masters the bus while SoM off

// ── Detected silicon Device IDs (TLV 0x1A04) — map model→role and label a responder. ──
pub const MODEL_FR2476: u16 = 0x832A;
pub const MODEL_FR2475: u16 = 0x832B;
pub const MODEL_FR2355: u16 = 0x830C;
pub const MODEL_FR2433: u16 = 0x8240;
pub const MODEL_FR2155: u16 = 0xF215; // placeholder — confirm against silicon

// ── PCA9698 command byte (I2C-API.md §7.3): low 6 bits = register pointer, bit7 = auto-increment. ──
pub const CMD_REG_MASK: u8 = 0x3F;
pub const CMD_AI: u8 = 0x80;

// ── PCA9698 GPIO banks (8 ports / 64 pins; bank = low 3 bits, 0..=7) + single registers. ──
pub const BANKS: u8 = 8;
pub const IP_BASE: u8 = 0x00; // Input Port
pub const OP_BASE: u8 = 0x08; // Output Port
pub const PI_BASE: u8 = 0x10; // Polarity Inversion
pub const IOC_BASE: u8 = 0x18; // I/O Config (direction)
pub const MSK_BASE: u8 = 0x20; // Mask Interrupt
pub const OUTCONF: u8 = 0x28; // Output structure config
pub const ALLBNK: u8 = 0x29; // All-bank control
pub const MODE: u8 = 0x2A; // PCA9698 mode selection

// ── Thepia extensions (I2C-API.md "Voltage and more"). ──
pub const INIT_CODE: u8 = 0x2B; // startup/reset init selector (persisted)
pub const VSOM_VOLTAGE: u8 = 0x2C; // READ: SoM rail voltage
pub const CHARGE_VOLTAGE: u8 = 0x2D; // READ: charge rail voltage
pub const VSOM_THRESHOLD: u8 = 0x2E; // WRITE 16-bit: threshold → MSG when crossed
pub const BAUD_RATE: u8 = 0x2F; // optional STEM-MSG baud set

// ── Debug / identity / status window (0x30–0x3F), additive to the PCA9698 core. ──
pub const DBG_IFACE: u8 = 0x30; // magic → DBG_IFACE_MAGIC
pub const DBG_IFACE_MAGIC: u8 = 0xD0; // "debug regs, v0" — bump on an incompatible layout change
pub const DBG_MODEL_L: u8 = 0x31; // detected Device ID, low byte
pub const DBG_MODEL_H: u8 = 0x32; // detected Device ID, high byte
pub const DBG_BUILD_0: u8 = 0x33; // 32-bit build id (FNV-1a of the build stamp), LSB first
pub const DBG_BUILD_1: u8 = 0x34;
pub const DBG_BUILD_2: u8 = 0x35;
pub const DBG_BUILD_3: u8 = 0x36;
pub const DBG_STATUS: u8 = 0x37; // status flag bits
pub const DBG_DEV_COUNT: u8 = 0x38; // I²C devices found on the sensor bus (last scan)
pub const DBG_KNOWN_PRESENT: u8 = 0x39; // bitmap over the known-device table
pub const DBG_FAULT: u8 = 0x3A; // fault flags (e.g. BUS_WEDGED)
pub const DBG_FW_VER_MAJOR: u8 = 0x3B; // semantic fw version (from Cargo.toml)
pub const DBG_FW_VER_MINOR: u8 = 0x3C;
pub const DBG_FW_VER_PATCH: u8 = 0x3D;

// SoM-facing operating-mode control. READ = current mode; WRITE = request a switch (applied by the
// node's mode machine after the transaction). Codes are a stable wire ABI regardless of compiled modes.
pub const MODE_CTRL: u8 = 0x3E;
pub const MODE_CODE_PASSIVE: u8 = 0x00;
pub const MODE_CODE_SENSING: u8 = 0x01;
pub const MODE_CODE_UNKNOWN: u8 = 0xFF; // reported when the current mode has no ABI code

// Per-sector FRAM CRC readout (ADR-0010). WINDOWED (the 6-bit pointer has no room for 8×u16 as separate
// addresses): a master WRITE sets the byte index 0..DBG_CRC_BYTES; a READ returns that CRC byte. The M7
// reads all bytes, assembles per-sector CRC16s, and compares to its golden values (see [`crate::crc`]).
pub const DBG_CRC: u8 = 0x3F;
pub const DBG_CRC_BYTES: usize = 16; // SECTORS × 2 (u16 each)
