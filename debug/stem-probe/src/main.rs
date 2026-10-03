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

use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_rp::bind_interrupts;
use embassy_rp::flash::{Blocking, Flash};
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::i2c::{self, I2c};
use embassy_rp::peripherals::{I2C0, I2C1, USB};
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_time::{Duration, Timer};
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
const STEM_ADDR: u8 = 0x20; // legacy single-target for `scan`/`read` (= supervisor role address)
const DBG_IFACE: u8 = 0x30; // → DBG_IFACE_MAGIC
const DBG_IFACE_MAGIC: u8 = 0xD0;
const DBG_MODEL_L: u8 = 0x31;
const DBG_MODEL_H: u8 = 0x32;
const FR2433_MODEL: u16 = 0x8240; // detected device-id (FR2433)
const FR2355_MODEL: u16 = 0x830C; // detected device-id (FR2355 — the current bench supervisor node)

// Role-based Stem addresses (I2C-API.md "Stem address is ROLE-BASED") — the node's address tells the
// master WHAT it is. `vstem` scans this window and labels each responder by role.
const ADDR_SUPERVISOR: u8 = 0x20; // FR2355/FR2155 — production sleep-supervisor
const ADDR_DEV_STANDIN: u8 = 0x21; // FR2476/FR2475 — dev stand-in (same fw, "not the real thing")
const ADDR_EXPANDER: u8 = 0x22; // FR2433 — I/O-expander role (placeholder)

