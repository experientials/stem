//! STEM-MSG — the 1-wire notification FRAMING (STEM-MSG.md). Frames are **null-terminated byte strings**
//! on a shared multiprocessor UART; the first byte is the sender/target **address**:
//! `[address, <payload…>, 0x00]`.
//!
//! This codec is the stable **byte-frame layer** (build / parse a frame), shared so the node (sender)
//! and the M7/host (receiver) agree on the wire format. The UART address-bit transport + inter-frame
//! idle timing live in the node/host, not here.
//!
//! The per-message TYPED payloads (sensor data, port/pin/interrupt reports, threshold crossings) are
//! listed as [`kind`] *base codes* from the doc, but STEM-MSG.md is still settling (e.g. it gives both
//! `0x8` and `0x40` for a VSOM threshold), so typed builders/parsers are deliberately NOT baked in yet —
//! callers assemble the payload and use [`encode`]. Revisit when the message spec is finalized.

/// Frame terminator (STEM-MSG.md: "messages are null-terminated byte strings").
pub const TERMINATOR: u8 = 0x00;

/// Message-type *base* codes (STEM-MSG.md "Message Types"). Several are ADDITIVE — the port number or
/// threshold index is added to the base (e.g. a port-value report is `PORT_VALUE + port`). PROVISIONAL:
/// confirm against the finalized STEM-MSG.md before relying on exact values.
pub mod kind {
    pub const SENSOR_DATA: u8 = 0x04; // [addr, 0x04, sensor-id, bytes…, 0x00]
    pub const PIN_CHANGE: u8 = 0x10; // + port: [addr, 0x10+port, pin_value&pin_index, 0x00]
    pub const INTERRUPT: u8 = 0x20; // + port: [addr, 0x20+port, interrupt-id, 0x00]
    pub const PORT_VALUE: u8 = 0xA0; // + port: [addr, 0xA0+port, value, 0x00]
}

/// Build a frame `[address, payload…, 0x00]` into `buf`. Returns the byte count written, or `None` if
/// it doesn't fit (needs `payload.len() + 2`). The caller owns the payload bytes (type + values).
pub fn encode(address: u8, payload: &[u8], buf: &mut [u8]) -> Option<usize> {
    let n = payload.len() + 2;
    if n > buf.len() {
        return None;
    }
    buf[0] = address;
    let mut i = 0;
    while i < payload.len() {
        buf[1 + i] = payload[i];
        i += 1;
    }
    buf[1 + payload.len()] = TERMINATOR;
    Some(n)
}

/// A decoded frame: the address + the payload slice (everything between the address and the terminator).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Frame<'a> {
    pub address: u8,
    pub payload: &'a [u8],
}

/// Parse the FIRST null-terminated frame in `bytes`. `None` if there's no address byte or no terminator.
/// (For a stream of frames, re-parse from `1 + address + payload.len() + 1` after each.)
pub fn decode(bytes: &[u8]) -> Option<Frame<'_>> {
    if bytes.is_empty() {
        return None;
    }
    let address = bytes[0];
    let mut i = 1;
    while i < bytes.len() {
        if bytes[i] == TERMINATOR {
            return Some(Frame { address, payload: &bytes[1..i] });
        }
        i += 1;
    }
    None // no terminator → incomplete frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let mut buf = [0u8; 8];
        // a port-value report: addr 0x20, [0xA0+port=0xA1, value=0x3C]
        let n = encode(0x20, &[kind::PORT_VALUE + 1, 0x3C], &mut buf).unwrap();
        assert_eq!(&buf[..n], &[0x20, 0xA1, 0x3C, 0x00]);
        let f = decode(&buf[..n]).unwrap();
        assert_eq!(f.address, 0x20);
        assert_eq!(f.payload, &[0xA1, 0x3C]);
    }

    #[test]
    fn decode_needs_terminator() {
        assert!(decode(&[0x20, 0xA1]).is_none()); // no null → incomplete
        assert!(decode(&[]).is_none());
    }
}
