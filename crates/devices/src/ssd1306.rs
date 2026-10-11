//! SSD1306 OLED text driver — a shared `crates/devices` display driver (generic over
//! `embedded_hal::i2c::I2c`), ported from the proven diag-local `diag/src/ssd1306_raw.rs`. Direct
//! register writes + a 5×7 font (no `embedded-graphics` stack). Horizontal addressing; each glyph is
//! 5 column bytes (bit0 = top row) + 1 px gap → 6 px advance.
//!
//! **Panel-parameterised** — the size-specific bytes (mux ratio, COM-pin config, page count, width,
//! address) live in a [`Panel`] descriptor, so an additional OLED dimension is a one-line `const`, not
//! a code change. An [`Oled`] handle binds a panel and exposes init/clear/text/off. Not a
//! [`Device`](crate::Device) (that trait is for sensors); a display has its own API.

use crate::Error;
use embedded_hal::i2c::{I2c, SevenBitAddress};

const CMD: u8 = 0x00; // control byte: following bytes are commands
const DATA: u8 = 0x40; // control byte: following bytes are GDDRAM data

/// A panel descriptor — everything that varies between SSD1306 sizes. Add a new size by adding a
/// `const`; the driver code is dimension-agnostic (uses `width`/`pages`/`mux`/`com`).
#[derive(Clone, Copy)]
pub struct Panel {
    pub addr: SevenBitAddress,
    pub width: u8, // columns (typically 128)
    pub pages: u8, // rows / 8 (4 for a 32-row panel, 8 for 64-row)
    mux: u8,       // 0xA8 arg = (rows - 1)
    com: u8,       // 0xDA arg = COM-pins hardware config
}

/// 128×32 (the bench panel). 4 pages, mux 31, COM config 0x02.
pub const SSD1306_128X32: Panel = Panel { addr: 0x3C, width: 128, pages: 4, mux: 0x1F, com: 0x02 };
/// 128×64. 8 pages, mux 63, COM config 0x12. Ready for a taller panel — construct `Oled::new` with it.
pub const SSD1306_128X64: Panel = Panel { addr: 0x3C, width: 128, pages: 8, mux: 0x3F, com: 0x12 };

/// A bound display: a [`Panel`] + the driver methods. Zero-cost — just carries the descriptor.
#[derive(Clone, Copy)]
pub struct Oled {
    p: Panel,
}

impl Oled {
    pub const fn new(panel: Panel) -> Self {
        Self { p: panel }
    }

    /// The panel's I²C address (so a caller can presence-check before init).
    pub const fn addr(&self) -> SevenBitAddress {
        self.p.addr
    }

    /// Init the panel (charge-pump on, horizontal addressing). `Err(Bus)` if it NACKs. The mux ratio
    /// and COM config come from the [`Panel`], so this handles any supported size.
    pub fn init<I: I2c>(&self, bus: &mut I) -> Result<(), Error<I::Error>> {
        bus.write(
            self.p.addr,
            &[
                CMD, 0xAE, // display off
                0xD5, 0x80, // clock divide/oscillator
                0xA8, self.p.mux, // multiplex ratio = rows-1
                0xD3, 0x00, // display offset 0
                0x40, // start line 0
                0x8D, 0x14, // charge pump on
                0x20, 0x00, // memory addressing = horizontal
                0xA1, // segment remap (col 127 -> SEG0)
                0xC8, // COM scan direction remapped
                0xDA, self.p.com, // COM pins hardware config (size-specific)
                0x81, 0x8F, // contrast
                0xD9, 0xF1, // pre-charge
                0xDB, 0x40, // VCOMH deselect
                0xA4, // resume to RAM content
                0xA6, // normal (not inverted)
                0x2E, // deactivate scroll
                0xAF, // display on
            ],
        )?;
        Ok(())
    }

    /// Turn the panel OFF (display off — dark/blank, low power). Use after a boot splash so an unused
    /// OLED shows NOTHING rather than stale content or an uninitialised white raster.
    pub fn off<I: I2c>(&self, bus: &mut I) -> Result<(), Error<I::Error>> {
        bus.write(self.p.addr, &[CMD, 0xAE])?;
        Ok(())
    }

    fn window<I: I2c>(&self, bus: &mut I, page: u8, col0: u8, col1: u8) -> Result<(), Error<I::Error>> {
        bus.write(self.p.addr, &[CMD, 0x22, page, page, 0x21, col0, col1])?;
        Ok(())
    }

    /// Blank the whole panel (all pages).
    pub fn clear<I: I2c>(&self, bus: &mut I) -> Result<(), Error<I::Error>> {
        let mut buf = [0u8; 129]; // DATA byte + up to 128 columns
        buf[0] = DATA;
        let n = 1 + self.p.width as usize;
        for page in 0..self.p.pages {
            self.window(bus, page, 0, self.p.width - 1)?;
            bus.write(self.p.addr, &buf[..n])?;
        }
        Ok(())
    }

    fn put<I: I2c>(&self, bus: &mut I, page: u8, col: u8, g: &[u8; 5]) -> Result<(), Error<I::Error>> {
        self.window(bus, page, col, col + 5)?;
        bus.write(self.p.addr, &[DATA, g[0], g[1], g[2], g[3], g[4], 0x00])?;
        Ok(())
    }

    /// Render `s` at (page, col); returns the next free column. Clips at the panel's right edge.
    pub fn text<I: I2c>(&self, bus: &mut I, page: u8, col: u8, s: &str) -> Result<u8, Error<I::Error>> {
        let last = self.p.width.saturating_sub(6); // last col that still fits a 6px cell
        let mut c = col;
        for &ch in s.as_bytes() {
            if c > last {
                break;
            }
            self.put(bus, page, c, &glyph(ch))?;
            c += 6;
        }
        Ok(c)
    }

