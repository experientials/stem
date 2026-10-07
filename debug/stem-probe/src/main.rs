#![no_std]
#![no_main]

//! stem-probe v0 — RP2040 bench/dev host (the i.MX8 M7 stand-in), first increment.
//!
//! Proves the load + identity + auto-update loop (docs/RP2040-BENCH-HOST.md §1, the "first challenge")
//! with NO thepia hwd — just picotool + a USB CDC terminal:
//!   • brings up a **USB CDC** console; sets USB `iSerial` = the QSPI **flash unique id** (the stable
//!     board key `picotool --ser` / a future `hwd` uses),
//!   • prints a **build banner** (version + flash-uid) on connect,
//!   • accepts a **`boot`** command → `reset_to_usb_boot()` (re-enumerate as BOOTSEL/PICOBOOT so the
//!     next image loads with no button — the auto-update path),
//!   • blinks the onboard LED (GP25) as a liveness heartbeat.
//! Bus mastering (Stem/Leaf I²C, control lines) lands next — this is the loading/identity spine only.

mod i2c_sniff;
use i2c_sniff::{Decoder, Ev, Stats};

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::yield_now;
use embassy_rp::bind_interrupts;
use embassy_rp::flash::{Blocking, Flash};
use embassy_rp::gpio::{Flex, Input, Level, Output, Pull};
use embassy_rp::i2c::{self, I2c};
use embassy_rp::peripherals::{I2C1, USB};
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_time::{Duration, Instant, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::control::{OutResponse, Request, RequestType};
use embassy_usb::driver::EndpointError;
use embassy_usb::{Builder, Config, Handler};

const FLASH_SIZE: usize = 2 * 1024 * 1024;
const FW_VERSION: &str = env!("CARGO_PKG_VERSION");

// Stem-I²C contract (the FR2433 node's slave surface; mirrors prod/src/regmap.rs — the shared contract).
// CANONICAL pin map (same across Pico/Pico H/Pico4ML/RP2350 product, no re-map):
//   Stem-I²C = I2C1 on GP18(SDA)/GP19(SCL)  — the SoM/M7 uplink; clear of the Pico4ML camera block.
//   Leaf-I²C = I2C0 on GP4(SDA)/GP5(SCL)    — sensors; on a Pico4ML this is the camera SCCB.
// Conformance target = the FR2433 regmap on the Stem bus.
// Stem-bus contract from the SHARED `stembus::regmap` (one definition with prod + the M7; crates/stembus).
use stembus::regmap::{
    DBG_IFACE, DBG_IFACE_MAGIC, DBG_MODEL_H, DBG_MODEL_L, MODEL_FR2355 as FR2355_MODEL,
    MODEL_FR2433 as FR2433_MODEL,
};

// Role-based Stem addresses (I2C-API.md "Stem address is ROLE-BASED") — the node's address tells the
// master WHAT it is. `vstem` scans this window and labels each responder by role.
use stembus::regmap::{ADDR_DEV_STANDIN, ADDR_EXPANDER, ADDR_SUPERVISOR};
// cros_ec v3 I²C framing (COMMANDS.md §0 milestone): the SAME codec the FR2433 EC slave runs, so the
// master side can't drift from the slave — the point of putting it in `stembus` (used here on thumbv6m).
use stembus::cros_ec;

// ── Passive I²C sniffer (`sniff <sda> <scl> [ms]`) ────────────────────────────
// Tap any TWO of these otherwise-free GPIOs and watch a live I²C bus WITHOUT driving it — pure inputs,
// NO pull (a sniff must not load the bus under test; the bus provides its own pull-ups). Decodes
// START/STOP, address+R/W, data bytes, and the ACK bit, so we can see what's actually on the wire
// vs what a master THINKS it saw. Edit TAP_GPIOS to tap other pins. Started for the FR2355 Leaf-bus
// false-ACK investigation: wire FR2355 P1.2(SDA)/P1.3(SCL)+GND to two taps and run `sniff`.
// Every exposed Pico GPIO NOT already used by stem-probe (Leaf GP4/5, sideband GP10-13, Stem I2C1
// GP18/19, LED GP25). `taps` probes all of them → a full connection map, so we find SDA/SCL wherever
// they were wired. Order MUST match the `taps` array in main().
const TAP_GPIOS: [u8; 18] = [0, 1, 2, 3, 6, 7, 8, 9, 14, 15, 16, 17, 20, 21, 22, 26, 27, 28];
const DEF_SNIFF_SDA: u8 = 4; // the Pico's Leaf-I²C SDA (GP4) — the usual tap point
const DEF_SNIFF_SCL: u8 = 5; // the Pico's Leaf-I²C SCL (GP5)
const DEF_SNIFF_MS: u16 = 3500; // ≥ the FR2355 diag's 3 s scan period → one scan is guaranteed in-window
const SNIFF_CAP: usize = 512; // decoded-event ring (START/STOP/byte); ~170 address probes
const SNIFF_ARM_SLICE: u32 = 60_000; // samples per arm-watch slice (between USB yields while awaiting traffic)
const SNIFF_ARM_EDGES: u16 = 24; // edges within one slice to count as a real burst (rejects init glitches)
const SNIFF_CAP_BURST: u32 = 100_000; // gap-free samples per capture burst (~25 ms ≫ one 100 µs probe)

// Debug/identity window + mode control (mirrors prod/src/regmap.rs — the shared contract).
use stembus::regmap::{
    DBG_DEV_COUNT, DBG_FW_VER_MAJOR, DBG_FW_VER_MINOR, DBG_FW_VER_PATCH, DBG_STATUS, MODE_CTRL,
    MODE_CODE_PASSIVE as MODE_PASSIVE, MODE_CODE_SENSING as MODE_SENSING,
};

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => InterruptHandler<USB>;
});

/// Minimal panic handler (no defmt): halt so a fault is visible as a dead board, not a silent reboot.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        cortex_m::asm::wfe();
    }
}

// picotool-compatible RESET interface (Pico-SDK `reset_interface`): a vendor interface (class 0xFF,
// subclass 0x00, protocol 0x01) with no endpoints. `picotool reboot -f` finds it by these ids and
// sends a control request `bRequest = RESET_REQUEST_BOOTSEL` to the interface → we reset to BOOTSEL.
// This is what lets `hwd`/`picotool reboot -f -u` force BOOTSEL hands-free (no `boot` keystroke, no button).
const RESET_INTERFACE_CLASS: u8 = 0xFF;
const RESET_INTERFACE_SUBCLASS: u8 = 0x00;
const RESET_INTERFACE_PROTOCOL: u8 = 0x01;
const RESET_REQUEST_BOOTSEL: u8 = 0x01;

struct ResetHandler;

impl Handler for ResetHandler {
    fn control_out(&mut self, req: Request, _data: &[u8]) -> Option<OutResponse> {
        // picotool's reset: bmRequestType = 0x41 (Vendor | Interface | host-to-device),
        // bRequest = RESET_REQUEST_BOOTSEL. That combination is uniquely ours (CDC uses Class requests),
        // so we don't need the interface index. (The handler must be declared before the builder to
        // outlive `usb`, which is why it can't hold the interface number.)
        // Match picotool's reset on a control-OUT regardless of Vendor-vs-Class/recipient: bRequest=0x01
        // on OUT is uniquely the reset (CDC control-OUTs use 0x20/0x21/0x22; its 0x01 is an IN).
        if req.request == RESET_REQUEST_BOOTSEL
            && matches!(req.request_type, RequestType::Vendor | RequestType::Class)
        {
            embassy_rp::rom_data::reset_to_usb_boot(0, 0); // never returns
        }
        None
    }
}

