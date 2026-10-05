//! I²C-slave register map — the Stembus Expander's **contract with the SoM master**.
//!
//! The register ADDRESS SPACE + codes now live in the shared **`stembus::regmap`** crate (ONE
//! definition, so this server and every master — stem-probe, M7 — agree by construction). This module
//! **re-exports** them and adds the **server-internal decode logic** ([`RegSel`], [`decode`],
//! [`next_ai`], [`ShadowState`]) that only the firmware needs. Canonical API: ../I2C-API.md; the
//! PCA9698 register summary: ../datasheets/reg-defs-{1,2}.png.

#![allow(dead_code)] // Contract definitions; some are consumed only by the slave ISR / masters.

// The wire contract (addresses, codes, role addrs, model ids, CRC window) — single source of truth.
pub use stembus::regmap::*;

// ── Server-only (not part of the shared wire contract) ──
pub const EXT_BASE: u8 = 0x30; // 0x30..=0x3F extended / custom register window (decode → Extended)
pub const EXT_END: u8 = 0x3F;
/// Number of VSOM voltage thresholds (I2C-API.md 0x2Eh; STEM-MSG threshold index 0..=2).
pub const VSOM_THRESHOLDS: u8 = 3;

/// A decoded command-register selection. The bank field (0..=7) is carried for the banked groups.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegSel {
    InputPort(u8),   // IP_BASE  + bank — read PxIN (with polarity inversion applied)
    OutputPort(u8),  // OP_BASE  + bank — R/W PxOUT
    PolarityInv(u8), // PI_BASE  + bank — R/W firmware shadow
    IoConfig(u8),    // IOC_BASE + bank — R/W direction (inverse of PxDIR)
    IntMask(u8),     // MSK_BASE + bank — R/W firmware shadow
    OutConf,
    AllBank,
    Mode,
    InitCode,
    VsomVoltage,
    ChargeVoltage,
    VsomThreshold,
    BaudRate,
    Extended(u8), // 0x30..=0x3F, carries the offset from EXT_BASE
    Reserved(u8), // an unused index (e.g. a bank slot ≥ BANKS) — carries the raw reg byte
}

/// Decode a raw command byte (AI already stripped by the caller via `CMD_REG_MASK`) into a [`RegSel`].
/// With the 8-port map every bank slot 0..=7 is valid; a slot ≥ `BANKS` surfaces as `Reserved`.
pub const fn decode(reg: u8) -> RegSel {
    let bank = reg & 0x07;
    let valid_bank = bank < BANKS;
    match reg {
        0x00..=0x07 if valid_bank => RegSel::InputPort(bank),
        0x08..=0x0F if valid_bank => RegSel::OutputPort(bank),
        0x10..=0x17 if valid_bank => RegSel::PolarityInv(bank),
        0x18..=0x1F if valid_bank => RegSel::IoConfig(bank),
        0x20..=0x27 if valid_bank => RegSel::IntMask(bank),
        OUTCONF => RegSel::OutConf,
        ALLBNK => RegSel::AllBank,
        MODE => RegSel::Mode,
        INIT_CODE => RegSel::InitCode,
        VSOM_VOLTAGE => RegSel::VsomVoltage,
        CHARGE_VOLTAGE => RegSel::ChargeVoltage,
        VSOM_THRESHOLD => RegSel::VsomThreshold,
        BAUD_RATE => RegSel::BaudRate,
        EXT_BASE..=EXT_END => RegSel::Extended(reg - EXT_BASE),
        _ => RegSel::Reserved(reg),
    }
}

/// Next command index under Auto-Increment: step the bank (low 3 bits) within the current group,
/// wrapping at 8 back to the group base — matches the PCA9698 "3 LSBs auto-incremented" behavior.
pub const fn next_ai(reg: u8) -> u8 {
    (reg & !0x07) | ((reg + 1) & 0x07)
}

/// Firmware-shadowed per-bank state the emulation must keep (PI and MSK have no MSP430 HW backing;
/// IP/OP/IOC drive the native port registers directly). One byte per active bank.
#[derive(Clone, Copy, Default)]
pub struct ShadowState {
    pub polarity: [u8; BANKS as usize], // PI: XOR mask applied to IP reads
    pub int_mask: [u8; BANKS as usize], // MSK: 1 = do not raise INT for that input pin
    pub init_code: u8,                  // 0x2B, persisted
    pub vsom_threshold: [u16; VSOM_THRESHOLDS as usize], // 0x2E
}