    /// Fill every pixel with `byte` (0x00 = off, 0xFF = on) — the boot self-test's "driver works" flash.
    pub fn fill<I: I2c>(&self, bus: &mut I, byte: u8) -> Result<(), Error<I::Error>> {
        let mut buf = [byte; 129];
        buf[0] = DATA; // control byte, not pixel data
        let n = 1 + self.p.width as usize;
        for page in 0..self.p.pages {
            self.window(bus, page, 0, self.p.width - 1)?;
            bus.write(self.p.addr, &buf[..n])?;
        }
        Ok(())
    }

    /// Blank from `col` to the end of `page` — wipe a variable-width line's tail so a shrinking value
    /// can't leave stale glyphs behind.
    pub fn clear_eol<I: I2c>(&self, bus: &mut I, page: u8, col: u8) -> Result<(), Error<I::Error>> {
        if col >= self.p.width {
            return Ok(());
        }
        self.window(bus, page, col, self.p.width - 1)?;
        let mut b = [0u8; 129];
        b[0] = DATA;
        let n = 1 + (self.p.width - col) as usize;
        bus.write(self.p.addr, &b[..n])?;
        Ok(())
    }

    /// Render `v` in decimal (no leading zeros) at (page, col); returns the next free column.
    pub fn num<I: I2c>(&self, bus: &mut I, page: u8, col: u8, v: u32) -> Result<u8, Error<I::Error>> {
        let mut tmp = [0u8; 10];
        let mut n = 0usize;
        let mut val = v;
        if val == 0 {
            tmp[0] = b'0';
            n = 1;
        }
        while val > 0 {
            tmp[n] = b'0' + (val % 10) as u8;
            val /= 10;
            n += 1;
        }
        let mut s = [0u8; 10]; // reverse (digits were emitted least-significant first)
        for i in 0..n {
            s[i] = tmp[n - 1 - i];
        }
        let st = core::str::from_utf8(&s[..n]).unwrap_or("");
        self.text(bus, page, col, st)
    }
}

// 5×7 font (column-major, bit0 = top row). Standard glcdfont values; unmapped renders blank.
#[rustfmt::skip]
static DIGITS: [[u8; 5]; 10] = [
    [0x3E, 0x51, 0x49, 0x45, 0x3E], [0x00, 0x42, 0x7F, 0x40, 0x00], [0x42, 0x61, 0x51, 0x49, 0x46],
    [0x21, 0x41, 0x45, 0x4B, 0x31], [0x18, 0x14, 0x12, 0x7F, 0x10], [0x27, 0x45, 0x45, 0x45, 0x39],
    [0x3C, 0x4A, 0x49, 0x49, 0x30], [0x01, 0x71, 0x09, 0x05, 0x03], [0x36, 0x49, 0x49, 0x49, 0x36],
    [0x06, 0x49, 0x49, 0x29, 0x1E],
];

#[rustfmt::skip]
static ALPHA: [[u8; 5]; 26] = [
    [0x7E, 0x11, 0x11, 0x11, 0x7E], [0x7F, 0x49, 0x49, 0x49, 0x36], [0x3E, 0x41, 0x41, 0x41, 0x22],
    [0x7F, 0x41, 0x41, 0x22, 0x1C], [0x7F, 0x49, 0x49, 0x49, 0x41], [0x7F, 0x09, 0x09, 0x09, 0x01],
    [0x3E, 0x41, 0x49, 0x49, 0x7A], [0x7F, 0x08, 0x08, 0x08, 0x7F], [0x00, 0x41, 0x7F, 0x41, 0x00],
    [0x20, 0x40, 0x41, 0x3F, 0x01], [0x7F, 0x08, 0x14, 0x22, 0x41], [0x7F, 0x40, 0x40, 0x40, 0x40],
    [0x7F, 0x02, 0x0C, 0x02, 0x7F], [0x7F, 0x04, 0x08, 0x10, 0x7F], [0x3E, 0x41, 0x41, 0x41, 0x3E],
    [0x7F, 0x09, 0x09, 0x09, 0x06], [0x3E, 0x41, 0x51, 0x21, 0x5E], [0x7F, 0x09, 0x19, 0x29, 0x46],
    [0x46, 0x49, 0x49, 0x49, 0x31], [0x01, 0x01, 0x7F, 0x01, 0x01], [0x3F, 0x40, 0x40, 0x40, 0x3F],
    [0x1F, 0x20, 0x40, 0x20, 0x1F], [0x7F, 0x20, 0x18, 0x20, 0x7F], [0x63, 0x14, 0x08, 0x14, 0x63],
    [0x07, 0x08, 0x70, 0x08, 0x07], [0x61, 0x51, 0x49, 0x45, 0x43],
];

fn glyph(c: u8) -> [u8; 5] {
    match c {
        b'0'..=b'9' => DIGITS[(c - b'0') as usize],
        b'A'..=b'Z' => ALPHA[(c - b'A') as usize],
        b'a'..=b'z' => ALPHA[(c - b'a') as usize], // fold lowercase onto uppercase
        b'.' => [0x00, 0x60, 0x60, 0x00, 0x00],
        b'-' => [0x08, 0x08, 0x08, 0x08, 0x08],
        b':' => [0x00, 0x36, 0x36, 0x00, 0x00],
        b'/' => [0x20, 0x10, 0x08, 0x04, 0x02],
        b'%' => [0x23, 0x13, 0x08, 0x64, 0x62],
        _ => [0x00, 0x00, 0x00, 0x00, 0x00], // space + anything unmapped
    }
}
