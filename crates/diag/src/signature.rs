//! I²C **signature-pattern** battery — the fault-isolation sequence from `docs/FR2355-I2C-BRINGUP-PLAN.md`
//! §4, re-homed from `diag/src/signature.rs` as the `DiagMode::Signature` body. Instead of one ambiguous
//! "send a byte", it emits a fixed sequence of deterministic patterns on the Leaf bus, each with a KNOWN
//! decoded signature; capture SDA/SCL on a logic analyzer and the first pattern whose signature deviates
//! pinpoints the broken FSM stage (the plan's table). Bounded → runs once, then the node proceeds.
//!
//! The old cooperative `Phase` state machine + `Cx` are dropped (it runs linearly with the WDT held). The
//! patterns:
//!   P1 bus-idle hold      — SDA/SCL flat high          → pull-ups / mux
//!   P2 address sweep      — i2cdetect over 0x08–0x77   → master START/STOP engine + per-device ACK
//!   P3 single-byte WRITE  — S·addr+W·A·data·A·P        → TXBUF→TXIFG pacing, ACK handling
//!   P4 single-byte READ   — S·addr+R·A·data·NAK·P      → RX path, master-NACK-before-STOP
//!   P5 repeated-START     — S·addr+W·A·reg·A·Sr·addr+R → the "phantom TXIFG" repeated-start bug
//!   P6 multi-byte burst   — one S, walking-ones, one P → TXIFG pacing / underrun
//!   P7 byte-value eye     — 0xAA/0x55 stream           → bit order (MSB-first) / noise-vs-signal
//!   P8 BRW freq ladder    — SCL 10→100 kHz, verified   → clock/divider math; the speed it breaks at
//!
//! NOTE (write side effects): P3/P6/P7 WRITE patterns to the chosen target's register. On a sensor that
//! register is not necessarily harmless — but this is a DELIBERATE bench diagnostic (opt-in via `diagmode
//! signature`) and the next POR resets the device. The target is the first device the P2 sweep finds.

use bsp::fmt as uart;
use bsp::pac::Peripherals;
use leaf_scan::enumerate;

const SCAN_LO: u8 = 0x08; // skip the reserved 0x00–0x07 / 0x78–0x7F ranges
const SCAN_HI: u8 = 0x77;
const TARGET_REG: u8 = 0x00; // a low, commonly-present register for the write/read patterns
const TARGET_VAL: u8 = 0xA5; // non-trivial write pattern (≠ 0x00/0xFF → a stuck bus reads wrong)
const EYE_REPS: u16 = 128; // 0xAA/0x55 writes for the scope eye

struct Step {
    khz: u16,
    brw: u16,
}
static LADDER: &[Step] = &[
    Step { khz: 10, brw: 100 },
    Step { khz: 25, brw: 40 },
    Step { khz: 50, brw: 20 },
    Step { khz: 100, brw: 10 },
];

/// `<tag><sig>  OK/FAIL`.
fn line(p: &Peripherals, tag: &str, sig: &str, ok: bool) {
    uart::puts(p, tag);
    uart::puts(p, sig);
    uart::puts(p, if ok { "  OK\n" } else { "  **NACK/timeout**\n" });
}

/// Like [`line`] but also prints the byte read back (hex), so a corrupt value is visible.
fn line_val(p: &Peripherals, tag: &str, sig: &str, ok: bool, val: u8) {
    uart::puts(p, tag);
    uart::puts(p, sig);
    if ok {
        uart::puts(p, "  OK =0x");
        uart::hex8(p, val);
        uart::putc(p, b'\n');
    } else {
        uart::puts(p, "  **NACK/timeout**\n");
    }
}