/// Write 8 id bytes as 16 uppercase hex chars into `out`.
fn hex16(id: &[u8; 8], out: &mut [u8; 16]) {
    const H: &[u8; 16] = b"0123456789ABCDEF";
    for (i, b) in id.iter().enumerate() {
        out[i * 2] = H[(b >> 4) as usize];
        out[i * 2 + 1] = H[(b & 0x0f) as usize];
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // Read the QSPI flash unique id → the board identity (USB iSerial / picotool --ser key).
    let mut uid = [0u8; 8];
    {
        let mut flash = Flash::<_, Blocking, FLASH_SIZE>::new_blocking(p.FLASH);
        // If the read ever fails, fall back to all-zeros rather than panicking during USB bring-up.
        let _ = flash.blocking_unique_id(&mut uid);
    }
    let mut uid_hex = [0u8; 16];
    hex16(&uid, &mut uid_hex);
    let serial: &str = core::str::from_utf8(&uid_hex).unwrap_or("0000000000000000");

    // USB device: app VID:PID 2e8a:000a (RP2040 stdio-serial style), iSerial = flash uid.
    let driver = Driver::new(p.USB, Irqs);
    let mut config = Config::new(0x2e8a, 0x000a);
    config.manufacturer = Some("Thepia");
    config.product = Some("stem-probe");
    config.serial_number = Some(serial);
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    let mut config_descriptor = [0u8; 256];
    let mut bos_descriptor = [0u8; 256];
    let mut control_buf = [0u8; 64];
    let mut state = State::new();
    // Declared before `builder` so it outlives `usb` (which holds its &mut via the registered handler).
    let mut reset_handler = ResetHandler;

    let mut builder = Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut [], // no msos descriptors
        &mut control_buf,
    );

    let mut class = CdcAcmClass::new(&mut builder, &mut state, 64);

    // Add the picotool-compatible reset interface (vendor 0xFF/0x00/0x01, no endpoints) + its control
    // handler, so `picotool reboot -f` can force BOOTSEL. Registered before `build()`.
    {
        let mut func = builder.function(
            RESET_INTERFACE_CLASS,
            RESET_INTERFACE_SUBCLASS,
            RESET_INTERFACE_PROTOCOL,
        );
        let mut iface = func.interface();
        iface.alt_setting(
            RESET_INTERFACE_CLASS,
            RESET_INTERFACE_SUBCLASS,
            RESET_INTERFACE_PROTOCOL,
            None,
        );
    }
    builder.handler(&mut reset_handler);

    let mut usb = builder.build();

    let usb_fut = usb.run();

    // Stem-I²C master — CANONICAL map (authoritative; identical on every board): I2C1 on
    // GP18(SDA)/GP19(SCL), clear of the Pico4ML camera block (GP4-16). The RP2040 MASTERS the Stem bus
    // to read the MSP430 node's slave regmap. `Config::default()` enables the RP internal pull-ups, so a
    // short link needs just 3 wires: RP GP18/GP19 ↔ FR2433 P1.2/P1.3 + GND (RP GND = pin 23). ~100 kHz.
    let mut i2c = I2c::new_blocking(p.I2C1, p.PIN_19, p.PIN_18, i2c::Config::default());

    // Leaf bus (GP4 SDA / GP5 SCL) — the Pico owns these as **bit-banged open-drain GPIO**, NOT a
    // hardware I²C controller (Q-SENSING-ENUM defect #2 / ADR-0006): a permanently-enabled I2C0 master
    // was a second controller on the shared Leaf bus and faulted the MSP's Sensing scan. The Pico must be
    // the SOLE Leaf master only while it plays the Passive-mode owner. So: hold the pins as `Flex`, PARKED
    // Hi-Z with pull-up by default (the bench has no external Leaf pull-ups — the Pico provides them so
    // the MSP master sees a healthy bus), and drive them ONLY inside `leaf_scan` (then re-park). One
    // master at a time — upholds the Leaf invariant without a hardware controller that could contend.
    let mut leaf_sda = Flex::new(p.PIN_4);
    let mut leaf_scl = Flex::new(p.PIN_5);
    leaf_park(&mut leaf_sda, &mut leaf_scl); // off the bus, pull-ups on

    // Sideband power-status lines — the Pico is the SoM+PMIC stand-in. It DRIVES the node's edge inputs
    // (SOUND_INT->P2.0, POR_B->P2.1) and READS its outputs (PMIC_ON_REQ<-P2.2, STEM_INT<-P2.4). Pull::Down
    // so an undriven node output reads low. Assumed polarity: SOUND_INT H=SoM-suspended, POR_B active-low
    // (H=power-good) — confirm when the FR2355 sideband firmware lands.
    let mut sound = Output::new(p.PIN_10, Level::Low); // SOUND_INT -> FR2355 P2.0 (J2.19)
    let mut por = Output::new(p.PIN_11, Level::High); // POR_B     -> FR2355 P2.1 (J4.40)
    let pmic_in = Input::new(p.PIN_12, Pull::Down); // PMIC_ON_REQ <- FR2355 P2.2 (J2.18)
    let stem_in = Input::new(p.PIN_13, Pull::Down); // STEM_INT    <- FR2355 P2.4 (J4.33)

    // Passive sniffer taps — claim a set of otherwise-free GPIOs as inputs with NO pull (a sniff must
    // not load the bus it watches). `sniff <sda> <scl>` picks two by GPIO number; see TAP_GPIOS. Order
    // here MUST match TAP_GPIOS.
    let mut taps = [
        Flex::new(p.PIN_0),
        Flex::new(p.PIN_1),
        Flex::new(p.PIN_2),
        Flex::new(p.PIN_3),
        Flex::new(p.PIN_6),
        Flex::new(p.PIN_7),
        Flex::new(p.PIN_8),
        Flex::new(p.PIN_9),
        Flex::new(p.PIN_14),
        Flex::new(p.PIN_15),
        Flex::new(p.PIN_16),
        Flex::new(p.PIN_17),
        Flex::new(p.PIN_20),
        Flex::new(p.PIN_21),
        Flex::new(p.PIN_22),
        Flex::new(p.PIN_26),
        Flex::new(p.PIN_27),
        Flex::new(p.PIN_28),
    ];
    for t in taps.iter_mut() {
        t.set_as_input();
        t.set_pull(Pull::None);
    }

    // Console: banner on connect, then a command loop
    // (id|scan|r|w|selftest|vstem|vleaf|pass|sens|sound0/1|por0/1|pins|boot).
    let console_fut = async {
        loop {
            class.wait_connection().await;
            let _ = banner(&mut class, serial).await;
            let mut buf = [0u8; 64];
            loop {
                match class.read_packet(&mut buf).await {
                    Ok(n) => {
                        let line = &buf[..n];
                        // Token-based parse (first token = command) for the arg-taking commands; the
                        // wordier fixed commands below still use substring match (tolerant of CR/LF).
                        let (toks, nt) = tokenize(line);
                        let cmd0: &[u8] = if nt > 0 { toks[0] } else { &[] };
                        if eq_ci(cmd0, b"help")
                            || eq_ci(cmd0, b"/help")
                            || eq_ci(cmd0, b"h")
                            || eq_ci(cmd0, b"/h")
                            || eq_ci(cmd0, b"?")
                        {
                            // Full, structured CLI reference — the discoverable contract a consumer
                            // scrapes to author a usage skill. Exact-token match (not substring) so a
                            // bare `h`/`?` is unambiguous and never collides with other commands.
                            let _ = help(&mut class).await;
                        } else if eq_ci(cmd0, b"r") {
                            // r <addr> <reg> — read one Stem register (hex, 0x optional).
                            if nt >= 3 {
                                if let (Some(addr), Some(reg)) = (parse_hex(toks[1]), parse_hex(toks[2])) {
                                    match rd(&mut i2c, addr, reg) {
                                        Some(v) => {
                                            let _ = write_str(&mut class, "\r\n0x").await;
                                            let _ = write_hex8(&mut class, addr).await;
                                            let _ = write_str(&mut class, "[0x").await;
                                            let _ = write_hex8(&mut class, reg).await;
                                            let _ = write_str(&mut class, "] = 0x").await;
                                            let _ = write_hex8(&mut class, v).await;
                                            let _ = write_str(&mut class, "\r\n> ").await;
                                        }
                                        None => { let _ = write_str(&mut class, "\r\n  no ACK\r\n> ").await; }
                                    }
                                } else {
                                    let _ = write_str(&mut class, "\r\nusage: r <addr> <reg>  (hex)\r\n> ").await;
                                }
                            } else {
                                let _ = write_str(&mut class, "\r\nusage: r <addr> <reg>  (hex)\r\n> ").await;
                            }
                        } else if eq_ci(cmd0, b"w") {
                            // w <addr> <reg> <val> — write one Stem register (hex, 0x optional).
                            if nt >= 4 {
                                if let (Some(addr), Some(reg), Some(val)) =
                                    (parse_hex(toks[1]), parse_hex(toks[2]), parse_hex(toks[3]))
                                {
                                    let ok = wr(&mut i2c, addr, reg, val);
                                    let _ = write_str(&mut class,
                                        if ok { "\r\n  [PASS] wrote\r\n> " } else { "\r\n  [FAIL] no ACK\r\n> " }).await;
                                } else {
                                    let _ = write_str(&mut class, "\r\nusage: w <addr> <reg> <val>  (hex)\r\n> ").await;
                                }
                            } else {
                                let _ = write_str(&mut class, "\r\nusage: w <addr> <reg> <val>  (hex)\r\n> ").await;
                            }
                        } else if eq_ci(cmd0, b"selftest") {
                            // Run T1–T3 across the role window + Leaf; end with one grep-able RESULT line.
                            let mut t = Tally::new();
                            verify_stem(&mut class, &mut i2c, &mut sound, &mut por, &pmic_in, &stem_in, &mut t).await;
                            verify_leaf(&mut class, &mut i2c, &mut leaf_sda, &mut leaf_scl, &mut t).await;
                            let _ = write_str(&mut class, "\r\nRESULT pass=").await;
                            let _ = write_dec(&mut class, t.pass).await;
                            let _ = write_str(&mut class, " fail=").await;
                            let _ = write_dec(&mut class, t.fail).await;
                            let _ = write_str(&mut class, "\r\n> ").await;
                        } else if eq_ci(cmd0, b"lscan") {
                            // Bit-bang scan the LEAF bus (GP4/GP5) and LIST responders — the Pico masters
                            // Leaf directly (e.g. to reach the I/O extender). Re-parks on exit.
                            let _ = write_str(&mut class, "\r\nleaf-scan:").await;
                            let mut found = 0u8;
                            let mut a = 0x08u8;
                            while a <= 0x77 {
                                if leaf_probe(&mut leaf_sda, &mut leaf_scl, a) {
                                    let _ = write_str(&mut class, " 0x").await;
                                    let _ = write_hex8(&mut class, a).await;
                                    found += 1;
                                }
                                a += 1;
                            }
                            leaf_park(&mut leaf_sda, &mut leaf_scl);
                            if found == 0 {
                                let _ = write_str(&mut class, " (none — check Leaf wiring GP4/GP5 + pull-ups)").await;
                            }
                            let _ = write_str(&mut class, "\r\n> ").await;
                        } else if eq_ci(cmd0, b"lr") {
                            // lr <addr> <reg> — bit-bang read one register from a LEAF device (hex).
                            if nt >= 3 {
                                if let (Some(addr), Some(reg)) = (parse_hex(toks[1]), parse_hex(toks[2])) {
                                    let v = leaf_read_reg(&mut leaf_sda, &mut leaf_scl, addr, reg);
                                    leaf_park(&mut leaf_sda, &mut leaf_scl);
                                    match v {
                                        Some(val) => {
                                            let _ = write_str(&mut class, "\r\nleaf 0x").await;
                                            let _ = write_hex8(&mut class, addr).await;
                                            let _ = write_str(&mut class, "[0x").await;
                                            let _ = write_hex8(&mut class, reg).await;
                                            let _ = write_str(&mut class, "] = 0x").await;
                                            let _ = write_hex8(&mut class, val).await;
                                            let _ = write_str(&mut class, "\r\n> ").await;
                                        }
                                        None => { let _ = write_str(&mut class, "\r\n  no ACK (device absent / wrong addr)\r\n> ").await; }
                                    }
                                } else {
                                    let _ = write_str(&mut class, "\r\nusage: lr <addr> <reg>  (hex)\r\n> ").await;
                                }
                            } else {
                                let _ = write_str(&mut class, "\r\nusage: lr <addr> <reg>  (hex)\r\n> ").await;
                            }
                        } else if eq_ci(cmd0, b"sniff") {
                            // sniff <sda_gpio> <scl_gpio> [ms] — passively decode a live I²C bus.
                            // GP4/GP5 = the Pico's own Leaf pins (the bit-bang-master pins); sniffable via the
                            // owned leaf Flex (kept input + pull-up — passive, but preserves Leaf pull-ups).
                            // Any of the TAP_GPIOS are also sniffable (no pull → pure passive tap).
                            let sda_gp = if nt >= 2 { parse_dec_u16(toks[1]).map(|v| v as u8) } else { Some(DEF_SNIFF_SDA) };
                            let scl_gp = if nt >= 3 { parse_dec_u16(toks[2]).map(|v| v as u8) } else { Some(DEF_SNIFF_SCL) };
                            let ms = if nt >= 4 { parse_dec_u16(toks[3]) } else { Some(DEF_SNIFF_MS) };
                            match (sda_gp, scl_gp, ms) {
                                (Some(4), Some(5), Some(ms)) => {
                                    leaf_sda.set_as_input();
                                    leaf_sda.set_pull(Pull::Up);
                                    leaf_scl.set_as_input();
                                    leaf_scl.set_pull(Pull::Up);
                                    run_sniff(&mut class, &leaf_sda, &leaf_scl, 4, 5, ms).await;
                                    leaf_park(&mut leaf_sda, &mut leaf_scl);
                                }
                                (Some(5), Some(4), Some(ms)) => {
                                    leaf_sda.set_as_input();
                                    leaf_sda.set_pull(Pull::Up);
                                    leaf_scl.set_as_input();
                                    leaf_scl.set_pull(Pull::Up);
                                    run_sniff(&mut class, &leaf_scl, &leaf_sda, 5, 4, ms).await; // GP5=SDA, GP4=SCL
                                    leaf_park(&mut leaf_sda, &mut leaf_scl);
                                }
                                (Some(s), Some(c), Some(ms))
                                    if tap_index(s).is_some() && tap_index(c).is_some() && s != c =>
                                {
                                    let si = tap_index(s).unwrap();
                                    let ci = tap_index(c).unwrap();
                                    taps[si].set_as_input();
                                    taps[si].set_pull(Pull::None);
                                    taps[ci].set_as_input();
                                    taps[ci].set_pull(Pull::None);
                                    run_sniff(&mut class, &taps[si], &taps[ci], s, c, ms).await;
                                }
                                _ => {
                                    let _ = write_str(&mut class, "\r\nusage: sniff <sda_gpio> <scl_gpio> [ms]\r\n  pins: 4 5 (Pico Leaf, default) | 6 7 8 9 14 15 16 17 20 21 22 26 27 28 (taps). sda/scl must differ.\r\n> ").await;
                                }
                            }
                        } else if eq_ci(cmd0, b"taps") {
                            // Connection probe: pull each tap DOWN briefly; a line held H is externally
                            // driven/pulled high (idle I²C bus) → that's where SDA/SCL landed AND proves a
                            // common GND. All-L = nothing connected (or no shared ground). Restores no-pull.
                            let _ = write_str(&mut class, "\r\ntaps (pull-down; H = externally high = connected to an idle bus):").await;
                            for t in taps.iter_mut() {
                                t.set_as_input();
                                t.set_pull(Pull::Down);
                            }
                            Timer::after(Duration::from_millis(2)).await;
                            for (i, t) in taps.iter().enumerate() {
                                let _ = write_str(&mut class, "  GP").await;
                                let _ = write_dec(&mut class, TAP_GPIOS[i] as u16).await;
                                let _ = write_str(&mut class, "=").await;
                                let _ = write_str(&mut class, if t.is_high() { "H" } else { "L" }).await;
                            }
                            for t in taps.iter_mut() {
                                t.set_pull(Pull::None);
                            }
                            let _ = write_str(&mut class, "\r\n> ").await;
                        } else if contains_ci(line, b"boot") {
                            let _ = write_str(&mut class, "\r\n[entering BOOTSEL]\r\n").await;
                            Timer::after(Duration::from_millis(50)).await; // let the packet flush
                            embassy_rp::rom_data::reset_to_usb_boot(0, 0); // never returns
                        } else if contains_ci(line, b"vstem") {
                            verify_stem(&mut class, &mut i2c, &mut sound, &mut por, &pmic_in, &stem_in, &mut Tally::new()).await;
                        } else if contains_ci(line, b"vleaf") {
                            verify_leaf(&mut class, &mut i2c, &mut leaf_sda, &mut leaf_scl, &mut Tally::new()).await;
                        } else if contains_ci(line, b"sens") {
                            let ok = wr(&mut i2c, ADDR_SUPERVISOR, MODE_CTRL, MODE_SENSING);
                            report(&mut class, "mode -> Sensing", ok, &mut Tally::new()).await;
                            let _ = write_str(&mut class, "> ").await;
                        } else if contains_ci(line, b"pass") {
                            let ok = wr(&mut i2c, ADDR_SUPERVISOR, MODE_CTRL, MODE_PASSIVE);
                            report(&mut class, "mode -> Passive", ok, &mut Tally::new()).await;
                            let _ = write_str(&mut class, "> ").await;
                        } else if contains_ci(line, b"sound1") {
                            sound.set_high();
                            let _ = write_str(&mut class, "\r\nSOUND_INT=H (SoM suspended)\r\n> ").await;
                        } else if contains_ci(line, b"sound0") {
                            sound.set_low();
                            let _ = write_str(&mut class, "\r\nSOUND_INT=L (SoM active)\r\n> ").await;
                        } else if contains_ci(line, b"por1") {
                            por.set_high();
                            let _ = write_str(&mut class, "\r\nPOR_B=H (power good)\r\n> ").await;
                        } else if contains_ci(line, b"por0") {
                            por.set_low();
                            let _ = write_str(&mut class, "\r\nPOR_B=L (reset)\r\n> ").await;
                        } else if contains_ci(line, b"pins") {
                            let _ = write_str(&mut class, "\r\nPMIC_ON_REQ=").await;
                            let _ = write_str(&mut class, if pmic_in.is_high() { "H" } else { "L" }).await;
                            let _ = write_str(&mut class, " STEM_INT=").await;
                            let _ = write_str(&mut class, if stem_in.is_high() { "H" } else { "L" }).await;
                            let _ = write_str(&mut class, "\r\n> ").await;
                        } else if contains_ci(line, b"scan") {
                            // Probe the Stem bus (MASTER) for responding addresses.
                            let _ = write_str(&mut class, "\r\nscan:").await;
                            let mut found = 0u8;
                            let mut a = 0x08u8;
                            while a <= 0x77 {
                                let mut b = [0u8; 1];
                                if i2c.blocking_read(a, &mut b).is_ok() {
                                    let _ = write_str(&mut class, " 0x").await;
                                    let _ = write_hex8(&mut class, a).await;
                                    found += 1;
                                }
                                a += 1;
                            }
                            if found == 0 {
                                let _ = write_str(&mut class, " (none — check wiring)").await;
                            }
                            let _ = write_str(&mut class, "\r\n> ").await;
                        } else if contains_ci(line, b"hello") {
                            // cros_ec v3 HELLO over I²C (COMMANDS.md §0 milestone): write a request frame,
                            // repeated-START read the response, verify out == in + 0x01020304. This is the
                            // end-to-end proof of the FR2433 EC-slave framing against a real master.
                            const IN_DATA: u32 = 0xA5A5_1234;
                            let mut req = [0u8; 16];
                            let mut resp = [0u8; 16];
                            let _ = write_str(&mut class, "\r\ncros_ec HELLO @0x").await;
                            let _ = write_hex8(&mut class, ADDR_EXPANDER).await;
                            match cros_ec::encode_request(cros_ec::EC_CMD_HELLO, 0, &IN_DATA.to_le_bytes(), &mut req) {
                                Ok(n) => {
                                    // HELLO reply frame = 2 (result,len) + 8 (header) + 4 (payload) = 14 bytes.
                                    let rr = i2c.blocking_write_read(ADDR_EXPANDER, &req[..n], &mut resp[..14]);
                                    if rr.is_ok() {
                                        match cros_ec::parse_response(&resp[..14]) {
                                            Ok(pr) if pr.result == cros_ec::EC_RES_SUCCESS && pr.data.len() >= 4 => {
                                                let out = u32::from_le_bytes([pr.data[0], pr.data[1], pr.data[2], pr.data[3]]);
                                                let want = IN_DATA.wrapping_add(0x0102_0304);
                                                let _ = write_str(&mut class, " in=").await;
                                                let _ = write_hex32(&mut class, IN_DATA).await;
                                                let _ = write_str(&mut class, " out=").await;
                                                let _ = write_hex32(&mut class, out).await;
                                                let _ = write_str(&mut class, if out == want { "  PASS\r\n" } else { "  FAIL (mismatch)\r\n" }).await;
                                            }
                                            Ok(pr) => {
                                                let _ = write_str(&mut class, "  FAIL (result=0x").await;
                                                let _ = write_hex8(&mut class, pr.result as u8).await;
                                                let _ = write_str(&mut class, ")\r\n").await;
                                            }
                                            Err(_) => {
                                                let _ = write_str(&mut class, "  FAIL (bad response framing)\r\n").await;
                                            }
                                        }
                                    } else {
                                        let _ = write_str(&mut class, "  FAIL (I2C no-ACK/timeout)\r\n").await;
                                    }
                                }
                                Err(_) => {
                                    let _ = write_str(&mut class, "  FAIL (encode)\r\n").await;
                                }
                            }
                            let _ = write_str(&mut class, "> ").await;
                        } else if contains_ci(line, b"id") {
                            let _ = write_str(&mut class, "\r\nid ").await;
                            let _ = class.write_packet(serial.as_bytes()).await;
                            let _ = write_str(&mut class, "\r\n> ").await;
                        } else {
                            let _ = write_str(&mut class, "\r\ncmds: id | scan | hello | r <a> <reg> | w <a> <reg> <v> | lscan | lr <a> <reg> | sniff <sda> <scl> [ms] | taps | selftest | vstem | vleaf | pass | sens | sound0/1 | por0/1 | pins | boot | help   (type 'help' for full semantics)\r\n> ").await;
                        }
                    }
                    Err(_) => break, // disconnected → wait for the next connection
                }
            }
        }
    };

    // Liveness heartbeat on the onboard LED (GP25 on a Pico-class board).
    let blink_fut = async {
        let mut led = Output::new(p.PIN_25, Level::Low);
        loop {
            led.toggle();
            Timer::after(Duration::from_millis(500)).await;
        }
    };

    join(usb_fut, join(console_fut, blink_fut)).await;
}

