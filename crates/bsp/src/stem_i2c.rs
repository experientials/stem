//! Stem-bus I²C **SLAVE** transport — the low-level eUSCI_B driver for the control surface the SoM (and
//! other masters) read/write our register interface on. Sibling to the Leaf-master [`crate::i2c`]: `i2c`
//! is UCB0 as MASTER (sensor bus); this is the node's SLAVE bus. The protocol/dispatch (register file,
//! command-pointer) lives ABOVE this, in the `deepsleep` crate — this file is transport ONLY, so diag
//! and prod bring the slave up identically (DEEPSLEEP-EXTRACTION.md §A).
//!
//! **One transport, two peripherals.** The register *layout* is identical across eUSCI_B instances; only
//! the instance + mode-field name differ, isolated in the [`hw`] seam:
//!   • DUAL families (fr2476/fr2355/fr2155): eUSCI_B1 (`ucb1*`) — a dedicated slave bus, independent of
//!     the eUSCI_B0 sensor-master, so it never disturbs enumeration.
//!   • FR2433 (single-I²C slave-only): its ONE eUSCI_B0 (`ucb0*`, mode-split PAC field
//!     `usci_b0_i2c_mode`) — this bus IS the node's whole job (no sensor master).
//! The **pins** differ per module and are owned by `board::route_stem_i2c_pins` (P3.2/3.6 FR2476,
//! P4.6/4.7 FR2355/FR2155, P1.2/3 FR2433), NOT here — same split as `route_console_uart_pins`.
//!
//! **Event seam.** [`next_event`] decodes the eUSCI_B IFG into one [`SlaveEvent`] per call; the caller
//! drains them and maps each onto its register file. `TxReq` means the master is clocking a read and the
//! eUSCI wants a byte — the caller computes it and answers with [`provide_tx`]. This is the invented
//! slave seam (no `embedded-hal` slave trait exists). eUSCI_B auto-clock-stretches SCL until the buffer
//! is serviced, so latency between an IFG and `next_event` shows up as clock-stretch on the master, not
//! lost data (`slau445i` §24.3). **Phase 1a: polled** (caller drains every main-loop pass). Phase 1b
//! adds the shared eUSCI_B ISR (`slau445i` §24.3.11) that wakes the CPU from LPM and masks its own
//! source; the servicing path below is unchanged — only *when* it runs changes.

use crate::pac::Peripherals;

// ── CTLW0 / I2COA0 / IFG bit definitions (eUSCI_B, slau445 §24). Shared: the register *layout* is
// identical across instances; only the accessor name differs (handled in `hw`). ──
const UCSWRST: u16 = 0x0001;
const UCMODE_3: u16 = 0x0600; // I2C mode
const UCSYNC: u16 = 0x0100;
const UCOAEN: u16 = 0x0400; // I2COA0: own-address enable
const UCRXIFG0: u16 = 0x0001;
const UCTXIFG0: u16 = 0x0002;
const UCSTTIFG: u16 = 0x0004; // START matching our own address
const UCSTPIFG: u16 = 0x0008;
const UCNACKIFG: u16 = 0x0020;

/// Chip-specific seam: the eUSCI_B **instance** + mode-field name. The two implementations are register-
/// for-register identical apart from the peripheral accessor (ucb1 vs ucb0) + the mode-split IFG field
/// name, so the shared protocol above never mentions a concrete peripheral. Pin routing is NOT here — it
/// lives in `board::route_stem_i2c_pins` (per-module), called from [`init`].
#[cfg(feature = "_leaf")]
mod hw {
    use super::{Peripherals, UCMODE_3, UCOAEN, UCSWRST, UCSYNC};

    /// Configure eUSCI_B1 as an I²C slave at `addr` (own-address). Pins are routed by the caller via
    /// `board::route_stem_i2c_pins` BEFORE this. Register-identical across the dual family.
    pub fn init(p: &Peripherals, addr: u16) {
        p.e_usci_b1.ucb1ctlw0().write(|w| unsafe { w.bits(UCSWRST) }); // hold in reset
        p.e_usci_b1
            .ucb1ctlw0()
            .modify(|r, w| unsafe { w.bits(r.bits() | UCMODE_3 | UCSYNC) }); // I2C SLAVE (no UCMST)
        p.e_usci_b1.ucb1i2coa0().write(|w| unsafe { w.bits(addr | UCOAEN) }); // role-based own-address
        p.e_usci_b1.ucb1ctlw0().modify(|r, w| unsafe { w.bits(r.bits() & !UCSWRST) }); // release
        p.e_usci_b1.ucb1ifg().write(|w| unsafe { w.bits(0) }); // clear stale flags
    }
    #[inline]
    pub fn ifg(p: &Peripherals) -> u16 {
        p.e_usci_b1.ucb1ifg().read().bits()
    }
    #[inline]
    pub fn clear(p: &Peripherals, bit: u16) {
        p.e_usci_b1.ucb1ifg().modify(|r, w| unsafe { w.bits(r.bits() & !bit) });
    }
    #[inline]
    pub fn rx(p: &Peripherals) -> u8 {
        p.e_usci_b1.ucb1rxbuf().read().bits() as u8 // read clears UCRXIFG
    }
    #[inline]
    pub fn tx(p: &Peripherals, v: u8) {
        p.e_usci_b1.ucb1txbuf().write(|w| unsafe { w.bits(v as u16) }); // write clears UCTXIFG
    }
}

