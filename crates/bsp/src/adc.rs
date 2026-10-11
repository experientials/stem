//! ADC board-health — supply rail (DVCC, mV) and die temperature (°C) from the 12-bit SAR ADC's
//! **internal** channels (no external wiring). The MCU-self "is the board actually healthy" numbers that
//! presence/ID checks can't give. Re-homed from the proven `diag/src/adc.rs` into `bsp` so the node's
//! `--diag` extended POST (and anything else) shares one copy (DESIGN.md "diag — per-device POST
//! extension"); the diag firmware's values were bench-proven on the FR2476.
//!
//!   - **DVCC** via the ratio trick (datasheet §PMM, Eq. 1): sample the internal 1.5-V reference (A13)
//!     with **DVCC as the ADC reference**, so `DVCC = full-scale × 1.5 V ÷ result` — no external ref.
//!   - **Die temperature** via the on-chip sensor (A12) against the internal 1.5-V reference, two-point
//!     interpolated through the TLV factory calibration (30 °C / 105 °C at 1.5 V). Approximate (±few °C).
//!
//! **⚠ Per-chip (to-confirm beyond FR2476):** the channel/reference encodings + the TLV cal addresses
//! below are verified against the **FR2476** datasheet (Table 9-19 channels, Table 9-30 TLV cal) + the
//! vendored PAC. For FR2355/FR2155 confirm the ADC channel map + the ADC cal-block addresses against
//! their datasheets before trusting the numbers (the register accessors compile per-PAC; the *values*
//! are the risk). Cite the local datasheet at the point of any change (`save-sources-locally`).

use crate::pac::Peripherals;

// TLV factory cal: ADC readings of the temperature sensor at the internal 1.5-V reference (FR2476
// datasheet Table 9-30, ADC calibration block tag 0x11 @ 0x1A14). Read-only info memory.
const CAL_ADC_15T30: *const u16 = 0x1A1A as *const u16; //  30 °C reading @ 1.5 V ref
const CAL_ADC_15T105: *const u16 = 0x1A1C as *const u16; // 105 °C reading @ 1.5 V ref

const FULL_SCALE: u32 = 4095; // 12-bit result (0..4095)
const VREF_MV: u32 = 1500; // internal shared reference = 1.5 V

/// Enable the internal 1.5-V reference + the on-chip temperature sensor and configure the ADC core
/// (one-shot, at boot). Leaves the ADC on so [`dvcc_mv`]/[`die_temp_c`] are cheap; call before either.
pub fn init(p: &Peripherals) {
    // Internal reference 1.5 V (REFVSEL=0), enabled; temp sensor (TSENSOREN) — its own power gate, else
    // the temp channel reads ~0. PMMCTL2 needs no password (unlike PMMCTL0).
    p.pmm
        .pmmctl2()
        .modify(|_, w| w.refvsel().refvsel_0().intrefen().set_bit().tsensoren().set_bit());
    // Bounded wait for the reference generator to become active.
    for _ in 0..4000 {
        if p.pmm.pmmctl2().read().refgenact().bit_is_set() {
            break;
        }
    }
    // 12-bit, sampling-timer (pulse) mode, long S&H (ADCSHT_8 = 256 cycles; the temp sensor is
    // high-impedance), core on. Clock = MODCLK (ADC auto-requests it); single channel/conversion.
    p.adc.adcctl0().write(|w| w.adcsht().adcsht_8().adcon().set_bit());
    p.adc.adcctl1().write(|w| w.adcshp().set_bit());
    p.adc.adcctl2().write(|w| w.adcres().adcres_2());
}

/// One single conversion on the currently-selected channel/reference → the 12-bit result. ADCENC must be
/// low to (re)select, so callers write ADCMCTL0 first; this re-clears ADCENC on exit. Bounded spin.
fn convert(p: &Peripherals) -> u16 {
    p.adc.adcctl0().modify(|_, w| w.adcenc().set_bit().adcsc().set_bit());
    for _ in 0..20000 {
        if p.adc.adcifg().read().adcifg0().bit_is_set() {
            break;
        }
    }
    let v = p.adc.adcmem0().read().bits() & 0x0FFF;
    p.adc.adcctl0().modify(|_, w| w.adcenc().clear_bit());
    v
}

/// Supply rail DVCC in millivolts (ratio method). 0 on a nonsensical zero reading.
pub fn dvcc_mv(p: &Peripherals) -> u16 {
    // VR+ = DVCC (ADCSREF=0), input = internal 1.5-V reference node (A13).
    p.adc.adcmctl0().write(|w| w.adcsref().adcsref_0().adcinch().adcinch_13());
    let r = convert(p) as u32;
    if r == 0 {
        return 0;
    }
    (FULL_SCALE * VREF_MV / r) as u16
}

/// Die temperature in whole °C (on-chip sensor, TLV two-point interpolated). 0 if the cal is blank
/// (degenerate span) — treat as "no reading" rather than trusting it.
pub fn die_temp_c(p: &Peripherals) -> i16 {
    // VR+ = internal 1.5-V reference (ADCSREF=1), input = temperature sensor (A12).
    p.adc.adcmctl0().write(|w| w.adcsref().adcsref_1().adcinch().adcinch_12());
    let raw = convert(p) as i32;
    let c30 = unsafe { core::ptr::read_volatile(CAL_ADC_15T30) } as i32;
    let c105 = unsafe { core::ptr::read_volatile(CAL_ADC_15T105) } as i32;
    if c105 == c30 {
        return 0;
    }
    ((raw - c30) * (105 - 30) / (c105 - c30) + 30) as i16
}
