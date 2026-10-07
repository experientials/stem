//! cros_ec EC-slave **transport glue** — maps the `bsp::stem_i2c` [`SlaveEvent`] seam onto the pure
//! [`stembus::cros_ec::EcSlave`] framer and answers master reads via `provide_tx`. All logic + buffers
//! live in the framer (host-tested in `stembus`); this file is only the MSP430 binding + the async
//! [`serve_ec`] task (structurally identical to [`crate::stem::serve`]).
//!
//! This is the FR2433 + Pico **HELLO milestone** path (COMMANDS.md §0): built only with the `cros_ec`
//! feature, it replaces the PCA9698-regmap slave with the cros_ec v3 EC-slave framing on the same bus.

use bsp::pac::Peripherals;
use bsp::stem_i2c::{self, SlaveEvent};
use stembus::cros_ec::EcSlave;

/// Apply one slave-bus event to the framer; answer a read request (`TxReq`) with the next staged byte.
/// The framer handles START reset / repeated-START turnaround / STOP internally (see `EcSlave`).
fn handle(p: &Peripherals, s: &mut EcSlave, ev: SlaveEvent) {
    match ev {
        SlaveEvent::Start => s.start(),
        SlaveEvent::Rx(b) => s.feed(b),
        SlaveEvent::TxReq => stem_i2c::provide_tx(p, s.next_tx()),
        SlaveEvent::Stop => s.stop(),
        SlaveEvent::Nack => {}
    }
}

/// Async cros_ec EC-slave dispatch — mirrors [`crate::stem::serve`] (Phase 1c LPM pender): `.await` one
/// [`SlaveEvent`] at a time, returning `Pending` when the bus is idle so `sched::run_pended` runs the
/// caller's LPM `idle` closure; the eUSCI_B slave ISR wakes the CPU. Never completes.
pub async fn serve_ec(p: &Peripherals, s: &mut EcSlave) {
    loop {
        let ev = core::future::poll_fn(|_cx| match stem_i2c::next_event(p) {
            Some(ev) => core::task::Poll::Ready(ev),
            None => core::task::Poll::Pending,
        })
        .await;
        handle(p, s, ev);
    }
}
