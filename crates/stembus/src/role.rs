//! Model → role → address, and the inverse labelings — shared so the node (which picks its own address
//! from its silicon model) and every master (which labels a responder by address or id) agree. Replaces
//! the copies in `prod::model` (`stem_addr`, `model_name`) and `stem-probe` (`role_name`).

use crate::regmap::{
    ADDR_DEV_STANDIN, ADDR_EXPANDER, ADDR_POWER, ADDR_SUPERVISOR, MODEL_FR2155, MODEL_FR2355,
    MODEL_FR2433, MODEL_FR2475, MODEL_FR2476,
};

/// The role a node plays on the Stem bus (its address encodes this — I2C-API.md).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Supervisor, // production sleep-supervisor (FR2355/FR2155)
    DevStandin, // dev stand-in, same fw, "not the real thing" (FR2476/FR2475)
    Expander,   // I/O-expander placeholder (FR2433)
    Power,      // PD controller (CYPM1111) — masters while the SoM is off
    Unknown,
}

impl Role {
    /// Stem I²C address for this role. `Unknown` → the supervisor base 0x20 (safe default).
    pub const fn addr(self) -> u8 {
        match self {
            Role::Supervisor => ADDR_SUPERVISOR,
            Role::DevStandin => ADDR_DEV_STANDIN,
            Role::Expander => ADDR_EXPANDER,
            Role::Power => ADDR_POWER,
            Role::Unknown => ADDR_SUPERVISOR,
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Role::Supervisor => "supervisor",
            Role::DevStandin => "dev-standin",
            Role::Expander => "expander",
            Role::Power => "power",
            Role::Unknown => "?",
        }
    }
}

/// Role a detected silicon model plays. Unknown silicon → `Unknown` (the node still answers at the
/// supervisor base via [`Role::addr`], so a cold board is reachable; boot flags the mismatch elsewhere).
pub const fn role_for_model(device_id: u16) -> Role {
    match device_id {
        MODEL_FR2355 | MODEL_FR2155 => Role::Supervisor,
        MODEL_FR2476 | MODEL_FR2475 => Role::DevStandin,
        MODEL_FR2433 => Role::Expander,
        _ => Role::Unknown,
    }
}

/// Role implied by a responder's Stem address (inverse of [`Role::addr`]) — label a scan hit by address.
pub const fn role_for_addr(addr: u8) -> Role {
    match addr {
        ADDR_SUPERVISOR => Role::Supervisor,
        ADDR_DEV_STANDIN => Role::DevStandin,
        ADDR_EXPANDER => Role::Expander,
        ADDR_POWER => Role::Power,
        _ => Role::Unknown,
    }
}

/// Human name for a detected silicon model id (TLV 0x1A04).
pub const fn model_name(device_id: u16) -> &'static str {
    match device_id {
        MODEL_FR2476 => "FR2476",
        MODEL_FR2475 => "FR2475",
        MODEL_FR2355 => "FR2355",
        MODEL_FR2155 => "FR2155",
        MODEL_FR2433 => "FR2433",
        _ => "UNKNOWN",
    }
}