// Debug/identity window + mode control (mirrors prod/src/regmap.rs — the shared contract).
const DBG_STATUS: u8 = 0x37;
const DBG_DEV_COUNT: u8 = 0x38; // devices the node found on its LEAF bus (last scan)
const DBG_FW_VER_MAJOR: u8 = 0x3B;
const DBG_FW_VER_MINOR: u8 = 0x3C;
const DBG_FW_VER_PATCH: u8 = 0x3D;
const MODE_CTRL: u8 = 0x3E;
const MODE_PASSIVE: u8 = 0x00;
const MODE_SENSING: u8 = 0x01;

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

    // Leaf-I²C master (I2C0: GP4 SDA / GP5 SCL). Used ONLY while the node is Passive — one master on the
    // Leaf bus at a time (the node masters it in Sensing). RP internal pull-ups, ~100 kHz. (MCU-BENCH.md)
    let mut i2c_leaf = I2c::new_blocking(p.I2C0, p.PIN_5, p.PIN_4, i2c::Config::default());

    // Sideband power-status lines — the Pico is the SoM+PMIC stand-in. It DRIVES the node's edge inputs
    // (SOUND_INT->P2.0, POR_B->P2.1) and READS its outputs (PMIC_ON_REQ<-P2.2, STEM_INT<-P2.4). Pull::Down
    // so an undriven node output reads low. Assumed polarity: SOUND_INT H=SoM-suspended, POR_B active-low
    // (H=power-good) — confirm when the FR2355 sideband firmware lands.
    let mut sound = Output::new(p.PIN_10, Level::Low); // SOUND_INT -> FR2355 P2.0 (J2.19)
    let mut por = Output::new(p.PIN_11, Level::High); // POR_B     -> FR2355 P2.1 (J4.40)
    let pmic_in = Input::new(p.PIN_12, Pull::Down); // PMIC_ON_REQ <- FR2355 P2.2 (J2.18)
    let stem_in = Input::new(p.PIN_13, Pull::Down); // STEM_INT    <- FR2355 P2.4 (J4.33)

    // Console: banner on connect, then a command loop (id|scan|read|vstem|vleaf|pass|sens|sound0/1|por0/1|pins|boot).
    let console_fut = async {
        loop {
            class.wait_connection().await;
            let _ = banner(&mut class, serial).await;
            let mut buf = [0u8; 64];
            loop {
                match class.read_packet(&mut buf).await {
                    Ok(n) => {
                        let line = &buf[..n];
                        if contains_ci(line, b"boot") {
                            let _ = write_str(&mut class, "\r\n[entering BOOTSEL]\r\n").await;
                            Timer::after(Duration::from_millis(50)).await; // let the packet flush
                            embassy_rp::rom_data::reset_to_usb_boot(0, 0); // never returns
                        } else if contains_ci(line, b"vstem") {
                            verify_stem(&mut class, &mut i2c, &mut sound, &mut por, &pmic_in, &stem_in).await;
                        } else if contains_ci(line, b"vleaf") {
                            verify_leaf(&mut class, &mut i2c, &mut i2c_leaf).await;
                        } else if contains_ci(line, b"sens") {
                            let ok = wr(&mut i2c, ADDR_SUPERVISOR, MODE_CTRL, MODE_SENSING);
                            report(&mut class, "mode -> Sensing", ok).await;
                            let _ = write_str(&mut class, "> ").await;
                        } else if contains_ci(line, b"pass") {
                            let ok = wr(&mut i2c, ADDR_SUPERVISOR, MODE_CTRL, MODE_PASSIVE);
                            report(&mut class, "mode -> Passive", ok).await;
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
                        } else if contains_ci(line, b"read") {
                            // Cross-core conformance read of the MSP430 node's regmap @ 0x20.
                            let mut iface = [0u8; 1];
                            let mut ml = [0u8; 1];
                            let mut mh = [0u8; 1];
                            let ok = i2c.blocking_write_read(STEM_ADDR, &[DBG_IFACE], &mut iface).is_ok()
                                && i2c.blocking_write_read(STEM_ADDR, &[DBG_MODEL_L], &mut ml).is_ok()
                                && i2c.blocking_write_read(STEM_ADDR, &[DBG_MODEL_H], &mut mh).is_ok();
                            if ok {
                                let model = ((mh[0] as u16) << 8) | ml[0] as u16;
                                let _ = write_str(&mut class, "\r\nstem@0x20 iface=0x").await;
                                let _ = write_hex8(&mut class, iface[0]).await;
                                let _ = write_str(&mut class, " model=0x").await;
                                let _ = write_hex8(&mut class, mh[0]).await;
                                let _ = write_hex8(&mut class, ml[0]).await;
                                let pass = iface[0] == DBG_IFACE_MAGIC && model == FR2433_MODEL;
                                let _ = write_str(
                                    &mut class,
                                    if pass { "  CONFORMANT (FR2433)\r\n> " } else { "  (unexpected)\r\n> " },
                                )
                                .await;
                            } else {
                                let _ = write_str(
                                    &mut class,
                                    "\r\nno response at 0x20 — wire RP GP18/19 -> FR2433 P1.2/3 + GND (RP pull-ups on)\r\n> ",
                                )
                                .await;
                            }
                        } else if contains_ci(line, b"id") {
                            let _ = write_str(&mut class, "\r\nid ").await;
                            let _ = class.write_packet(serial.as_bytes()).await;
                            let _ = write_str(&mut class, "\r\n> ").await;
                        } else {
                            let _ = write_str(&mut class, "\r\ncmds: id | scan | read | vstem | vleaf | pass | sens | sound0/1 | por0/1 | pins | boot\r\n> ").await;
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
    match addr {
        ADDR_SUPERVISOR => "supervisor (FR2355/FR2155)",
        ADDR_DEV_STANDIN => "dev stand-in (FR2476)",
        ADDR_EXPANDER => "I/O-expander (FR2433)",
        _ => "?",
    }
}

/// Emit a `[PASS]`/`[FAIL]` line.
async fn report<'d>(class: &mut CdcAcmClass<'d, Driver<'d, USB>>, label: &str, ok: bool) {
    let _ = write_str(class, if ok { "\r\n  [PASS] " } else { "\r\n  [FAIL] " }).await;
    let _ = write_str(class, label).await;
}

/// Verify the full Stem-bus surface: role discovery, identity, version, status, mode control, sideband.
async fn verify_stem<'d>(
    class: &mut CdcAcmClass<'d, Driver<'d, USB>>,
    i2c: &mut I2c<'_, I2C1, i2c::Blocking>,
    sound: &mut Output<'_>,
    por: &mut Output<'_>,
    pmic_in: &Input<'_>,
    stem_in: &Input<'_>,
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
    report(class, "a Stem node answered (0x20/0x21/0x22)", node != 0).await;
    if node == 0 {
        let _ = write_str(class, "\r\n  (no node — check Stem wiring: GP18/19 <-> P4.6/P4.7 + GND)\r\n> ").await;
        return;
    }

    // 2) Identity magic
    report(class, "DBG_IFACE == 0xD0", rd(i2c, node, DBG_IFACE) == Some(DBG_IFACE_MAGIC)).await;

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
    report(class, "fw version present (not 0.0.0)", ver_present).await;

    // 5) Status byte
    if let Some(s) = rd(i2c, node, DBG_STATUS) {
        let _ = write_str(class, "\r\n  status=0x").await;
        let _ = write_hex8(class, s).await;
    }

    // 6) Mode control (I²C MODE_CTRL override). Sensing needs a 2nd I²C → dual parts only; a single-I²C
    // node (FR2433) is Passive-only and MUST reject Sensing — so test for the *refusal* instead.
    let _ = wr(i2c, node, MODE_CTRL, MODE_PASSIVE);
    Timer::after(Duration::from_millis(120)).await;
    report(class, "mode -> Passive (0x3E reads 0x00)", rd(i2c, node, MODE_CTRL) == Some(MODE_PASSIVE)).await;
    let _ = wr(i2c, node, MODE_CTRL, MODE_SENSING);
    Timer::after(Duration::from_millis(200)).await;
    let m = rd(i2c, node, MODE_CTRL);
    if single_i2c {
        report(class, "Passive-only: Sensing correctly REFUSED (stays 0x00)", m == Some(MODE_PASSIVE)).await;
    } else {
        report(class, "mode -> Sensing (0x3E reads 0x01)", m == Some(MODE_SENSING)).await;
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

/// Verify the Leaf bus: node masters + enumerates it in Sensing (read dev-count over Stem), releases it
/// in Passive (the Pico then masters it and sees the extender) — proving the one-master handoff.
async fn verify_leaf<'d>(
    class: &mut CdcAcmClass<'d, Driver<'d, USB>>,
    i2c_stem: &mut I2c<'_, I2C1, i2c::Blocking>,
    i2c_leaf: &mut I2c<'_, I2C0, i2c::Blocking>,
) {
    let _ = write_str(class, "\r\n== VERIFY LEAF ==").await;
    let node = ADDR_SUPERVISOR; // Leaf mastering is a supervisor (Sensing) capability
    if rd(i2c_stem, node, DBG_IFACE).is_none() {
        let _ = write_str(class, "\r\n  no supervisor @0x20 — run vstem first\r\n> ").await;
        return;
    }

    // 1) Sensing: node masters Leaf + enumerates → read its device count over Stem (Pico stays OFF Leaf).
    let _ = wr(i2c_stem, node, MODE_CTRL, MODE_SENSING);
    Timer::after(Duration::from_millis(300)).await; // allow the enumerate scan
    let dev = rd(i2c_stem, node, DBG_DEV_COUNT);
    if let Some(d) = dev {
        let _ = write_str(class, "\r\n  Sensing dev_count=0x").await;
        let _ = write_hex8(class, d).await;
    }
    report(class, "Sensing: node enumerated >=1 Leaf device", dev.map_or(false, |d| d >= 1)).await;

    // 2) Passive: node releases Leaf → NOW the Pico may master it (one-master invariant respected).
    let _ = wr(i2c_stem, node, MODE_CTRL, MODE_PASSIVE);
    Timer::after(Duration::from_millis(150)).await;
    let mut found = 0u8;
    let mut a = 0x08u8;
    while a <= 0x77 {
        let mut b = [0u8; 1];
        if i2c_leaf.blocking_read(a, &mut b).is_ok() {
            found += 1;
        }
        a += 1;
    }
    let _ = write_str(class, "\r\n  Passive: Pico-mastered Leaf scan found 0x").await;
    let _ = write_hex8(class, found).await;
    let _ = write_str(class, " device(s)").await;
    report(class, "Passive: Pico masters Leaf, sees >=1 (node released)", found >= 1).await;
    let _ = write_str(class, "\r\n  (left in Passive — node off the Leaf bus)\r\n> ").await;
}
