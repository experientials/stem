//! eUSCI_B0 I2C master, 100 kHz — the **Leaf** bus (sensors/LCD) in `bsp`, shared by diag and prod
//! (the union of both: diag's `set_brw` margin sweep + prod's `bus_levels`/`read_reg_pumped`/empty-
//! write probe). Polled/blocking with bounded waits so a stuck bus reports failure instead of
//! hanging. UCB0 is present on all four families. Pins routed via `board::route_leaf_i2c_pins`.
//!
//! NOTE: on the FR247x this is the *sensor-master* bus. The MCU-bus I2C SLAVE surface (eUSCI_B1,
//! answering the SoM) is a separate driver — not this file.

use crate::pac::Peripherals;

const UCSWRST: u16 = 0x0001;
const UCMODE_3: u16 = 0x0600;
const UCMST: u16 = 0x0800;
const UCSYNC: u16 = 0x0100;
const UCSSEL_SMCLK: u16 = 0x0080;
const UCTR: u16 = 0x0010;
const UCTXSTT: u16 = 0x0002;
const UCTXSTP: u16 = 0x0004;
const UCNACKIFG: u16 = 0x0020;
const UCTXIFG0: u16 = 0x0002;
const UCRXIFG0: u16 = 0x0001;

/// Poll bound. One I2C byte @100 kHz is ~90 µs; this bails in well under a ms of "stuck".
const SPIN: u16 = 4000;

const SDA_BIT: u8 = 0x04; // P1.2 / UCB0SDA
const SCL_BIT: u8 = 0x08; // P1.3 / UCB0SCL

fn bitdelay() {
    for _ in 0..30u16 {
        msp430::asm::nop();
    }
}

fn sda_high(p: &Peripherals) -> bool {
    p.p1.p1in().read().bits() & SDA_BIT != 0
}

/// Idle levels of the bus pads (P1IN reflects the real pad even when muxed to eUSCI): `(sda, scl)`
/// high?. A low SDA at idle = a wedged bus (a slave holding it) → every probe false-ACKs.
pub fn bus_levels(p: &Peripherals) -> (bool, bool) {
    let lv = p.p1.p1in().read().bits();
    (lv & SDA_BIT != 0, lv & SCL_BIT != 0)
}

/// True when both pads are idle-high (bus free).
fn bus_idle(p: &Peripherals) -> bool {
    let (sda, scl) = bus_levels(p);
    sda && scl
}

/// Recover a wedged bus. Two wedge modes are handled (both observed on the FR2355 Leaf bus):
///   • **SCL held low by our own eUSCI** — an aborted/timed-out read leaves the master holding the
///     clock (the dominant mode here). Asserting UCSWRST makes the module release SDA/SCL, so the
///     pads float back to the pull-ups. Cheap and decisive; no bit-banging needed.
///   • **SDA held low by a slave** mid-byte — bit-bang up to 9 SCL pulses until it releases, then a
///     manual STOP (the classic 9-clock recovery).
///
/// A slave stretching **SCL** can't be forced safely (driving against it is contention), so that
/// case is reported as failure rather than fought. Returns true iff the bus ends idle-high.
///
/// Preserves the caller's UCSWRST state: a live module is left live+configured; a module already in
/// reset (e.g. `init` called us) is left in reset for the caller to finish configuring.
pub fn recover(p: &Peripherals) -> bool {
    if bus_idle(p) {
        return true; // already free — fast path (also the SCL-low gate for callers)
    }
    let was_live = ctlw0(p) & UCSWRST == 0;
    // Release any eUSCI-side hold first: in UCSWRST the module lets go of SDA/SCL and the pads float
    // to the pull-ups. This clears a master-held SCL (or SDA) outright, and is the precondition for
    // the bit-bang dance below (so we aren't fighting the module for the pins).
    p.e_usci_b0.ucb0ctlw0().modify(|r, w| unsafe { w.bits(r.bits() | UCSWRST) });
    bitdelay();
    // If SDA is still low, a slave is holding it mid-byte → clock it out (≤9 pulses) + manual STOP.
    if !sda_high(p) {
        p.p1.p1sel0().modify(|r, w| unsafe { w.bits(r.bits() & !(SDA_BIT | SCL_BIT)) });
        p.p1.p1out().modify(|r, w| unsafe { w.bits(r.bits() | SCL_BIT) });
        p.p1.p1dir().modify(|r, w| unsafe { w.bits((r.bits() & !SDA_BIT) | SCL_BIT) });
        let mut i = 0u8;
        while i < 9 && !sda_high(p) {
            p.p1.p1out().modify(|r, w| unsafe { w.bits(r.bits() & !SCL_BIT) });
            bitdelay();
            p.p1.p1out().modify(|r, w| unsafe { w.bits(r.bits() | SCL_BIT) });
            bitdelay();
            i += 1;
        }
        // Manual STOP: with SCL high, drive SDA low then release (rising edge = STOP).
        p.p1.p1out().modify(|r, w| unsafe { w.bits(r.bits() & !SDA_BIT) });
        p.p1.p1dir().modify(|r, w| unsafe { w.bits(r.bits() | SDA_BIT) });
        bitdelay();
        p.p1.p1out().modify(|r, w| unsafe { w.bits(r.bits() | SCL_BIT) });
        bitdelay();
        p.p1.p1dir().modify(|r, w| unsafe { w.bits(r.bits() & !SDA_BIT) });
        bitdelay();
        p.p1.p1sel0().modify(|r, w| unsafe { w.bits(r.bits() | SDA_BIT | SCL_BIT) }); // pins → eUSCI
    }
    // Leave the module as the caller expects: re-enable it if it was live; else keep it in reset.
    if was_live {
        p.e_usci_b0.ucb0ctlw0().modify(|r, w| unsafe { w.bits(r.bits() & !UCSWRST) });
    }
    bus_idle(p)
}