/// Boot banner: version + flash-uid identity (the line a `--verify-stamp` would watch for).
async fn banner<'d, T: embassy_rp::usb::Instance + 'd>(
    class: &mut CdcAcmClass<'d, Driver<'d, T>>,
    serial: &str,
) -> Result<(), EndpointError> {
    write_str(class, "\r\n== stem-probe v").await?;
    write_str(class, FW_VERSION).await?;
    write_str(class, " rp2040 uid=").await?;
    class.write_packet(serial.as_bytes()).await?;
    write_str(class, " — bench host up ==\r\n> ").await?;
    Ok(())
}

/// Full CLI reference (`help`/`h`/`?`/`/h`). This is the DISCOVERABLE CONTRACT: a consumer scrapes
/// this output to author a usage skill, so it must state the full semantics of every command — bus,
/// args + number base, side effects, and output format — not just a name list. Keep it in sync with
/// the dispatch chain in `main`. Structured as labelled sections; each command line is
/// `  <invocation>   <semantics>`. Written in chunks (`write_str` splits to 64-byte CDC packets).
async fn help<'d, T: embassy_rp::usb::Instance + 'd>(
    class: &mut CdcAcmClass<'d, Driver<'d, T>>,
) -> Result<(), EndpointError> {
    write_str(class, "\r\n== stem-probe v").await?;
    write_str(class, FW_VERSION).await?;
    write_str(class, " CLI reference ==\r\n").await?;
    write_str(
        class,
        "role: RP2040 bench host = SoM/M7 stand-in. Stem-bus MASTER; Leaf bit-bang master (on demand); sideband driver.\r\n",
    )
    .await?;
    write_str(
        class,
        "bus pins (CANONICAL, identical on every RP2040/RP2350 board):\r\n\
         \x20 Stem I2C = I2C1 GP18(SDA)/GP19(SCL)      -> MCU node slave surface (role-addressed)\r\n\
         \x20 Leaf I2C = bit-bang GP4(SDA)/GP5(SCL)    -> sensors; parked Hi-Z+pull-up when idle\r\n\
         \x20 sideband = GP10 SOUND_INT(out) GP11 POR_B(out) GP12 PMIC_ON_REQ(in) GP13 STEM_INT(in)\r\n",
    )
    .await?;
    write_str(
        class,
        "roles (Stem addr): 0x20 supervisor | 0x21 dev-standin | 0x22 I/O-expander | 0x23 power\r\n\
         numbers: <addr>/<reg>/<val> = HEX (0x optional); <gpio>/<ms> = DECIMAL\r\n",
    )
    .await?;

    write_str(
        class,
        "\r\nSTEM bus (RP2040 is master):\r\n\
         \x20 scan                   probe 0x08..0x77, list addresses that ACK\r\n\
         \x20 r <addr> <reg>         read 1 register (write reg ptr, repeated-start read); '0x.. = 0x..' or 'no ACK'\r\n\
         \x20 w <addr> <reg> <val>   write 1 register; '[PASS] wrote' / '[FAIL] no ACK'\r\n\
         \x20 vstem                  full verify: role discovery, DBG_IFACE magic, model id, fw version, status, MODE_CTRL Passive<->Sensing, sideband readout\r\n\
         \x20 pass | sens            set node @0x20 MODE_CTRL Passive(0x00) / Sensing(0x01)\r\n",
    )
    .await?;

    write_str(
        class,
        "\r\nLEAF bus (RP2040 bit-bang master; one-master, re-parks on exit):\r\n\
         \x20 lscan                  scan 0x08..0x77 on GP4/GP5, list devices that ACK\r\n\
         \x20 lr <addr> <reg>        read 1 register from a Leaf device\r\n\
         \x20 vleaf                  prove the handoff: node enumerates Leaf in Sensing, Pico masters it in Passive\r\n",
    )
    .await?;

    write_str(
        class,
        "\r\nPASSIVE sniff / wiring (never drives the bus):\r\n\
         \x20 sniff <sda> <scl> [ms] decode a live I2C bus. pins: 4 5 (Leaf, default) OR any two distinct taps\r\n\
         \x20                        0 1 2 3 6 7 8 9 14 15 16 17 20 21 22 26 27 28. ms default 3500.\r\n\
         \x20                        frame out: 'S <aa>W+A <dd>+a ... P' (A/N=addr ACK/NAK, a/n=data ACK/NAK)\r\n\
         \x20 taps                   pull-down probe of all free GPIOs; H = wired to an idle (pulled-up) bus + shared GND\r\n",
    )
    .await?;

    write_str(
        class,
        "\r\nSIDEBAND (SoM/PMIC stand-in; raw electrical, node reaction needs node sideband fw):\r\n\
         \x20 sound0 | sound1        drive SOUND_INT L(SoM active) / H(SoM suspended)\r\n\
         \x20 por0 | por1            drive POR_B L(reset) / H(power good)\r\n\
         \x20 pins                   read PMIC_ON_REQ and STEM_INT levels\r\n",
    )
    .await?;

    write_str(
        class,
        "\r\nSYSTEM:\r\n\
         \x20 id                     print board identity (QSPI flash uid = USB iSerial)\r\n\
         \x20 selftest               run vstem+vleaf; ends with one grep-able line 'RESULT pass=N fail=M'\r\n\
         \x20 boot                   reset to USB BOOTSEL (reflash path)\r\n\
         \x20 help | h | ? | /h      this reference\r\n> ",
    )
    .await?;
    Ok(())
}

