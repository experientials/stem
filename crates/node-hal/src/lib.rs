#![no_std]

//! `node-hal` — the host-clean **hardware-seam traits** the `deepsleep` node dispatch is generic over,
//! so `deepsleep` names no concrete MCU PAC (DEEPSLEEP-EXTRACTION.md Phase 6a). This crate is pure: it
//! depends on NO PAC, on `bsp`, or on `msp430`. The MSP430 implementation is a newtype wrapper in `bsp`
//! (`bsp::StemHw`) that `impl`s these traits; a future TI-M0 `bsp` provides its own.
//!
//! The seams are **narrow and per-concern** — each generic function in `deepsleep` is bounded by exactly
//! the trait(s) its body uses:
//!   • [`StemSlave`]  — the Stem-bus I²C **slave** event transport (the invented slave seam; no
//!     `embedded-hal` slave trait exists). Methods take `&self`; the handle (PAC) is captured by the impl.
//!   • [`GpioBanks`]  — the PCA9698 IP/OP/IOC **bank backing** onto real port pins (fr2433) or a no-op
//!     shadow (dual, no bank↔port map yet).
//!   • [`LeafMaster`] — the Leaf-I²C **master** register read with a serviced spin-pump, used only by the
//!     dual loopback self-test (gate behind the Leaf bus in the impl crate).
//!   • [`Console`]    — the backchannel text-console surface (`bsp::fmt`'s formatters) the `_dual`
//!     enumerate report writes over (DEEPSLEEP-EXTRACTION.md Phase 6b).
//!   • [`LeafBus`]    — the Leaf-I²C **master** bus seam enumerate/mode drive: a fresh `embedded-hal`
//!     master handle plus the bus-health/recovery + acquire/release role ops (Phase 6b).

/// Host-clean chip identity — the two facts `post`/`status` need about the detected part, lifted out of
/// the chip-specific `bsp::model::Model` enum so those files name no concrete PAC type
/// (DEEPSLEEP-EXTRACTION.md Phase 6c). The bin builds it once from the detected model
/// (`ChipId { device_id: m.device_id(), family_ok: m.matches_build_family() }`) and passes it down.
#[derive(Clone, Copy)]
pub struct ChipId {
    /// The 16-bit TLV Device ID of the detected part (`Model::device_id`).
    pub device_id: u16,
    /// True if the detected part belongs to the family this image was compiled for
    /// (`Model::matches_build_family`); false flags a wrong-family flash.
    pub family_ok: bool,
}

/// One decoded Stem-bus slave event — the seam between the eUSCI_B (or any MCU's) I²C-slave transport and
/// the `deepsleep` register/command dispatch above it. The transport decodes one event per
/// [`StemSlave::next_event`] call; the dispatch maps each onto its register file.
///
/// Moved here from `bsp::stem_i2c` (Phase 6a) so the dispatch is PAC-free; `bsp::stem_i2c` re-exports it
/// (`pub use node_hal::SlaveEvent;`) for unchanged call sites.
pub enum SlaveEvent {
    /// START matched our own address — the next `Rx` is the command byte.
    Start,
    /// STOP — transaction over.
    Stop,
    /// A byte the master WROTE to us, already read out of the RX buffer.
    Rx(u8),
    /// The master is READING and the transport wants the next byte. The caller MUST answer with
    /// [`StemSlave::provide_tx`] before the next [`StemSlave::next_event`] call, or it re-reports `TxReq`.
    TxReq,
    /// The master NACKed (end of a read, typically).
    Nack,
}

/// The Stem-bus I²C **slave** transport seam. `init` brings the slave up at the role-based address (+
/// routes its pins); `next_event` decodes one pending [`SlaveEvent`]; `provide_tx` answers a `TxReq`;
/// `irq_{enable,disable}` gate the slave wake interrupt for LPM (the ISR itself stays in the bin). All
/// methods take `&self` — the concrete hardware handle is captured by the implementing newtype.
pub trait StemSlave {
    /// Bring the slave up at the role-based own-address `addr` and route its pins.
    fn init(&self, addr: u16);
    /// Decode the next pending slave event (non-blocking), or `None` when no event is pending.
    fn next_event(&self) -> Option<SlaveEvent>;
    /// Answer a [`SlaveEvent::TxReq`]: load the byte the master reads.
    fn provide_tx(&self, v: u8);
    /// Enable the slave interrupt(s) so a bus event wakes the CPU from LPM.
    fn irq_enable(&self);
    /// Mask the slave interrupt(s) — the "ISR masks its own source" idiom.
    fn irq_disable(&self);
}

