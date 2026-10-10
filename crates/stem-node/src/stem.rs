//! Stem-bus I²C **slave transport bring-up**. The eUSCI programming, pins, IFG decode, and the wake ISR
//! live in `bsp::stem_i2c` (the transport); this is just the thin, chip-agnostic entry the bin calls to
//! bring the slave up at its role-based address. The command SURFACE above the transport is cros_ec — the
//! `EcSlave` framer (`stembus::cros_ec`) + the `ec`/`node` glue — NOT a register file.
//!
//! (History: this module used to also carry the PCA9698 **regmap** register-file dispatch — `RegFile`/
//! `Slave`/`poll` + the GPIO-bank shadow. That was retired when cros_ec became the sole command interface;
//! see `docs/COMMANDS.md` §0 and the regmap-drop. The shared wire CONSTANTS — role addresses, mode codes,
//! CRC width, iface magic, model ids — live in `stembus::regmap` and are unaffected.)

use node_hal::StemSlave;

/// Bring the node's slave transport up at the role-based `addr` (+ route its pins), generic over the
/// hardware seam. Thin wrapper over [`StemSlave::init`] so call sites keep `stem::init(&hw, addr)`.
///
/// The I²C-slave address is **ROLE-BASED** (`stembus::role::role_for_model(..).addr()`; I2C-API.md):
/// FR2355/FR2155=0x20 (production supervisor), FR2476/FR2475=0x21 (dev stand-in), FR2433=0x22
/// (I/O-expander). The physical GPIO-bank backing moved into `bsp::node_hw`'s `node_hal::GpioBanks` impl.
pub fn init<H: StemSlave>(hw: &H, addr: u16) {
    hw.init(addr);
}