/// Write a `&str` over CDC in <=64-byte packets.
async fn write_str<'d, T: embassy_rp::usb::Instance + 'd>(
    class: &mut CdcAcmClass<'d, Driver<'d, T>>,
    s: &str,
) -> Result<(), EndpointError> {
    for chunk in s.as_bytes().chunks(64) {
        class.write_packet(chunk).await?;
    }
    Ok(())
}

/// Write a byte as two uppercase hex chars over CDC.
async fn write_hex8<'d, T: embassy_rp::usb::Instance + 'd>(
    class: &mut CdcAcmClass<'d, Driver<'d, T>>,
    v: u8,
) -> Result<(), EndpointError> {
    const H: &[u8; 16] = b"0123456789ABCDEF";
    class
        .write_packet(&[H[(v >> 4) as usize], H[(v & 0x0f) as usize]])
        .await
}

/// Write a u32 as eight uppercase hex chars (MSB→LSB) over CDC.
async fn write_hex32<'d, T: embassy_rp::usb::Instance + 'd>(
    class: &mut CdcAcmClass<'d, Driver<'d, T>>,
    v: u32,
) -> Result<(), EndpointError> {
    write_hex8(class, (v >> 24) as u8).await?;
    write_hex8(class, (v >> 16) as u8).await?;
    write_hex8(class, (v >> 8) as u8).await?;
    write_hex8(class, v as u8).await
}