pub fn init(p: &Peripherals) {
    p.e_usci_b0.ucb0ctlw0().write(|w| unsafe { w.bits(UCSWRST) }); // hold in reset (releases pins)
    recover(p); // free a slave holding SDA low before we master the bus
    p.e_usci_b0
        .ucb0ctlw0()
        .modify(|r, w| unsafe { w.bits(r.bits() | UCMODE_3 | UCMST | UCSYNC | UCSSEL_SMCLK) });
    p.e_usci_b0.ucb0brw().write(|w| unsafe { w.bits(10) }); // 1 MHz / 10 = 100 kHz
    p.e_usci_b0
        .ucb0ctlw0()
        .modify(|r, w| unsafe { w.bits(r.bits() & !UCSWRST) });
}

/// Change the I²C bit clock: SCL = SMCLK (1 MHz) / `brw`. eUSCI requires UCSWRST to alter the baud
/// divider, so we toggle it; mode/master/clock-source are retained. Used by diag's stress margin
/// sweep to push SCL from 100 kHz (brw=10) up to 1 MHz (brw=1), and to restore it afterwards.
/// (diag-side; prod doesn't margin-sweep — LTO-stripped from the prod image.)
pub fn set_brw(p: &Peripherals, brw: u16) {
    p.e_usci_b0
        .ucb0ctlw0()
        .modify(|r, w| unsafe { w.bits(r.bits() | UCSWRST) });
    p.e_usci_b0.ucb0brw().write(|w| unsafe { w.bits(brw) });
    p.e_usci_b0
        .ucb0ctlw0()
        .modify(|r, w| unsafe { w.bits(r.bits() & !UCSWRST) });
}

fn ifg(p: &Peripherals) -> u16 {
    p.e_usci_b0.ucb0ifg().read().bits()
}

fn ctlw0(p: &Peripherals) -> u16 {
    p.e_usci_b0.ucb0ctlw0().read().bits()
}

fn stop(p: &Peripherals) {
    p.e_usci_b0
        .ucb0ctlw0()
        .modify(|r, w| unsafe { w.bits(r.bits() | UCTXSTP) });
    let mut n = 0u16;
    while ctlw0(p) & UCTXSTP != 0 {
        n += 1;
        if n >= SPIN {
            break;
        }
    }
}

