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
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::USB;
use embassy_rp::usb::{Driver, InterruptHandler};
use embassy_time::{Duration, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State};
use embassy_usb::control::{OutResponse, Request, RequestType};
use embassy_usb::driver::EndpointError;
use embassy_usb::{Builder, Config, Handler};

const FLASH_SIZE: usize = 2 * 1024 * 1024;
const FW_VERSION: &str = env!("CARGO_PKG_VERSION");

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

    // Console: banner on connect, then a tiny command loop (`boot` → BOOTSEL).
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
                        } else if contains_ci(line, b"id") {
                            let _ = write_str(&mut class, "\r\nid ").await;
                            let _ = class.write_packet(serial.as_bytes()).await;
                            let _ = write_str(&mut class, "\r\n> ").await;
                        } else {
                            let _ = write_str(&mut class, "\r\ncmds: id | boot\r\n> ").await;
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