/// Case-insensitive substring test (needle is lowercase ASCII).
fn contains_ci(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    'outer: for i in 0..=hay.len() - needle.len() {
        for (j, &nb) in needle.iter().enumerate() {
            if hay[i + j] | 0x20 != nb {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

// ───────────────────────── Stem / Leaf verification ─────────────────────────
// The RP2040 is the Stem-bus MASTER (SoM/M7 stand-in). `vstem` exercises the node's Stem slave surface
// + mode control; `vleaf` proves the node masters the Leaf bus in Sensing and releases it in Passive.
// I²C is blocking (sync); only the CDC writes await. Build-unverified here (no thumbv6m target in CI).

/// Blocking Stem-bus register read (write reg ptr, repeated-start read 1 byte). None = no ACK / error.
fn rd(i2c: &mut I2c<'_, I2C1, i2c::Blocking>, addr: u8, reg: u8) -> Option<u8> {
    let mut b = [0u8; 1];
    i2c.blocking_write_read(addr, &[reg], &mut b).ok().map(|_| b[0])
}

/// Blocking Stem-bus register write (reg ptr + value). false = no ACK / error.
fn wr(i2c: &mut I2c<'_, I2C1, i2c::Blocking>, addr: u8, reg: u8, val: u8) -> bool {
    i2c.blocking_write(addr, &[reg, val]).is_ok()
}

/// Role name for a Stem address (I2C-API.md role-based addressing).
fn role_name(addr: u8) -> &'static str {
    // Shared addr→role mapping (stembus::role); stem-probe keeps its richer display strings.
    use stembus::role::Role;
    match stembus::role::role_for_addr(addr) {
        Role::Supervisor => "supervisor (FR2355/FR2155)",
        Role::DevStandin => "dev stand-in (FR2476)",
        Role::Expander => "I/O-expander (FR2433)",
        Role::Power => "power (CYPM1111)",
        Role::Unknown => "?",
    }
}

/// Running pass/fail count so `selftest` can emit one grep-able `RESULT pass=N fail=M` line.
struct Tally {
    pass: u16,
    fail: u16,
}
impl Tally {
    fn new() -> Self {
        Self { pass: 0, fail: 0 }
    }
}

/// Emit a `[PASS]`/`[FAIL]` line and tally it.
async fn report<'d>(class: &mut CdcAcmClass<'d, Driver<'d, USB>>, label: &str, ok: bool, t: &mut Tally) {
    if ok {
        t.pass += 1;
    } else {
        t.fail += 1;
    }
    let _ = write_str(class, if ok { "\r\n  [PASS] " } else { "\r\n  [FAIL] " }).await;
    let _ = write_str(class, label).await;
}

/// True if `tok` equals keyword `kw` (kw lowercase ASCII), case-insensitively.
fn eq_ci(tok: &[u8], kw: &[u8]) -> bool {
    tok.len() == kw.len() && tok.iter().zip(kw).all(|(a, b)| a | 0x20 == *b)
}

fn is_ws(b: u8) -> bool {
    b == b' ' || b == b'\t' || b == b'\r' || b == b'\n'
}

/// Split a line into up to 4 whitespace-delimited tokens. Returns (tokens, count).
fn tokenize(line: &[u8]) -> ([&[u8]; 4], usize) {
    let mut toks: [&[u8]; 4] = [&[]; 4];
    let mut nt = 0;
    let mut i = 0;
    while i < line.len() && nt < 4 {
        while i < line.len() && is_ws(line[i]) {
            i += 1;
        }
        let start = i;
        while i < line.len() && !is_ws(line[i]) {
            i += 1;
        }
        if i > start {
            toks[nt] = &line[start..i];
            nt += 1;
        }
    }
    (toks, nt)
}

/// Parse a hex byte token, optional `0x`/`0X` prefix. None on empty/too-long/non-hex.
fn parse_hex(tok: &[u8]) -> Option<u8> {
    let t = if tok.len() >= 2 && tok[0] == b'0' && tok[1] | 0x20 == b'x' {
        &tok[2..]
    } else {
        tok
    };
    if t.is_empty() || t.len() > 2 {
        return None;
    }
    let mut v = 0u8;
    for &c in t {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return None,
        };
        v = (v << 4) | d;
    }
    Some(v)
}

