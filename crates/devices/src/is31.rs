//! ISSI/Lumissil **IS31FL3730** 8×8 LED-matrix driver — a shared `crates/devices` display driver
//! (generic over the `embedded-hal` `I2c` seam), like [`crate::ssd1306`] (OLED). Just another device:
//! an I²C interface to the LED panel; any harness (diag today) uses it to paint a status glyph.
//!
//! Address: 7-bit `0b11000_AA`, `AA` set by the AD pin (datasheet Table 1):
//!   AD→GND = 0x60, AD→SCL = 0x61, AD→SDA = 0x62, AD→VCC = 0x63.
//! (0x74 is the IS31FL3731 — a *different* chip.) On the QFLS2-EB, AD is hardwired to GND ⇒ **0x60**;
//! other straps are supported by constructing [`Matrix::new`] with the ACKing address from a scan.
//!
//! Mirrors [`crate::ssd1306::Oled`]: a bound handle carrying just the descriptor; presence/address is the
//! caller's concern (check via the bus scan before `init`).

use crate::Error;
use embedded_hal::i2c::{I2c, SevenBitAddress};

const REG_CONFIG: u8 = 0x00; // config: matrix mode / shutdown
const REG_DATA: u8 = 0x01; // matrix-1 data start (auto-increments across the 8 rows)
const REG_UPDATE: u8 = 0x0C; // write anything to latch the data register into the display
const REG_PWM: u8 = 0x19; // global PWM (brightness)
const CONFIG_8X8: u8 = 0x00; // 8×8 matrix, normal operation, audio input off
const PWM_DEFAULT: u8 = 0x10; // dim but clearly visible (range 0x00..=0x80)

/// The default AD→GND strap address (the QFLS2-EB wiring).
pub const ADDR: SevenBitAddress = 0x60;
/// The strap-address range to scan when the board's AD pin is unknown.
pub const ADDR_RANGE: core::ops::RangeInclusive<u8> = 0x60..=0x63;

/// A bound IS31FL3730 matrix: just the I²C address. Zero-cost — carries no bus handle.
#[derive(Clone, Copy)]
pub struct Matrix {
    addr: SevenBitAddress,
}

impl Matrix {
    pub const fn new(addr: SevenBitAddress) -> Self {
        Self { addr }
    }
    /// The AD→GND default (0x60).
    pub const fn default_strap() -> Self {
        Self { addr: ADDR }
    }
    pub const fn addr(&self) -> SevenBitAddress {
        self.addr
    }

    /// Enable 8×8 matrix mode and set a visible default brightness. `Err(Bus)` if it NACKs.
    pub fn init<I: I2c>(&self, bus: &mut I) -> Result<(), Error<I::Error>> {
        bus.write(self.addr, &[REG_CONFIG, CONFIG_8X8])?;
        bus.write(self.addr, &[REG_PWM, PWM_DEFAULT])?;
        Ok(())
    }

    /// Set global brightness (0x00..=0x80).
    pub fn set_pwm<I: I2c>(&self, bus: &mut I, pwm: u8) -> Result<(), Error<I::Error>> {
        bus.write(self.addr, &[REG_PWM, pwm])?;
        Ok(())
    }

    /// Push an 8-row frame (one byte per row; bit = column) and latch it. Orientation (which bit is which
    /// physical column, row order) is board-dependent — a caller's glyph table owns that mapping.
    pub fn show<I: I2c>(&self, bus: &mut I, frame: &[u8; 8]) -> Result<(), Error<I::Error>> {
        let mut buf = [0u8; 9];
        buf[0] = REG_DATA;
        buf[1..].copy_from_slice(frame);
        bus.write(self.addr, &buf)?;
        bus.write(self.addr, &[REG_UPDATE, 0x00])?;
        Ok(())
    }

    /// Clear the matrix (all LEDs off).
    pub fn clear<I: I2c>(&self, bus: &mut I) -> Result<(), Error<I::Error>> {
        self.show(bus, &[0u8; 8])
    }
}
