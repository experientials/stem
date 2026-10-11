//! cros_ec EC-slave **transport glue** — maps the `bsp::stem_i2c` [`SlaveEvent`] seam onto the pure
//! [`stembus::cros_ec::EcSlave`] framer and answers master reads via `provide_tx`. Standard commands
//! (HELLO/GET_PROTOCOL_INFO) self-dispatch in the framer; the **vendor** command (`EC_CMD_STEMBUS`) carries
//! a postcard `stembus::command::Command` which we decode, run through `command::execute` against the
//! node, and re-encode as a postcard `Response` — the vendor+postcard path (COMMANDS.md §0).
//!
//! All framing + buffers live in the framer (host-tested in `stembus`); this file is the MSP430 binding +
//! the async [`serve_ec`] task (structurally identical to [`crate::stem::serve`]).

use node_hal::{SlaveEvent, StemSlave}; // host-clean slave seam — this file names no concrete PAC (6c)
use stembus::command::{self, Event, Node, Operate, EVENT_RING_LEN};
use stembus::cros_ec::{self, EcSlave};

/// Decode + run a pending vendor request against `node`, then stage the postcard `Response`. The request
/// payload borrow is dropped (the `Command` is `Copy`) before we touch the framer's response buffer.
fn serve_vendor<N: Node + Operate>(s: &mut EcSlave, node: &mut N) {
    let decoded = match s.vendor_request() {
        Some(vr) => command::wire::decode_command(vr.data), // borrows s.req; Command is owned on return
        None => return,
    };
    match decoded {
        Ok(cmd) => {
            let mut events = [Event::default(); EVENT_RING_LEN];
            let resp = command::execute(node, cmd, &mut events);
            let mut out = [0u8; cros_ec::EC_PAYLOAD_MAX];
            match command::wire::encode_response(&resp, &mut out) {
                Ok(n) => s.stage_response(cros_ec::EC_RES_SUCCESS, &out[..n]),
                Err(_) => s.stage_response(cros_ec::EC_RES_RESPONSE_TOO_BIG, &[]),
            }
        }
        // Payload wasn't a valid postcard Command.
        Err(_) => s.stage_response(cros_ec::EC_RES_INVALID_PARAM, &[]),
    }
}

/// Apply one slave-bus event to the framer; answer a read (`TxReq`) with the next staged byte. After a
/// write byte, if the framer surfaced a vendor request, dispatch it immediately (same event context, well
/// before the read phase — the eUSCI clock-stretches meanwhile).
fn handle<N: Node + Operate, H: StemSlave>(hw: &H, s: &mut EcSlave, node: &mut N, ev: SlaveEvent) {
    match ev {
        SlaveEvent::Start => s.start(),
        SlaveEvent::Rx(b) => {
            s.feed(b);
            if s.vendor_request().is_some() {
                serve_vendor(s, node);
            }
        }
        SlaveEvent::TxReq => hw.provide_tx(s.next_tx()),
        SlaveEvent::Stop => s.stop(),
        SlaveEvent::Nack => {}
    }
}

/// Async cros_ec EC-slave dispatch — mirrors [`crate::stem::serve`] (Phase 1c LPM pender): `.await` one
/// [`SlaveEvent`] at a time, returning `Pending` when the bus is idle so `sched::run_pended` runs the
/// caller's LPM `idle` closure; the eUSCI_B slave ISR wakes the CPU. Never completes. Used by fr24xx.
pub async fn serve_ec<N: Node + Operate, H: StemSlave>(hw: &H, s: &mut EcSlave, node: &mut N) {
    loop {
        let ev = core::future::poll_fn(|_cx| match hw.next_event() {
            Some(ev) => core::task::Poll::Ready(ev),
            None => core::task::Poll::Pending,
        })
        .await;
        handle(hw, s, node, ev);
    }
}

/// **Polled** cros_ec EC-slave drain (non-blocking) — the counterpart to [`crate::stem::poll`] for the
/// dual node's hand cooperative loop (`run_dual`), which services the slave alongside the Leaf scanner,
/// mode machine, and console each pass. Drains every pending [`SlaveEvent`]; vendor commands dispatch
/// against `node` (which the caller refreshes from live status + drains mode requests from).
pub fn poll<N: Node + Operate, H: StemSlave>(hw: &H, s: &mut EcSlave, node: &mut N) {
    while let Some(ev) = hw.next_event() {
        handle(hw, s, node, ev);
    }
}