/// Parse a decimal `u16` token (GPIO numbers are decimal). None on empty/overflow/non-digit.
fn parse_dec_u16(tok: &[u8]) -> Option<u16> {
    if tok.is_empty() || tok.len() > 5 {
        return None;
    }
    let mut v: u16 = 0;
    for &c in tok {
        if !c.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((c - b'0') as u16)?;
    }
    Some(v)
}

/// Map a GPIO number to its index in the claimed `taps` array (TAP_GPIOS), or None if not a tap pin.
fn tap_index(gp: u8) -> Option<usize> {
    TAP_GPIOS.iter().position(|&g| g == gp)
}

// ───────────────── passive I²C sniffer: RP2040/embassy/CDC adapter ─────────────────
// The reusable, HAL-agnostic sniffer core lives in `i2c_sniff` (closures for the line reads, no I/O).
// `run_sniff` is the thin board glue: it adapts embassy `Flex` pins → read closures, keeps USB alive
// while WAITING for traffic (yielding between short arm-watch slices), then captures the burst GAP-FREE
// (one synchronous pass — yields mid-scan drop bits), and `print_sniff` streams the result over CDC.

/// Passively sniff two input pins (already configured as inputs by the caller): arm on the first SCL
/// edge, capture one gap-free burst, then print. `ms` bounds how long to wait for traffic to appear.
async fn run_sniff<'d>(
    class: &mut CdcAcmClass<'d, Driver<'d, USB>>,
    sda: &Flex<'_>,
    scl: &Flex<'_>,
    sda_gp: u8,
    scl_gp: u8,
    ms: u16,
) {
    let _ = write_str(class, "\r\nsniff SDA=GP").await;
    let _ = write_dec(class, sda_gp as u16).await;
    let _ = write_str(class, " SCL=GP").await;
    let _ = write_dec(class, scl_gp as u16).await;
    let _ = write_str(class, " passive, waiting ").await;
    let _ = write_dec(class, ms).await;
    let _ = write_str(class, "ms for clock ...").await;

    let read_sda = || sda.is_high();
    let read_scl = || scl.is_high();
    let mut log = [Ev::Stop; SNIFF_CAP];
    let mut n = 0usize;
    let mut stats = Stats::new();

    // Arm: watch for the first SCL edge, yielding between slices so the (possibly long) wait for traffic
    // doesn't starve USB. Once armed, we capture synchronously — no more yields until the burst is done.
    let deadline = Instant::now() + Duration::from_millis(ms as u64);
    let mut prev = read_scl();
    let mut armed = false;
    while Instant::now() < deadline {
        if i2c_sniff::wait_for_burst(&read_scl, &mut prev, SNIFF_ARM_SLICE, SNIFF_ARM_EDGES) {
            armed = true;
            break;
        }
        yield_now().await;
    }
    if armed {
        // Capture across the whole (UART-throttled, ~hundreds-of-ms) scan: gap-free bursts with a yield
        // in the idle gaps between probes. Each probe is ~100 µs ≪ a burst, so straddles are rare and the
        // decoder resyncs on the next START. idle_limit = MAX → bursts run their full length, no early exit.
        let mut dec = Decoder::new(read_scl(), read_sda());
        // Capture until the overall deadline (the scan is slow — UART-throttled, seconds long) or the ring fills.
        while Instant::now() < deadline && n < SNIFF_CAP {
            i2c_sniff::capture_burst(&mut dec, &read_sda, &read_scl, &mut log, &mut n, &mut stats, SNIFF_CAP_BURST, u32::MAX);
            yield_now().await;
        }
    }
    print_sniff(class, &log[..n], &stats).await;
}

/// Print the sniff summary (one grep-able line) then the decoded frames. Format per frame:
/// `S <addr>W+A <data>+a ... P`  — uppercase A/N = address-byte ACK/NAK, lowercase a/n = data-byte.
async fn print_sniff<'d>(class: &mut CdcAcmClass<'d, Driver<'d, USB>>, log: &[Ev], s: &Stats) {
    let _ = write_str(class, "\r\nsniff: starts=").await;
    let _ = write_dec(class, s.starts).await;
    let _ = write_str(class, " stops=").await;
    let _ = write_dec(class, s.stops).await;
    let _ = write_str(class, " sclEdges=").await;
    let _ = write_dec(class, s.scl_edges).await;
    let _ = write_str(class, " bytes=").await;
    let _ = write_dec(class, s.bytes).await;
    let _ = write_str(class, " ack=").await;
    let _ = write_dec(class, s.ack).await;
    let _ = write_str(class, " nak=").await;
    let _ = write_dec(class, s.nak).await;
    if s.scl_edges == 0 {
        let _ = write_str(class, "\r\n  NO CLOCK: SCL never toggled on the SCL tap. The master isn't driving, or SDA/SCL aren't on these GPIOs / not wired (need SDA, SCL, and a common GND).\r\n> ").await;
        return;
    }
    let mut expect_addr = false;
    for ev in log {
        match *ev {
            Ev::Start => {
                let _ = write_str(class, "\r\nS ").await;
                expect_addr = true;
            }
            Ev::Stop => {
                let _ = write_str(class, " P").await;
                expect_addr = false;
            }
            Ev::Byte(v, ack) => {
                if expect_addr {
                    let _ = write_hex8(class, v >> 1).await;
                    let _ = write_str(class, if v & 1 == 1 { "R" } else { "W" }).await;
                    let _ = write_str(class, if ack { "+A" } else { "+N" }).await;
                    expect_addr = false;
                } else {
                    let _ = write_str(class, " ").await;
                    let _ = write_hex8(class, v).await;
                    let _ = write_str(class, if ack { "+a" } else { "+n" }).await;
                }
            }
        }
    }
    let _ = write_str(class, "\r\n> ").await;
}

