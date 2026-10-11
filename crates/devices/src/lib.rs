#![no_std]
//! Shared per-device drivers for the bob-929 Stem — the "nervous system" sensor library.
//!
//! **One module per device**, each behind the standard [`Device`] convention and **generic over
//! `embedded_hal::i2c::I2c`**, so diag (identify / self-test) and prod (measure / report) — on ANY
//! MCU family — consume the SAME driver instead of forking per-sensor code. The per-family bus impl
//! (the `embedded-hal` I2c over eUSCI_B, e.g. `diag/src/hal.rs`) is the only chip-specific piece;
//! everything here is portable. This is the foundation of the stem firmware library.
//!
//! Two consumers, one driver:
//!   • **diag** — enumerate the bus ([`present`] + [`KNOWN`]) and verify identity ([`Device::identify`],
//!     [`Device::self_test`]).
//!   • **prod** — read/report state on demand ([`Device::measure`]).

use embedded_hal::i2c::{I2c, SevenBitAddress};

pub mod apds9960;
pub mod is31;
pub mod mc6470;
pub mod si7021;
pub mod ssd1306;
pub mod vl53l0x;

/// Driver error, parameterised by the HAL's bus error. `Bus` = NACK / stuck bus / HAL failure;
/// `Identity` = the device answered but its WHO_AM_I / data was wrong (present-but-faulty);
/// `NotReady` = present and configured, but no sample was ready within the bounded poll (poll again);
/// `Unsupported` = a transaction shape the bus impl doesn't provide yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    Bus(E),
    Identity,
    NotReady,
    Unsupported,
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self {
        Error::Bus(e)
    }
}

/// The standard convention every device module implements — the "standard protocol/interface".
///
/// A device type is a zero-sized marker (e.g. `Si7021`); the methods are generic over the bus, so a
/// call like `Si7021::measure(&mut bus)` is monomorphised and zero-cost, and an unused driver is
/// stripped by LTO. Split by consumer:
///   • DIAG uses `ADDR` + [`present`] + [`identify`](Device::identify) (+ optional
///     [`self_test`](Device::self_test)).
///   • PROD uses [`measure`](Device::measure) to read / report state on demand.
pub trait Device {
    /// Human-readable name for reports/logs.
    const NAME: &'static str;
    /// Primary 7-bit I²C address. Strap-selectable alternates are exposed by the module.
    const ADDR: SevenBitAddress;
    /// The measurement / state this device reports (prod). Fixed-point — no float on MSP430.
    type Reading;

    /// DIAG: confirm the RIGHT chip is present (WHO_AM_I / signature), not just an address ACK.
    fn identify<I: I2c>(bus: &mut I) -> Result<(), Error<I::Error>>;

    /// PROD: take a reading / snapshot current state, on demand.
    fn measure<I: I2c>(bus: &mut I) -> Result<Self::Reading, Error<I::Error>>;

    /// DIAG: optional deeper exercise (read-back, config round-trip). Default = identify only.
    fn self_test<I: I2c>(bus: &mut I) -> Result<(), Error<I::Error>> {
        Self::identify(bus)
    }
}

/// Address-only presence probe — the bus-scan primitive (diag enumeration, prod detection).
/// `true` if `addr` ACKs. A zero-length write is the `embedded-hal` presence-check convention; the
/// per-family bus impl maps it to an address-only START..STOP.
pub fn present<I: I2c>(bus: &mut I, addr: SevenBitAddress) -> bool {
    bus.write(addr, &[]).is_ok()
}

/// A known device on the Stem, for LABELING a bus scan and (optionally) a single-register WHO_AM_I
/// check. Rich drivers (custom identify/measure) additionally implement [`Device`] in their module;
/// this data-driven table is the fast path for enumeration + config-drift. Ported from diag's
/// `DEVICES`; becomes a per-product (Bob/Ziloo) manifest later.
pub struct Known {
    pub name: &'static str,
    pub addr: SevenBitAddress,
    /// WHO_AM_I register, or [`NO_ID`] for presence-only.
    pub id_reg: u8,
    pub id_val: u8,
}

/// `id_reg == NO_ID` → the device has no single-register WHO_AM_I (presence-only in the table).
pub const NO_ID: u8 = 0xFF;

/// The expected device set. Extend to teach the Stem about a new part. (Seeded from diag's registry;
/// per-product manifests will supersede this.)
pub static KNOWN: &[Known] = &[
    Known { name: "SSD1306 OLED",   addr: 0x3C, id_reg: NO_ID, id_val: 0x00 },
    Known { name: "VL53L0X ToF",    addr: 0x29, id_reg: 0xC0, id_val: 0xEE },
    Known { name: "APDS-9960",      addr: 0x39, id_reg: 0x92, id_val: 0xAB },
    Known { name: "MC6470 accel",   addr: 0x4C, id_reg: NO_ID, id_val: 0x00 },
    Known { name: "MC6470 mag",     addr: 0x0C, id_reg: NO_ID, id_val: 0x00 },
    Known { name: "IS31FL3730 LED", addr: 0x60, id_reg: NO_ID, id_val: 0x00 },
    Known { name: "Si7021 T/RH",    addr: 0x40, id_reg: NO_ID, id_val: 0x00 },
];

/// Look up a scanned address in [`KNOWN`] (to label an enumeration hit).
pub fn known(addr: SevenBitAddress) -> Option<&'static Known> {
    KNOWN.iter().find(|k| k.addr == addr)
}
