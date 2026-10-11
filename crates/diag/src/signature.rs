//! I²C **signature-pattern** generator — the fault-isolation battery from
//! `docs/FR2355-I2C-BRINGUP-PLAN.md` §4. Instead of one ambiguous "send a byte", it emits a fixed
//! sequence of deterministic patterns on eUSCI_B0 (Leaf, P1.2 SDA / P1.3 SCL), each with a **known
//! decoded signature**. Capture the bus on a logic analyzer and the *first* pattern whose signature
//! deviates pinpoints the broken FSM stage (see the plan's table). Runs once, then returns to POST.
//!
//! The patterns (and what a deviation localizes):
//!   P1 bus-idle hold      — SDA/SCL flat high          → pull-ups / mux
//!   P2 address sweep      — i2cdetect over 0x08–0x77   → master START/STOP engine + per-device ACK
//!   P3 single-byte WRITE  — S·addr+W·A·data·A·P        → TXBUF→TXIFG pacing, ACK handling
//!   P4 single-byte READ   — S·addr+R·A·data·NAK·P      → RX path, master-NACK-before-STOP
//!   P5 repeated-START     — S·addr+W·A·reg·A·Sr·addr+R → the "phantom TXIFG" repeated-start bug
//!   P6 multi-byte burst   — one S, walking-ones, one P → TXIFG pacing / underrun
//!   P7 byte-value eye     — 0xAA/0x55 stream           → bit order (MSB-first) / noise-vs-signal
//!   P8 BRW freq ladder    — SCL 10→100 kHz, verified   → clock/divider math; the speed it breaks at
//!
//! Like the other bus tasks it is **cooperative**: each `poll` does a bounded chunk and yields, so
//! radar/button/UI stay responsive. State a blocking run would keep on the stack lives in the task.

use crate::tasks::{Cx, Mode};
use crate::{i2c, uart};
use crate::pac::Peripherals;
use sched::Task;

// Bench Leaf target when the sweep finds no device (PCA9698 @0x20, same one the stress task verifies).
// reg 0x10 = Polarity-Inversion bank 0 (reset 0x00, unused) — safe to write a non-trivial 0xA5 to.
const FALLBACK_ADDR: u8 = 0x20;
const TARGET_REG: u8 = 0x10;
const TARGET_VAL: u8 = 0xA5;

const SCAN_LO: u8 = 0x08; // skip the reserved 0x00–0x07 / 0x78–0x7F ranges
const SCAN_HI: u8 = 0x77;
const SCAN_BATCH: u8 = 16; // addresses probed per poll (bounded → yields)
const EYE_REPS: u16 = 128; // 0xAA/0x55 writes for the scope eye
const EYE_BATCH: u16 = 16; // eye writes per poll

/// SCL = SMCLK (1 MHz) / brw. The low end of the stress ladder — the range that separates a
/// rise-time/capacitance fault (clears as the clock drops) from a logic fault (persists to 10 kHz).
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

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Idle,   // P1
    Scan,   // P2 (batched over addresses)
    Write,  // P3
    Read,   // P4
    RegRead, // P5
    Burst,  // P6
    Eye,    // P7 (batched over reps)
    Ladder, // P8 (one step per poll)
    Done,
}

pub struct SignatureTask {
    entered: bool,
    phase: Phase,
    addr: u8,   // scan cursor
    found: u8,   // devices that ACKed in the sweep
    target: u8,  // first ACKing address (→ the P3–P8 target), else FALLBACK_ADDR
    eye: u16,    // eye reps done
    li: usize,   // ladder index
}

impl SignatureTask {
    pub const fn new() -> Self {
        Self {
            entered: false,
            phase: Phase::Idle,
            addr: SCAN_LO,
            found: 0,
            target: FALLBACK_ADDR,
            eye: 0,
            li: 0,
        }
    }

    fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.addr = SCAN_LO;
        self.found = 0;
        self.target = FALLBACK_ADDR;
        self.eye = 0;
        self.li = 0;
    }
}