/// Write a `u16` as decimal over CDC.
async fn write_dec<'d, T: embassy_rp::usb::Instance + 'd>(
    class: &mut CdcAcmClass<'d, Driver<'d, T>>,
    v: u16,
) -> Result<(), EndpointError> {
    if v == 0 {
        return class.write_packet(b"0").await;
    }
    let mut buf = [0u8; 5];
    let mut i = 5;
    let mut n = v;
    while n > 0 && i > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    class.write_packet(&buf[i..]).await
}

/// Verify the full Stem-bus surface: role discovery, identity, version, status, mode control, sideband.
async fn verify_stem<'d>(
    class: &mut CdcAcmClass<'d, Driver<'d, USB>>,
    i2c: &mut I2c<'_, I2C1, i2c::Blocking>,
    sound: &mut Output<'_>,
    por: &mut Output<'_>,
    pmic_in: &Input<'_>,
    stem_in: &Input<'_>,
    t: &mut Tally,
) {
    let _ = write_str(class, "\r\n== VERIFY STEM ==").await;

    // 1) Role discovery — scan the role window; for EACH responder show role + identity magic + silicon
    // model id (so the address-is-role claim is proven per node, not just by which address ACKed).
    let mut node = 0u8;
    for &a in &[ADDR_SUPERVISOR, ADDR_DEV_STANDIN, ADDR_EXPANDER] {
        if let Some(iface) = rd(i2c, a, DBG_IFACE) {
            let _ = write_str(class, "\r\n  role @0x").await;
            let _ = write_hex8(class, a).await;
            let _ = write_str(class, " = ").await;
            let _ = write_str(class, role_name(a)).await;
            let _ = write_str(class, "  iface=0x").await;
            let _ = write_hex8(class, iface).await;
            if let (Some(l), Some(h)) = (rd(i2c, a, DBG_MODEL_L), rd(i2c, a, DBG_MODEL_H)) {
                let _ = write_str(class, " model=0x").await;
                let _ = write_hex8(class, h).await;
                let _ = write_hex8(class, l).await;
            }
            if node == 0 {
                node = a;
            }
        }
    }
    report(class, "a Stem node answered (0x20/0x21/0x22)", node != 0, t).await;
    if node == 0 {
        let _ = write_str(class, "\r\n  (no node — check Stem wiring: GP18/19 <-> P4.6/P4.7 + GND)\r\n> ").await;
        return;
    }

    // 2) Identity magic
    report(class, "DBG_IFACE == 0xD0", rd(i2c, node, DBG_IFACE) == Some(DBG_IFACE_MAGIC), t).await;

    // 3) Detected model (captured — drives the role-aware mode check below)
    let model = match (rd(i2c, node, DBG_MODEL_L), rd(i2c, node, DBG_MODEL_H)) {
        (Some(l), Some(h)) => {
            let m = ((h as u16) << 8) | l as u16;
            let _ = write_str(class, "\r\n  model=0x").await;
            let _ = write_hex8(class, h).await;
            let _ = write_hex8(class, l).await;
            let _ = write_str(class,
                if m == FR2355_MODEL { " (FR2355)" } else if m == FR2433_MODEL { " (FR2433)" } else { " (?)" }).await;
            Some(m)
        }
        _ => None,
    };
    // FR2433 is single-I²C → Passive-only (no sensor-master bus), so it MUST refuse Sensing.
    let single_i2c = model == Some(FR2433_MODEL);

    // 4) Firmware version — require it's PRESENT (not 0.0.0). v0.x is valid, so don't demand major != 0.
    let maj = rd(i2c, node, DBG_FW_VER_MAJOR);
    let min = rd(i2c, node, DBG_FW_VER_MINOR);
    let pat = rd(i2c, node, DBG_FW_VER_PATCH);
    if let (Some(mj), Some(mn), Some(pt)) = (maj, min, pat) {
        let _ = write_str(class, "\r\n  fw v").await;
        let _ = write_hex8(class, mj).await;
        let _ = write_str(class, ".").await;
        let _ = write_hex8(class, mn).await;
        let _ = write_str(class, ".").await;
        let _ = write_hex8(class, pt).await;
    }
    let ver_present = (maj.unwrap_or(0) | min.unwrap_or(0) | pat.unwrap_or(0)) != 0;
    report(class, "fw version present (not 0.0.0)", ver_present, t).await;

    // 5) Status byte
    if let Some(s) = rd(i2c, node, DBG_STATUS) {
        let _ = write_str(class, "\r\n  status=0x").await;
        let _ = write_hex8(class, s).await;
    }

    // 6) Mode control (I²C MODE_CTRL override). Sensing needs a 2nd I²C → dual parts only; a single-I²C
    // node (FR2433) is Passive-only and MUST reject Sensing — so test for the *refusal* instead.
    let _ = wr(i2c, node, MODE_CTRL, MODE_PASSIVE);
    Timer::after(Duration::from_millis(120)).await;
    report(class, "mode -> Passive (0x3E reads 0x00)", rd(i2c, node, MODE_CTRL) == Some(MODE_PASSIVE), t).await;
    let _ = wr(i2c, node, MODE_CTRL, MODE_SENSING);
    Timer::after(Duration::from_millis(200)).await;
    let m = rd(i2c, node, MODE_CTRL);
    if single_i2c {
        report(class, "Passive-only: Sensing correctly REFUSED (stays 0x00)", m == Some(MODE_PASSIVE), t).await;
    } else {
        report(class, "mode -> Sensing (0x3E reads 0x01)", m == Some(MODE_SENSING), t).await;
    }
    let _ = wr(i2c, node, MODE_CTRL, MODE_PASSIVE); // leave the node Passive (off the Leaf bus)

    // 7) Sideband electrical readout (raw — node-side reaction needs the FR2355 sideband firmware).
    sound.set_low(); // SoM 'active'
    por.set_high(); // power 'good'
    Timer::after(Duration::from_millis(5)).await;
    let _ = write_str(class, "\r\n  sideband: PMIC_ON_REQ=").await;
    let _ = write_str(class, if pmic_in.is_high() { "H" } else { "L" }).await;
    let _ = write_str(class, " STEM_INT=").await;
    let _ = write_str(class, if stem_in.is_high() { "H" } else { "L" }).await;
    let _ = write_str(class, "  (driving SOUND_INT=L POR_B=H)").await;
    let _ = write_str(class, "\r\n  NOTE: node sideband reaction (edge-trigger + outputs) needs the FR2355 sideband firmware (pending)\r\n> ").await;
}

// ───────────────────────── bit-banged Leaf bus (open-drain GPIO) ─────────────────────────
// The Pico owns GP4/GP5 as `Flex`, PARKED Hi-Z + pull-up when idle (the bench's only Leaf pull-ups), and
// drives them as open-drain bit-bang I²C ONLY during `leaf_scan`. No hardware I²C *controller* is ever
// enabled on these pins, so the Pico can't contend as a second master while the MSP owns Leaf in Sensing
// (Q-SENSING-ENUM defect #2 / ADR-0006). SDA is true open-drain (release=input→pull-up pulls high, 0=drive
// low); SCL is push-pull (the Pico is the sole clock during its scan). Slow (~tens of kHz) — fine for a
// presence scan.

/// ~a few µs bit half-period — slow and safe; a presence scan doesn't need speed.
fn leaf_delay() {
    for _ in 0..60u16 {
        cortex_m::asm::nop();
    }
}

/// Park the Leaf bus: both lines Hi-Z inputs with pull-ups → bus idles high, Pico OFF as a driver but
/// still providing the pull-ups the MSP master needs. Call at boot and after every `leaf_scan`.
fn leaf_park(sda: &mut Flex<'_>, scl: &mut Flex<'_>) {
    sda.set_as_input();
    sda.set_pull(Pull::Up);
    scl.set_as_input();
    scl.set_pull(Pull::Up);
}