/// Address-only probe: START + addr(W) + STOP. `true` if the device ACKs. A stuck bus returns
/// `false` rather than hanging. (TI's recommended eUSCI_B presence-probe order: read UCNACKIFG only
/// AFTER the STOP completes.)
pub fn probe(p: &Peripherals, addr: u8) -> bool {
    if !bus_idle(p) {
        recover(p);
    }
    p.e_usci_b0.ucb0i2csa().write(|w| unsafe { w.bits(addr as u16) });
    p.e_usci_b0
        .ucb0ifg()
        .modify(|r, w| unsafe { w.bits(r.bits() & !(UCNACKIFG | UCTXIFG0)) });
    p.e_usci_b0
        .ucb0ctlw0()
        .modify(|r, w| unsafe { w.bits(r.bits() | UCTR | UCTXSTT) });
    let mut n = 0u16;
    while ctlw0(p) & UCTXSTT != 0 {
        n += 1;
        if n >= SPIN {
            break;
        }
    }
    stop(p);
    let acked = ifg(p) & UCNACKIFG == 0;
    p.e_usci_b0
        .ucb0ifg()
        .modify(|r, w| unsafe { w.bits(r.bits() & !(UCNACKIFG | UCTXIFG0)) });
    acked
}

/// Write `data` to `addr` (caller includes any register pointer as data[0]). `false` on NACK or
/// timeout; never hangs. An empty `data` is an address-only presence probe (see hal.rs).
pub fn write(p: &Peripherals, addr: u8, data: &[u8]) -> bool {
    if data.is_empty() {
        return probe(p, addr);
    }
    if !bus_idle(p) {
        recover(p);
    }
    p.e_usci_b0.ucb0i2csa().write(|w| unsafe { w.bits(addr as u16) });
    p.e_usci_b0
        .ucb0ifg()
        .modify(|r, w| unsafe { w.bits(r.bits() & !(UCNACKIFG | UCTXIFG0)) });
    p.e_usci_b0
        .ucb0ctlw0()
        .modify(|r, w| unsafe { w.bits(r.bits() | UCTR | UCTXSTT) });

    for &byte in data {
        let mut n = 0u16;
        while ifg(p) & (UCTXIFG0 | UCNACKIFG) == 0 {
            n += 1;
            if n >= SPIN {
                stop(p);
                return false;
            }
        }
        if ifg(p) & UCNACKIFG != 0 {
            stop(p);
            p.e_usci_b0
                .ucb0ifg()
                .modify(|r, w| unsafe { w.bits(r.bits() & !UCNACKIFG) });
            return false;
        }
        p.e_usci_b0.ucb0txbuf().write(|w| unsafe { w.bits(byte as u16) });
    }
    let mut n = 0u16;
    while ifg(p) & UCTXIFG0 == 0 {
        n += 1;
        if n >= SPIN {
            stop(p);
            return false;
        }
    }
    stop(p);
    true
}

/// Set the register pointer at `addr` (transmitter, no STOP), repeated-START as receiver, and read
/// `buf.len()` bytes into `buf`. `false` on NACK/timeout; never hangs. Ported verbatim from diag's
/// proven `i2c::read_reg` (same FR2476 eUSCI_B) — the register-read primitive the shared device
/// drivers use via `hal::EusciI2c`'s `write_read`.
pub fn read_reg(p: &Peripherals, addr: u8, reg: u8, buf: &mut [u8]) -> bool {
    read_reg_pumped(p, addr, reg, buf, || {})
}