impl<'a> Task<Cx<'a>> for SignatureTask {
    fn poll(&mut self, cx: &mut Cx<'a>, _now: u32) -> Option<u32> {
        if cx.mode != Mode::Signature {
            self.entered = false; // re-entry re-initialises
            return Some(100);
        }
        let p = cx.p;
        if !self.entered {
            self.entered = true;
            self.reset();
            i2c::set_brw(p, 10); // 100 kHz — the operating clock for P1–P7
            uart::puts(p, "\n-- I2C signature patterns (B0 / P1.2-P1.3) --\n");
            uart::puts(p, "   capture SCL/SDA on a logic analyzer; compare each line's signature\n");
            // Start from a known-good bus: clear any wedge a prior POST/read left (SCL-low or SDA-low)
            // so P1-P8 measure the bus, not the last test's debris.
            let clean = i2c::recover(p);
            uart::puts(p, if clean {
                "   entry: bus recovered idle-high\n"
            } else {
                "   entry: **bus still wedged after recover() (slave stretching SCL?)**\n"
            });
        }
        crate::pet_wdt(p);

        match self.phase {
            // P1 — no transaction, just report the idle bus. Should read high/high with pull-ups.
            Phase::Idle => {
                let (sda, scl) = i2c::bus_levels(p);
                uart::puts(p, "P1 idle:  SDA=");
                uart::putc(p, if sda { b'1' } else { b'0' });
                uart::puts(p, " SCL=");
                uart::putc(p, if scl { b'1' } else { b'0' });
                uart::puts(p, if sda && scl { "  (bus idle high)\n" } else { "  **NOT HIGH -> pull-ups/mux**\n" });
                self.phase = Phase::Scan;
            }

            // P2 — address sweep (i2cdetect). Each probe = S·addr+W·(A|NAK)·P. Prints every ACK.
            Phase::Scan => {
                if self.addr == SCAN_LO {
                    // Guard: a wedged bus false-ACKs every address. Don't burn 112 timeouts on it —
                    // report and skip to the transaction patterns (which carry their own recover).
                    let (sda, scl) = i2c::bus_levels(p);
                    if !(sda && scl) {
                        uart::puts(p, "P2 scan:  **bus wedged (SDA=");
                        uart::putc(p, if sda { b'1' } else { b'0' });
                        uart::puts(p, " SCL=");
                        uart::putc(p, if scl { b'1' } else { b'0' });
                        uart::puts(p, ") -> skipping scan**\n");
                        self.phase = Phase::Write;
                        return Some(1);
                    }
                    uart::puts(p, "P2 scan:  ");
                }
                let mut n = 0;
                while n < SCAN_BATCH && self.addr <= SCAN_HI {
                    if i2c::probe(p, self.addr) {
                        uart::hex8(p, self.addr);
                        uart::putc(p, b' ');
                        if self.found == 0 {
                            self.target = self.addr; // first device found drives P3–P8
                        }
                        self.found += 1;
                    }
                    self.addr += 1;
                    n += 1;
                    crate::pet_wdt(p); // a NACK burns the full timeout; feed per probe
                }
                if self.addr > SCAN_HI {
                    if self.found == 0 {
                        uart::puts(p, "(none) ");
                    }
                    uart::puts(p, "-> target 0x");
                    uart::hex8(p, self.target);
                    uart::putc(p, b'\n');
                    self.phase = Phase::Write;
                }
            }

            // P3 — single-byte write. S·addr+W·A·reg·A·val·A·P.
            Phase::Write => {
                let ok = i2c::write(p, self.target, &[TARGET_REG, TARGET_VAL]);
                line(p, "P3 write: ", "S addr+W reg val P", ok);
                self.phase = Phase::Read;
            }

            // P4 — single-byte read (no reg pointer). S·addr+R·A·data·NAK·P.
            Phase::Read => {
                let mut b = [0u8; 1];
                let ok = i2c::read(p, self.target, &mut b);
                line_val(p, "P4 read:  ", "S addr+R data NAK P", ok, b[0]);
                self.phase = Phase::RegRead;
            }

            // P5 — repeated-START register read. S·addr+W·A·reg·A·Sr·addr+R·A·data·NAK·P.
            // The classic sensor shape; exposes the phantom-TXIFG repeated-start bug.
            Phase::RegRead => {
                let mut b = [0u8; 1];
                let ok = i2c::read_reg(p, self.target, TARGET_REG, &mut b);
                line_val(p, "P5 regrd: ", "S addr+W reg Sr addr+R data NAK P", ok, b[0]);
                self.phase = Phase::Burst;
            }

            // P6 — multi-byte burst, walking ones. One S, N data bytes each ACKed, one P.
            Phase::Burst => {
                let ok = i2c::write(p, self.target, &[TARGET_REG, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80]);
                line(p, "P6 burst: ", "S addr+W reg +8 bytes P", ok);
                self.phase = Phase::Eye;
            }

            // P7 — byte-value eye: alternating 0xAA / 0x55 to the target. On the scope SDA shows an
            // unmistakable alternating pattern (confirms MSB-first bit order, signal vs noise).
            // Needs an ACKing device — the address byte must ACK or the eUSCI aborts before data.
            Phase::Eye => {
                if self.eye == 0 {
                    uart::puts(p, "P7 eye:   0xAA/0x55 x");
                    uart::dec(p, EYE_REPS);
                    uart::puts(p, " -> scope SDA\n");
                }
                let mut n = 0;
                while n < EYE_BATCH && self.eye < EYE_REPS {
                    let pat = if self.eye & 1 == 0 { 0xAA } else { 0x55 };
                    let _ = i2c::write(p, self.target, &[pat]);
                    self.eye += 1;
                    n += 1;
                }
                if self.eye >= EYE_REPS {
                    self.phase = Phase::Ladder;
                }
            }

            // P8 — BRW frequency ladder. One verified reg-read per step; the SCL period steps
            // 100µs→10µs on the trace. Restores 100 kHz when done.
            Phase::Ladder => {
                let s = &LADDER[self.li];
                i2c::set_brw(p, s.brw);
                let mut b = [0u8; 1];
                let ok = i2c::read_reg(p, self.target, TARGET_REG, &mut b);
                uart::puts(p, "P8 ");
                uart::dec(p, s.khz);
                uart::puts(p, " kHz: ");
                uart::puts(p, if ok { "OK\n" } else { "FAIL\n" });
                self.li += 1;
                if self.li >= LADDER.len() {
                    i2c::set_brw(p, 10); // restore operating clock
                    self.phase = Phase::Done;
                }
            }

            Phase::Done => {
                uart::puts(p, "-- signature battery complete -> POST --\n");
                cx.mode = Mode::Post;
                return Some(200);
            }
        }
        Some(1)
    }

    fn name(&self) -> &'static str {
        "sig"
    }
}

/// `<tag><sig>  OK/FAIL` — one pattern result keyed to its expected decoded signature.
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
