//! `DBG_STATUS` (0x37) and `DBG_FAULT` (0x3A) flag bits — shared so a master decodes a node's status and
//! fault bytes the same way the node sets them. (Mirrors what `prod::status` sets; one definition now.)

/// `DBG_STATUS` (0x37) bits.
pub mod status {
    pub const BOOTED: u8 = 1 << 0; // firmware reached the run loop
    pub const ENUMERATED: u8 = 1 << 1; // a sensor-bus scan has completed
    pub const BUS_OK: u8 = 1 << 2; // the sensor bus scanned cleanly (not wedged)
    pub const WRONG_FAMILY: u8 = 1 << 3; // detected part isn't in this image's family (bad flash)
}

/// `DBG_FAULT` (0x3A) bits.
pub mod fault {
    pub const BUS_WEDGED: u8 = 1 << 0; // SDA stuck low / all-ACK — enumeration unreliable
}