/// The PCA9698 GPIO **bank backing** seam: maps IP/OP/IOC banks onto real MCU port registers. Chip-
/// specific (which bits are safe to drive), so the impl is per-chip. On a node with no bank↔port map
/// (the dual families today) the impl is the shadow no-op: `read_ip` returns 0, `drive_op`/`set_dir` do
/// nothing — byte-identical to the former inline dual stubs.
pub trait GpioBanks {
    /// IP (Input Port): live pin state XOR the `pol` polarity shadow, restricted to the backed bits.
    fn read_ip(&self, bank: u8, pol: u8) -> u8;
    /// OP (Output Port): drive the bank's port output, backed bits only.
    fn drive_op(&self, bank: u8, val: u8);
    /// IOC (I/O Config / direction): set the bank's port direction from the PCA9698 config byte.
    fn set_dir(&self, bank: u8, ioc: u8);
}

/// The Leaf-I²C **master** register-read seam, with a `pump` closure serviced on every spin-wait so the
/// on-chip slave can be serviced during a same-MCU loopback read (breaks the clock-stretch deadlock).
/// Only the dual loopback self-test uses it; the impl is gated behind the Leaf bus's presence.
pub trait LeafMaster {
    /// Set the register pointer at `addr`, repeated-START as receiver, and read `buf.len()` bytes into
    /// `buf`, calling `pump` at the top of every spin-wait. `false` on NACK/timeout; never hangs.
    fn read_reg_pumped(&self, addr: u8, reg: u8, buf: &mut [u8], pump: &mut dyn FnMut()) -> bool;
}

/// The backchannel **console** text seam — the exact surface of `bsp::fmt` (DEEPSLEEP-EXTRACTION.md
/// Phase 6b), so the `console`-gated enumerate report names no concrete PAC. Integer-only (no float on
/// MSP430); every method takes `&self` and bottoms out in the impl's board console. The `_dual`
/// enumerate report is the consumer; `bsp::StemHw` delegates each to `bsp::fmt::*`.
pub trait Console {
    /// Emit one raw byte (no newline translation).
    fn putc(&self, c: u8);
    /// Print a string, translating `\n` → `\r\n` for a raw serial terminal.
    fn puts(&self, s: &str);
    /// Two-digit uppercase hex of a byte.
    fn hex8(&self, v: u8);
    /// Four-digit uppercase hex of a 16-bit word.
    fn hex16(&self, v: u16);
    /// Unsigned decimal.
    fn dec(&self, v: u16);
    /// Signed decimal (`dec` is unsigned).
    fn dec_i16(&self, v: i16);
    /// Signed hundredths as fixed-point, e.g. `-1234 -> "-12.34"` (centi-units; no float).
    fn fixed2(&self, centi: i32);
}

/// The Leaf-I²C **master** bus seam enumerate/mode drive (DEEPSLEEP-EXTRACTION.md Phase 6b). `bus()`
/// hands out a fresh `embedded-hal` master handle (the device drivers talk to it); `recover`/`bus_levels`
/// are the stuck-bus-health ops; `acquire`/`release` are the Sensing↔Passive bus-role handoff. The
/// associated `Bus` type is tied to the **impl's** lifetime (NOT a GAT / method lifetime) so the MSP430
/// impl (`type Bus = bsp::hal::EusciI2c<'a>`) compiles on the current nightly. Dual-I²C families only —
/// the impl is gated behind the Leaf bus's presence (`_leaf`), as fr2433 has no Leaf master.
pub trait LeafBus {
    /// A fresh `embedded-hal` master handle on the Leaf bus (cheap — borrows the captured hardware).
    type Bus: embedded_hal::i2c::I2c;
    /// Hand out a fresh master handle (drivers issue transactions on it).
    fn bus(&self) -> Self::Bus;
    /// Stuck-bus recovery; `true` only when BOTH SDA and SCL ended idle-high.
    fn recover(&self) -> bool;
    /// Current `(sda, scl)` line states (`true` = high).
    fn bus_levels(&self) -> (bool, bool);
    /// ACQUIRE the bus: route the pins and bring the master up (enter Sensing).
    fn acquire(&self);
    /// RELEASE the bus: reset the master and tri-state the pins (enter Passive / leave Sensing).
    fn release(&self);
    /// The Leaf-master eUSCI config/flag registers `(ucb0ctlw0, ucb0ifg)` — the `--debug` Leaf diagnostic
    /// (`post::debug_leaf`) decodes MST/SWRST/NACK from them. Chip-specific register read, so it lives on
    /// the impl (DEEPSLEEP-EXTRACTION.md Phase 6c).
    fn leaf_debug_regs(&self) -> (u16, u16);
}