fn sda_release(sda: &mut Flex<'_>) {
    sda.set_as_input(); // pull-up drives it high; also how we read ACK / let a slave hold it
}
fn sda_low(sda: &mut Flex<'_>) {
    sda.set_low();
    sda.set_as_output();
}

/// Open-drain write of one bit on SDA, clocked by SCL (push-pull). Returns the SDA level sampled while
/// SCL is high (used for the ACK bit). Assumes SCL starts low.
fn leaf_clock_bit(sda: &mut Flex<'_>, scl: &mut Flex<'_>, bit: bool) -> bool {
    if bit {
        sda_release(sda);
    } else {
        sda_low(sda);
    }
    leaf_delay();
    scl.set_high();
    leaf_delay();
    let sampled = sda.is_high();
    scl.set_low();
    sampled
}

/// Address-only presence probe via bit-bang: START, send addr+W, sample ACK, STOP. `true` = device ACKed.
fn leaf_probe(sda: &mut Flex<'_>, scl: &mut Flex<'_>, addr: u8) -> bool {
    // Idle → START: SCL driven high, SDA high→low while SCL high.
    sda_release(sda);
    scl.set_high();
    scl.set_as_output();
    leaf_delay();
    sda_low(sda); // START
    leaf_delay();
    scl.set_low();
    // 8 address bits (addr<<1 | W=0), MSB first.
    let byte = addr << 1;
    for i in (0..8).rev() {
        let _ = leaf_clock_bit(sda, scl, (byte >> i) & 1 == 1);
    }
    // 9th clock = ACK: release SDA, a device pulls it low.
    let ack = !leaf_clock_bit(sda, scl, true);
    // STOP: SDA low→high while SCL high.
    sda_low(sda);
    leaf_delay();
    scl.set_high();
    leaf_delay();
    sda_release(sda); // SDA rises while SCL high = STOP
    leaf_delay();
    ack
}

/// Scan 0x08..=0x77 on the bit-banged Leaf bus; returns the ACK count. Re-parks the bus on exit.
fn leaf_scan(sda: &mut Flex<'_>, scl: &mut Flex<'_>) -> u8 {
    let mut found = 0u8;
    let mut a = 0x08u8;
    while a <= 0x77 {
        if leaf_probe(sda, scl, a) {
            found += 1;
        }
        a += 1;
    }
    leaf_park(sda, scl); // back off the bus (Hi-Z + pull-up) so the MSP can own it in Sensing
    found
}

/// Bit-bang STOP: SDA low→high while SCL high.
fn leaf_stop(sda: &mut Flex<'_>, scl: &mut Flex<'_>) {
    scl.set_low();
    sda_low(sda);
    leaf_delay();
    scl.set_high();
    leaf_delay();
    sda_release(sda);
    leaf_delay();
}

/// Bit-bang START (assumes SCL already an output): SDA high→low while SCL high.
fn leaf_start(sda: &mut Flex<'_>, scl: &mut Flex<'_>) {
    sda_release(sda);
    scl.set_high();
    scl.set_as_output();
    leaf_delay();
    sda_low(sda);
    leaf_delay();
    scl.set_low();
}

/// Send one byte MSB-first; returns true if the slave ACKed (SDA low on the 9th clock).
fn leaf_send_byte(sda: &mut Flex<'_>, scl: &mut Flex<'_>, byte: u8) -> bool {
    for i in (0..8).rev() {
        let _ = leaf_clock_bit(sda, scl, (byte >> i) & 1 == 1);
    }
    !leaf_clock_bit(sda, scl, true) // release SDA, sample ACK (low = ACK)
}

/// Read one byte MSB-first; `send_ack` drives the master's ACK (true) or NACK (false) on the 9th clock.
fn leaf_read_byte(sda: &mut Flex<'_>, scl: &mut Flex<'_>, send_ack: bool) -> u8 {
    sda_release(sda); // let the slave drive SDA
    let mut v = 0u8;
    for _ in 0..8 {
        leaf_delay();
        scl.set_high();
        leaf_delay();
        v = (v << 1) | (sda.is_high() as u8);
        scl.set_low();
    }
    if send_ack {
        sda_low(sda);
    } else {
        sda_release(sda);
    }
    leaf_delay();
    scl.set_high();
    leaf_delay();
    scl.set_low();
    v
}

/// Bit-bang register read: START, addr+W, reg, repeated-START, addr+R, read 1 byte (NACK), STOP.
/// None on any NACK. Does NOT re-park (caller parks after).
fn leaf_read_reg(sda: &mut Flex<'_>, scl: &mut Flex<'_>, addr: u8, reg: u8) -> Option<u8> {
    leaf_start(sda, scl);
    if !leaf_send_byte(sda, scl, addr << 1) {
        leaf_stop(sda, scl);
        return None;
    }
    if !leaf_send_byte(sda, scl, reg) {
        leaf_stop(sda, scl);
        return None;
    }
    // repeated START
    sda_release(sda);
    leaf_delay();
    scl.set_high();
    leaf_delay();
    sda_low(sda);
    leaf_delay();
    scl.set_low();
    if !leaf_send_byte(sda, scl, (addr << 1) | 1) {
        leaf_stop(sda, scl);
        return None;
    }
    let v = leaf_read_byte(sda, scl, false); // single byte → master NACKs
    leaf_stop(sda, scl);
    Some(v)
}

/// Verify the Leaf bus: node masters + enumerates it in Sensing (read dev-count over Stem), releases it
/// in Passive (the Pico then bit-bang-masters it and sees the device) — proving the one-master handoff.
async fn verify_leaf<'d>(
    class: &mut CdcAcmClass<'d, Driver<'d, USB>>,
    i2c_stem: &mut I2c<'_, I2C1, i2c::Blocking>,
    leaf_sda: &mut Flex<'_>,
    leaf_scl: &mut Flex<'_>,
    t: &mut Tally,
) {
    let _ = write_str(class, "\r\n== VERIFY LEAF ==").await;
    let node = ADDR_SUPERVISOR; // Leaf mastering is a supervisor (Sensing) capability
    if rd(i2c_stem, node, DBG_IFACE).is_none() {
        let _ = write_str(class, "\r\n  no supervisor @0x20 — run vstem first\r\n> ").await;
        return;
    }

    // 1) Sensing: node masters Leaf + enumerates → read its device count over Stem. The Pico stays PARKED
    // (Hi-Z + pull-up) — it is NOT a bus driver here, so the MSP owns Leaf alone.
    let _ = wr(i2c_stem, node, MODE_CTRL, MODE_SENSING);
    Timer::after(Duration::from_millis(400)).await; // allow the incremental enumerate to complete
    let dev = rd(i2c_stem, node, DBG_DEV_COUNT);
    if let Some(d) = dev {
        let _ = write_str(class, "\r\n  Sensing dev_count=0x").await;
        let _ = write_hex8(class, d).await;
    }
    report(class, "Sensing: node enumerated >=1 Leaf device", dev.map_or(false, |d| d >= 1), t).await;

    // 2) Passive: node releases Leaf → NOW the Pico bit-bang-masters it (one-master invariant respected;
    // `leaf_scan` re-parks on exit so Sensing can own it again).
    let _ = wr(i2c_stem, node, MODE_CTRL, MODE_PASSIVE);
    Timer::after(Duration::from_millis(150)).await;
    let found = leaf_scan(leaf_sda, leaf_scl);
    let _ = write_str(class, "\r\n  Passive: Pico-mastered Leaf scan found 0x").await;
    let _ = write_hex8(class, found).await;
    let _ = write_str(class, " device(s)").await;
    report(class, "Passive: Pico masters Leaf, sees >=1 (node released)", found >= 1, t).await;
    let _ = write_str(class, "\r\n  (left in Passive — node + Pico both off the Leaf bus)\r\n> ").await;
}