/// Like [`read_reg`], but calls `pump` at the top of every spin-wait. Used by the B0↔B1 loopback
/// self-test (`stem::loopback_selftest`) to service the eUSCI_B1 SLAVE (`stem::poll`) while this
/// master reads it on the SAME MCU — without the pump the slave's clock-stretch deadlocks the blocking
/// master. Normal callers use [`read_reg`] (`pump = || {}`, which the optimiser drops). Behaviourally
/// identical to the ported diag `i2c::read_reg` apart from the injected pump.
pub fn read_reg_pumped(
    p: &Peripherals,
    addr: u8,
    reg: u8,
    buf: &mut [u8],
    mut pump: impl FnMut(),
) -> bool {
    if buf.is_empty() {
        return false;
    }
    if !bus_idle(p) {
        recover(p);
    }
    // Phase 1: write the register pointer (transmitter), no STOP.
    p.e_usci_b0.ucb0i2csa().write(|w| unsafe { w.bits(addr as u16) });
    p.e_usci_b0
        .ucb0ifg()
        .modify(|r, w| unsafe { w.bits(r.bits() & !(UCNACKIFG | UCTXIFG0)) });
    p.e_usci_b0
        .ucb0ctlw0()
        .modify(|r, w| unsafe { w.bits(r.bits() | UCTR | UCTXSTT) });
    let mut n = 0u16;
    while ifg(p) & (UCTXIFG0 | UCNACKIFG) == 0 {
        pump();
        n += 1;
        if n >= SPIN {
            stop(p);
            return false;
        }
    }
    if ifg(p) & UCNACKIFG != 0 {
        stop(p);
        p.e_usci_b0
            .ucb0ifg()
            .modify(|r, w| unsafe { w.bits(r.bits() & !UCNACKIFG) });
        return false;
    }
    p.e_usci_b0.ucb0txbuf().write(|w| unsafe { w.bits(reg as u16) });
    n = 0;
    while ifg(p) & UCTXIFG0 == 0 {
        pump();
        n += 1;
        if n >= SPIN {
            stop(p);
            return false;
        }
    }
    // Phase 2: repeated START as receiver.
    p.e_usci_b0
        .ucb0ctlw0()
        .modify(|r, w| unsafe { w.bits((r.bits() & !UCTR) | UCTXSTT) });
    let len = buf.len();
    for i in 0..len {
        if i == len - 1 {
            // last byte: wait for the repeated-START/address to go out, then arm NACK+STOP.
            n = 0;
            while ctlw0(p) & UCTXSTT != 0 {
                pump();
                n += 1;
                if n >= SPIN {
                    stop(p);
                    return false;
                }
            }
            p.e_usci_b0
                .ucb0ctlw0()
                .modify(|r, w| unsafe { w.bits(r.bits() | UCTXSTP) });
        }
        n = 0;
        while ifg(p) & UCRXIFG0 == 0 {
            pump();
            n += 1;
            if n >= SPIN {
                stop(p);
                return false;
            }
        }
        buf[i] = p.e_usci_b0.ucb0rxbuf().read().bits() as u8;
    }
    n = 0;
    while ctlw0(p) & UCTXSTP != 0 {
        pump();
        n += 1;
        if n >= SPIN {
            break;
        }
    }
    true
}

/// Pointer-less receiver read: START(R) + read `buf.len()` bytes + STOP. No register write first —
/// for devices that return a result to a bare read (e.g. the Si7021 no-hold measurement result and
/// its ID/firmware-rev sequences). Ported verbatim from diag's proven `i2c::read`. Bounded/self-
/// healing like `read_reg`'s receive phase: never hangs; returns false on NACK (device busy/absent)
/// or timeout — which is exactly the signal a no-hold poll needs ("not ready yet").
pub fn read(p: &Peripherals, addr: u8, buf: &mut [u8]) -> bool {
    if buf.is_empty() {
        return false;
    }
    if !bus_idle(p) {
        recover(p);
    }
    p.e_usci_b0.ucb0i2csa().write(|w| unsafe { w.bits(addr as u16) });
    p.e_usci_b0
        .ucb0ifg()
        .modify(|r, w| unsafe { w.bits(r.bits() & !(UCNACKIFG | UCRXIFG0)) });
    // Receiver (UCTR=0) + START.
    p.e_usci_b0
        .ucb0ctlw0()
        .modify(|r, w| unsafe { w.bits((r.bits() & !UCTR) | UCTXSTT) });
    let len = buf.len();
    for i in 0..len {
        if i == len - 1 {
            // Last byte (also the only byte when len==1): wait for the address to clear, then arm
            // NACK+STOP so the eUSCI NACKs the final byte and releases the bus.
            let mut n = 0u16;
            while ctlw0(p) & UCTXSTT != 0 {
                n += 1;
                if n >= SPIN {
                    stop(p);
                    return false;
                }
            }
            p.e_usci_b0
                .ucb0ctlw0()
                .modify(|r, w| unsafe { w.bits(r.bits() | UCTXSTP) });
        }
        let mut n = 0u16;
        while ifg(p) & UCRXIFG0 == 0 {
            n += 1;
            if n >= SPIN {
                stop(p);
                return false;
            }
        }
        buf[i] = p.e_usci_b0.ucb0rxbuf().read().bits() as u8;
    }
    let mut n = 0u16;
    while ctlw0(p) & UCTXSTP != 0 {
        n += 1;
        if n >= SPIN {
            break;
        }
    }
    true
}