/// Run the P1–P8 signature battery on the Leaf bus. `scan` provides a default target (its first present
/// device); if none, P3–P8 still emit against that address so the scope shows the master's START/addr
/// attempt (a NACK is itself a signature).
pub fn signature_battery(p: &Peripherals, scan: &enumerate::Scan) {
    uart::puts(p, "\n-- I2C signature patterns (Leaf) --\n");
    uart::puts(p, "   capture SCL/SDA on a logic analyzer; compare each line's signature\n");

    // Start from a known-good bus: clear any wedge a prior read left so P1–P8 measure the bus, not debris.
    let clean = bsp::i2c::recover(p);
    uart::puts(
        p,
        if clean { "   entry: bus recovered idle-high\n" } else { "   entry: **bus still wedged (slave stretching SCL?)**\n" },
    );
    bsp::i2c::set_brw(p, 10); // 100 kHz for P1–P7

    // P1 — idle bus: should read high/high with pull-ups.
    let (sda, scl) = bsp::i2c::bus_levels(p);
    uart::puts(p, "P1 idle:  SDA=");
    uart::putc(p, if sda { b'1' } else { b'0' });
    uart::puts(p, " SCL=");
    uart::putc(p, if scl { b'1' } else { b'0' });
    uart::puts(p, if sda && scl { "  (bus idle high)\n" } else { "  **NOT HIGH -> pull-ups/mux**\n" });

    // P2 — address sweep (i2cdetect). Prints every ACK; first ACK drives P3–P8.
    uart::puts(p, "P2 scan:  ");
    let mut target = 0u8;
    let mut found = 0u8;
    let mut addr = SCAN_LO;
    while addr <= SCAN_HI {
        if bsp::i2c::probe(p, addr) {
            uart::hex8(p, addr);
            uart::putc(p, b' ');
            if found == 0 {
                target = addr;
            }
            found += 1;
        }
        addr += 1;
    }
    if found == 0 {
        // No ACK — fall back to any scan-known present address for the pattern phases (else 0x08).
        target = (SCAN_LO..=SCAN_HI).find(|a| scan.present.get(*a)).unwrap_or(SCAN_LO);
        uart::puts(p, "(none) ");
    }
    uart::puts(p, "-> target 0x");
    uart::hex8(p, target);
    uart::putc(p, b'\n');

    // P3 — single-byte write. S·addr+W·A·reg·A·val·A·P.
    let ok = bsp::i2c::write(p, target, &[TARGET_REG, TARGET_VAL]);
    line(p, "P3 write: ", "S addr+W reg val P", ok);

    // P4 — single-byte read (no reg pointer). S·addr+R·A·data·NAK·P.
    let mut b = [0u8; 1];
    let ok = bsp::i2c::read(p, target, &mut b);
    line_val(p, "P4 read:  ", "S addr+R data NAK P", ok, b[0]);

    // P5 — repeated-START register read. S·addr+W·A·reg·A·Sr·addr+R·A·data·NAK·P.
    let mut b = [0u8; 1];
    let ok = bsp::i2c::read_reg(p, target, TARGET_REG, &mut b);
    line_val(p, "P5 regrd: ", "S addr+W reg Sr addr+R data NAK P", ok, b[0]);

    // P6 — multi-byte burst, walking ones. One S, N data bytes each ACKed, one P.
    let ok = bsp::i2c::write(p, target, &[TARGET_REG, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80]);
    line(p, "P6 burst: ", "S addr+W reg +8 bytes P", ok);

    // P7 — byte-value eye: alternating 0xAA / 0x55. On the scope SDA shows an unmistakable alternating
    // pattern (confirms MSB-first bit order, signal vs noise). Needs an ACKing device for the data phase.
    uart::puts(p, "P7 eye:   0xAA/0x55 x");
    uart::dec(p, EYE_REPS);
    uart::puts(p, " -> scope SDA\n");
    for i in 0..EYE_REPS {
        let pat = if i & 1 == 0 { 0xAA } else { 0x55 };
        let _ = bsp::i2c::write(p, target, &[pat]);
    }

    // P8 — BRW frequency ladder. One verified reg-read per step; the SCL period steps 100µs→10µs on the
    // trace. Restores 100 kHz when done.
    for s in LADDER {
        bsp::i2c::set_brw(p, s.brw);
        let mut b = [0u8; 1];
        let ok = bsp::i2c::read_reg(p, target, TARGET_REG, &mut b);
        uart::puts(p, "P8 ");
        uart::dec(p, s.khz);
        uart::puts(p, " kHz: ");
        uart::puts(p, if ok { "OK\n" } else { "FAIL\n" });
    }
    bsp::i2c::set_brw(p, 10); // restore operating clock
    uart::puts(p, "-- signature battery complete --\n");
}