#[cfg(feature = "fr2433")]
mod hw {
    use super::{Peripherals, UCMODE_3, UCOAEN, UCSWRST, UCSYNC};

    /// Configure FR2433's single eUSCI_B0 as the MCU-bus slave at `addr`. Pins (P1.2/P1.3) are routed by
    /// the caller via `board::route_stem_i2c_pins` BEFORE this. Mode-split PAC → fields are `ucb0*_i2c`.
    pub fn init(p: &Peripherals, addr: u16) {
        let b = &p.usci_b0_i2c_mode;
        b.ucb0ctlw0().write(|w| unsafe { w.bits(UCSWRST) }); // hold in reset
        b.ucb0ctlw0()
            .modify(|r, w| unsafe { w.bits(r.bits() | UCMODE_3 | UCSYNC) }); // I2C SLAVE (no UCMST)
        b.ucb0i2coa0().write(|w| unsafe { w.bits(addr | UCOAEN) }); // role-based own-address
        b.ucb0ctlw0().modify(|r, w| unsafe { w.bits(r.bits() & !UCSWRST) }); // release
        b.ucb0ifg_i2c().write(|w| unsafe { w.bits(0) }); // clear stale flags
    }
    #[inline]
    pub fn ifg(p: &Peripherals) -> u16 {
        p.usci_b0_i2c_mode.ucb0ifg_i2c().read().bits()
    }
    #[inline]
    pub fn clear(p: &Peripherals, bit: u16) {
        p.usci_b0_i2c_mode
            .ucb0ifg_i2c()
            .modify(|r, w| unsafe { w.bits(r.bits() & !bit) });
    }
    #[inline]
    pub fn rx(p: &Peripherals) -> u8 {
        p.usci_b0_i2c_mode.ucb0rxbuf().read().bits() as u8 // read clears UCRXIFG
    }
    #[inline]
    pub fn tx(p: &Peripherals, v: u8) {
        p.usci_b0_i2c_mode.ucb0txbuf().write(|w| unsafe { w.bits(v as u16) }); // write clears UCTXIFG
    }
}

/// One decoded slave-bus event — the seam between this transport and the `deepsleep` register/command
/// dispatch above it. `slau445i` §24.3.11: one shared eUSCI_B I²C vector; the flags below are the states
/// it multiplexes. See [`next_event`].
pub enum SlaveEvent {
    /// START matched our own address — the next `Rx` is the command byte (`UCSTTIFG`).
    Start,
    /// STOP — transaction over (`UCSTPIFG`).
    Stop,
    /// A byte the master WROTE to us, already read out of RXBUF (`UCRXIFG0`, cleared).
    Rx(u8),
    /// The master is READING and the eUSCI wants the next byte (`UCTXIFG0`, NOT yet cleared). The caller
    /// MUST answer with [`provide_tx`] before the next [`next_event`] call, or it re-reports `TxReq`.
    TxReq,
    /// The master NACKed (end of a read, typically) (`UCNACKIFG`, cleared).
    Nack,
}

/// Configure the node's eUSCI_B as an I²C slave at the **role-based** `addr`
/// (`stembus::role::role_for_model(..).addr()`; see I2C-API.md) and route its pins
/// (`board::route_stem_i2c_pins`, per-module). Register programming is in [`hw`].
pub fn init(p: &Peripherals, addr: u16) {
    crate::board::route_stem_i2c_pins(p); // chip-specific pin mux (SEL=01); owned by the board layer
    hw::init(p, addr);
}

/// Decode the next pending slave event (non-blocking), or `None` when no IFG is set. Priority order
/// matches the transaction phases: START → NACK → RX → TX → STOP. Each call clears the flag it reports,
/// EXCEPT `TxReq` (cleared by the answering [`provide_tx`] writing TXBUF). Drain in a loop:
/// ```ignore
/// while let Some(ev) = stem_i2c::next_event(p) {
///     match ev { SlaveEvent::TxReq => stem_i2c::provide_tx(p, byte), /* … */ }
/// }
/// ```
pub fn next_event(p: &Peripherals) -> Option<SlaveEvent> {
    let ifg = hw::ifg(p);
    if ifg & UCSTTIFG != 0 {
        hw::clear(p, UCSTTIFG);
        return Some(SlaveEvent::Start);
    }
    if ifg & UCNACKIFG != 0 {
        hw::clear(p, UCNACKIFG);
        return Some(SlaveEvent::Nack);
    }
    if ifg & UCRXIFG0 != 0 {
        return Some(SlaveEvent::Rx(hw::rx(p))); // rx() clears UCRXIFG
    }
    if ifg & UCTXIFG0 != 0 {
        return Some(SlaveEvent::TxReq); // left set; provide_tx clears it by writing TXBUF
    }
    if ifg & UCSTPIFG != 0 {
        hw::clear(p, UCSTPIFG);
        return Some(SlaveEvent::Stop);
    }
    None
}

/// Answer a [`SlaveEvent::TxReq`]: load the byte the master reads. Writing TXBUF clears `UCTXIFG0`.
#[inline]
pub fn provide_tx(p: &Peripherals, v: u8) {
    hw::tx(p, v);
}
