//! cros_ec v3 host-command **I²C wire framing** — encode/decode the ChromeOS EC packet so a Linux
//! `cros_ec_i2c` master (or our RP2040 `stem-probe` stand-in) and the MSP430 EC slave agree **by
//! construction**. This is the framing layer only (COMMANDS.md §0 "thinnest slice"): it turns a
//! `(command, payload)` pair into the on-wire request bytes and back, and a `(result, payload)` response
//! likewise. Command *meaning* (HELLO echo, our vendor commands) layers on top — see [`crate::command`].
//!
//! **Byte layout is pinned to the mainline Linux driver, cached verbatim** at
//! `docs/references/cache/cros-ec/` (extract: `docs/references/cros-ec-wire-framing.md`). Confirmed there:
//! - request  (master→EC): `[0]=0xda | [1..9]=ec_host_request | [9..]=payload`
//! - response (EC→master): `[0]=result | [1]=packet_length | [2..10]=ec_host_response | [10..]=payload`
//! - **checksum (both ways):** header + payload + the checksum field itself sum to **0 mod 256**
//!   (`request->checksum = -csum`, `cros_ec_proto.c:87`). The `0xda` request prefix (driver does
//!   `dout++` before `cros_ec_prepare_tx`) and the `result`/`packet_length` response prefix are **not**
//!   summed.
//! - `packet_length = sizeof(ec_host_response) + data_len`; the driver rejects `< 8` (`cros_ec_i2c.c:144`).
//!
//! All multi-byte fields are little-endian. `no_std`, no alloc: the caller owns all buffers.

/// `EC_COMMAND_PROTOCOL_3` — the leading request byte that selects protocol v3.
pub const EC_COMMAND_PROTOCOL_3: u8 = 0xda;
pub const EC_HOST_REQUEST_VERSION: u8 = 3;
pub const EC_HOST_RESPONSE_VERSION: u8 = 3;

/// `sizeof(struct ec_host_request)` / `sizeof(struct ec_host_response)` — both 8 bytes.
pub const REQ_HDR: usize = 8;
pub const RESP_HDR: usize = 8;
/// I²C wrapper prefixes: 1 byte (`0xda`) on the request, 2 bytes (`result`,`packet_length`) on the response.
pub const I2C_REQ_PREFIX: usize = 1;
pub const I2C_RESP_PREFIX: usize = 2;

// Command ids used by the first slice (see the cached cros_ec_commands.h / the extract doc).
pub const EC_CMD_HELLO: u16 = 0x0001;
pub const EC_CMD_GET_VERSION: u16 = 0x0002;
pub const EC_CMD_GET_BUILD_INFO: u16 = 0x0004;
pub const EC_CMD_GET_PROTOCOL_INFO: u16 = 0x000B;
/// `EC_CMD_HELLO` echoes `in_data + 0x01020304` (cros_ec_commands.h).
pub const HELLO_MAGIC: u32 = 0x0102_0304;

// Result codes (enum ec_status) — the subset the v0 slice uses.
pub const EC_RES_SUCCESS: u16 = 0;
pub const EC_RES_INVALID_COMMAND: u16 = 1;
pub const EC_RES_INVALID_PARAM: u16 = 3;
pub const EC_RES_INVALID_CHECKSUM: u16 = 7;

/// A framing error from decoding a cros_ec packet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// Buffer shorter than the declared packet (prefix+header+data_len).
    TooShort,
    /// Request did not start with `EC_COMMAND_PROTOCOL_3` (0xda).
    BadProtocol,
    /// `struct_version` field was not 3.
    BadVersion,
    /// Header+payload did not sum to zero (mod 256).
    BadChecksum,
    /// Output buffer too small to hold the frame, or payload > 65535.
    BufferTooSmall,
}

#[inline]
fn sum8(bytes: &[u8]) -> u8 {
    let mut s = 0u8;
    for &b in bytes {
        s = s.wrapping_add(b);
    }
    s
}

/// The checksum byte that makes `bytes` + itself sum to 0 (mod 256) — i.e. `-sum` (`cros_ec_proto.c:87`).
#[inline]
pub fn checksum(bytes: &[u8]) -> u8 {
    0u8.wrapping_sub(sum8(bytes))
}

/// A decoded request (borrowed from the input buffer) — what the **EC slave** gets.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Request<'a> {
    pub command: u16,
    pub command_version: u8,
    pub data: &'a [u8],
}

/// A decoded response (borrowed) — what the **master** gets back.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Response<'a> {
    pub result: u16,
    pub data: &'a [u8],
}

/// **Master side:** build the on-wire request (`0xda | ec_host_request | payload`) into `out`.
/// Returns the frame length. Checksum covers the 8-byte header + payload (not the `0xda` prefix).
pub fn encode_request(
    command: u16,
    command_version: u8,
    data: &[u8],
    out: &mut [u8],
) -> Result<usize, Error> {
    if data.len() > u16::MAX as usize {
        return Err(Error::BufferTooSmall);
    }
    let total = I2C_REQ_PREFIX + REQ_HDR + data.len();
    if out.len() < total {
        return Err(Error::BufferTooSmall);
    }
    let dl = data.len() as u16;
    out[0] = EC_COMMAND_PROTOCOL_3;
    let h = I2C_REQ_PREFIX; // start of ec_host_request
    out[h] = EC_HOST_REQUEST_VERSION;
    out[h + 1] = 0; // checksum placeholder
    out[h + 2] = command as u8;
    out[h + 3] = (command >> 8) as u8;
    out[h + 4] = command_version;
    out[h + 5] = 0; // reserved
    out[h + 6] = dl as u8;
    out[h + 7] = (dl >> 8) as u8;
    out[h + REQ_HDR..total].copy_from_slice(data);
    out[h + 1] = checksum(&out[h..total]);
    Ok(total)
}

/// **EC-slave side:** decode a buffered write transaction (`0xda | ec_host_request | payload`).
/// Verifies the protocol byte, version, and checksum.
pub fn parse_request(buf: &[u8]) -> Result<Request<'_>, Error> {
    if buf.len() < I2C_REQ_PREFIX + REQ_HDR {
        return Err(Error::TooShort);
    }
    if buf[0] != EC_COMMAND_PROTOCOL_3 {
        return Err(Error::BadProtocol);
    }
    let h = I2C_REQ_PREFIX;
    if buf[h] != EC_HOST_REQUEST_VERSION {
        return Err(Error::BadVersion);
    }
    let data_len = u16::from_le_bytes([buf[h + 6], buf[h + 7]]) as usize;
    let total = I2C_REQ_PREFIX + REQ_HDR + data_len;
    if buf.len() < total {
        return Err(Error::TooShort);
    }
    if sum8(&buf[h..total]) != 0 {
        return Err(Error::BadChecksum);
    }
    Ok(Request {
        command: u16::from_le_bytes([buf[h + 2], buf[h + 3]]),
        command_version: buf[h + 4],
        data: &buf[h + REQ_HDR..total],
    })
}

/// **EC-slave side:** build the on-wire response (`result | packet_length | ec_host_response | payload`)
/// into `out`. The `result` is written both to the I²C prefix byte and the header's u16 `result` field
/// (the master reads the prefix byte for its status switch). Returns the frame length.
pub fn encode_response(result: u16, data: &[u8], out: &mut [u8]) -> Result<usize, Error> {
    if data.len() > (u8::MAX as usize - RESP_HDR) {
        // packet_length is one byte = RESP_HDR + data_len, so data_len <= 247 on this (small) path.
        return Err(Error::BufferTooSmall);
    }
    let total = I2C_RESP_PREFIX + RESP_HDR + data.len();
    if out.len() < total {
        return Err(Error::BufferTooSmall);
    }
    let dl = data.len() as u16;
    out[0] = result as u8; // I²C result byte
    out[1] = (RESP_HDR + data.len()) as u8; // packet_length (>= 8)
    let h = I2C_RESP_PREFIX; // start of ec_host_response
    out[h] = EC_HOST_RESPONSE_VERSION;
    out[h + 1] = 0; // checksum placeholder
    out[h + 2] = result as u8;
    out[h + 3] = (result >> 8) as u8;
    out[h + 4] = dl as u8;
    out[h + 5] = (dl >> 8) as u8;
    out[h + 6] = 0; // reserved u16
    out[h + 7] = 0;
    out[h + RESP_HDR..total].copy_from_slice(data);
    out[h + 1] = checksum(&out[h..total]);
    Ok(total)
}

/// **Master side:** decode a response read back from the EC. Verifies version, `packet_length >= 8`,
/// length, and checksum, then returns the header `result` + payload.
pub fn parse_response(buf: &[u8]) -> Result<Response<'_>, Error> {
    if buf.len() < I2C_RESP_PREFIX + RESP_HDR {
        return Err(Error::TooShort);
    }
    if (buf[1] as usize) < RESP_HDR {
        // packet_length < sizeof(ec_host_response) → the driver errors (cros_ec_i2c.c:144).
        return Err(Error::TooShort);
    }
    let h = I2C_RESP_PREFIX;
    if buf[h] != EC_HOST_RESPONSE_VERSION {
        return Err(Error::BadVersion);
    }
    let data_len = u16::from_le_bytes([buf[h + 4], buf[h + 5]]) as usize;
    let total = I2C_RESP_PREFIX + RESP_HDR + data_len;
    if buf.len() < total {
        return Err(Error::TooShort);
    }
    if sum8(&buf[h..total]) != 0 {
        return Err(Error::BadChecksum);
    }
    Ok(Response {
        result: u16::from_le_bytes([buf[h + 2], buf[h + 3]]),
        data: &buf[h + RESP_HDR..total],
    })
}

/// `EC_CMD_HELLO` reply value: `in_data + 0x01020304`.
#[inline]
pub const fn hello_reply(in_data: u32) -> u32 {
    in_data.wrapping_add(HELLO_MAGIC)
}

/// Max command/response payload the on-chip EC-slave framer buffers (keeps RAM small on FR2433).
pub const EC_PAYLOAD_MAX: usize = 32;
const FRAMER_REQ_MAX: usize = I2C_REQ_PREFIX + REQ_HDR + EC_PAYLOAD_MAX;
const FRAMER_RESP_MAX: usize = I2C_RESP_PREFIX + RESP_HDR + EC_PAYLOAD_MAX;

/// Over-read filler returned when a master clocks a read with no (or exhausted) staged response.
pub const EC_FILLER: u8 = 0xEC;

/// EC-slave-side **framer + dispatcher** — a pure, host-testable state machine driven by the four I²C
/// slave events (START / byte-in / byte-out / STOP). It accumulates a master write transaction into a
/// request frame, [`parse_request`]s it when the full declared frame has arrived, dispatches (HELLO for
/// the v0 slice), stages the response via [`encode_response`], and hands it out byte-by-byte on the read
/// phase. The hardware glue (map `bsp::stem_i2c::SlaveEvent` → these calls, answer `TxReq` via
/// `provide_tx`) lives in `deepsleep::ec`; this type touches **no peripheral**, so it unit-tests on the
/// host — the purity seam of `firmware-architecture.md` §11.
///
/// Staging happens the instant the full frame arrives (inside [`feed`](EcSlave::feed)), so it is robust
/// to both framings a master uses — repeated-START (write→read turnaround; `cros_ec_i2c`'s 2-message
/// transfer) and STOP-then-START — and the eUSCI's clock-stretch covers the compute latency.
pub struct EcSlave {
    req: [u8; FRAMER_REQ_MAX],
    req_len: usize,
    resp: [u8; FRAMER_RESP_MAX],
    resp_len: usize,
    resp_pos: usize,
    staged: bool,
    done: bool,
}

impl EcSlave {
    pub const fn new() -> Self {
        Self {
            req: [0; FRAMER_REQ_MAX],
            req_len: 0,
            resp: [0; FRAMER_RESP_MAX],
            resp_len: 0,
            resp_pos: 0,
            staged: false,
            done: true,
        }
    }

    /// START matching our address. A *fresh* start (the previous transaction ended in STOP) resets the
    /// framer; a repeated START mid-transaction (the write→read turnaround) is a no-op so the just-staged
    /// response survives into the read phase.
    pub fn start(&mut self) {
        if self.done {
            self.req_len = 0;
            self.resp_len = 0;
            self.resp_pos = 0;
            self.staged = false;
            self.done = false;
        }
    }

    /// A byte the master wrote. Buffers it; once the full declared frame has arrived, dispatches + stages.
    pub fn feed(&mut self, b: u8) {
        if self.req_len < FRAMER_REQ_MAX {
            self.req[self.req_len] = b;
            self.req_len += 1;
        }
        if !self.staged && self.frame_complete() {
            self.dispatch();
        }
    }

    /// The next response byte for a master read — [`EC_FILLER`] before a response is staged or past its
    /// end (the cros_ec over-read convention).
    pub fn next_tx(&mut self) -> u8 {
        if self.staged && self.resp_pos < self.resp_len {
            let v = self.resp[self.resp_pos];
            self.resp_pos += 1;
            v
        } else {
            EC_FILLER
        }
    }

    /// STOP — the transaction is over; the next START begins a fresh request.
    pub fn stop(&mut self) {
        self.done = true;
    }

    fn frame_complete(&self) -> bool {
        let hdr_end = I2C_REQ_PREFIX + REQ_HDR;
        if self.req_len < hdr_end {
            return false;
        }
        let data_len =
            u16::from_le_bytes([self.req[I2C_REQ_PREFIX + 6], self.req[I2C_REQ_PREFIX + 7]]) as usize;
        self.req_len >= hdr_end + data_len
    }

    fn dispatch(&mut self) {
        // Parse into owned locals so the borrow of `self.req` ends before we write `self.resp`.
        let (result, payload) = match parse_request(&self.req[..self.req_len]) {
            Ok(r) => dispatch_command(&r),
            Err(Error::BadChecksum) => (EC_RES_INVALID_CHECKSUM, Payload::empty()),
            Err(_) => (EC_RES_INVALID_COMMAND, Payload::empty()),
        };
        self.resp_len = encode_response(result, payload.as_slice(), &mut self.resp).unwrap_or(0);
        self.resp_pos = 0;
        self.staged = self.resp_len > 0;
    }
}

impl Default for EcSlave {
    fn default() -> Self {
        Self::new()
    }
}

/// Owned payload scratch so [`EcSlave::dispatch`] doesn't hold a borrow of the request buffer while
/// writing the response buffer (both are fields of `EcSlave`).
struct Payload {
    buf: [u8; EC_PAYLOAD_MAX],
    len: usize,
}
impl Payload {
    const fn empty() -> Self {
        Self { buf: [0; EC_PAYLOAD_MAX], len: 0 }
    }
    fn from_bytes(src: &[u8]) -> Self {
        let mut buf = [0u8; EC_PAYLOAD_MAX];
        let n = if src.len() < EC_PAYLOAD_MAX { src.len() } else { EC_PAYLOAD_MAX };
        buf[..n].copy_from_slice(&src[..n]);
        Self { buf, len: n }
    }
    fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

/// The v0 **standard-command** dispatch (COMMANDS.md §0): `HELLO` echoes `in_data + 0x01020304`. Unknown
/// commands → `INVALID_COMMAND`; a short `HELLO` payload → `INVALID_PARAM`. `GET_VERSION`/`GET_BUILD_INFO`
/// and the vendor+postcard commands are added here as later slices land.
fn dispatch_command(r: &Request) -> (u16, Payload) {
    match r.command {
        EC_CMD_HELLO => {
            if r.data.len() < 4 {
                return (EC_RES_INVALID_PARAM, Payload::empty());
            }
            let in_data = u32::from_le_bytes([r.data[0], r.data[1], r.data[2], r.data[3]]);
            (EC_RES_SUCCESS, Payload::from_bytes(&hello_reply(in_data).to_le_bytes()))
        }
        _ => (EC_RES_INVALID_COMMAND, Payload::empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_makes_packet_sum_to_zero() {
        let body = [3u8, 0, 0x01, 0x00, 0, 0, 0x04, 0x00, 0xDE, 0xAD];
        let c = checksum(&body);
        let mut total = 0u8;
        for &b in &body {
            total = total.wrapping_add(b);
        }
        assert_eq!(total.wrapping_add(c), 0);
    }

    #[test]
    fn hello_round_trip_master_to_ec_and_back() {
        // Master encodes a HELLO request with in_data...
        let in_data: u32 = 0xA0B0_C0D0;
        let mut req = [0u8; 32];
        let n = encode_request(EC_CMD_HELLO, 0, &in_data.to_le_bytes(), &mut req).unwrap();
        assert_eq!(n, I2C_REQ_PREFIX + REQ_HDR + 4);
        assert_eq!(req[0], EC_COMMAND_PROTOCOL_3);

        // EC slave parses it, checks the command, computes the reply...
        let r = parse_request(&req[..n]).unwrap();
        assert_eq!(r.command, EC_CMD_HELLO);
        assert_eq!(r.data.len(), 4);
        let got_in = u32::from_le_bytes([r.data[0], r.data[1], r.data[2], r.data[3]]);
        assert_eq!(got_in, in_data);
        let out_data = hello_reply(got_in);

        // ...and encodes the response.
        let mut resp = [0u8; 32];
        let m = encode_response(EC_RES_SUCCESS, &out_data.to_le_bytes(), &mut resp).unwrap();
        assert_eq!(resp[0], EC_RES_SUCCESS as u8);
        assert_eq!(resp[1] as usize, RESP_HDR + 4); // packet_length

        // Master parses the response: SUCCESS + out_data = in_data + 0x01020304.
        let pr = parse_response(&resp[..m]).unwrap();
        assert_eq!(pr.result, EC_RES_SUCCESS);
        let out = u32::from_le_bytes([pr.data[0], pr.data[1], pr.data[2], pr.data[3]]);
        assert_eq!(out, in_data.wrapping_add(0x0102_0304));
    }

    #[test]
    fn request_rejects_bad_protocol_byte() {
        let mut req = [0u8; 16];
        let n = encode_request(EC_CMD_HELLO, 0, &[0u8; 4], &mut req).unwrap();
        req[0] = 0x00; // not 0xda
        assert_eq!(parse_request(&req[..n]), Err(Error::BadProtocol));
    }

    #[test]
    fn request_rejects_corrupted_payload_via_checksum() {
        let mut req = [0u8; 16];
        let n = encode_request(EC_CMD_HELLO, 0, &[1u8, 2, 3, 4], &mut req).unwrap();
        req[n - 1] ^= 0xFF; // flip a payload byte
        assert_eq!(parse_request(&req[..n]), Err(Error::BadChecksum));
    }

    #[test]
    fn response_rejects_short_packet_length() {
        let mut resp = [0u8; 16];
        let m = encode_response(EC_RES_SUCCESS, &[0u8; 4], &mut resp).unwrap();
        resp[1] = (RESP_HDR as u8) - 1; // packet_length < 8
        assert_eq!(parse_response(&resp[..m]), Err(Error::TooShort));
    }

    #[test]
    fn encode_request_errors_when_buffer_too_small() {
        let mut tiny = [0u8; 4];
        assert_eq!(
            encode_request(EC_CMD_HELLO, 0, &[0u8; 4], &mut tiny),
            Err(Error::BufferTooSmall)
        );
    }

    // Drain the framer's read phase into `out`; returns a slice long enough for parse_response (the
    // real response, with trailing EC_FILLER beyond it — parse_response bounds itself by data_len).
    fn drain<'a>(f: &mut EcSlave, out: &'a mut [u8]) -> &'a [u8] {
        for slot in out.iter_mut() {
            *slot = f.next_tx();
        }
        out
    }

    #[test]
    fn framer_hello_transaction_repeated_start() {
        // Write then read with NO stop between (the cros_ec_i2c 2-message / repeated-START framing).
        let mut f = EcSlave::new();
        let in_data = 0x1122_3344u32;
        let mut req = [0u8; 32];
        let n = encode_request(EC_CMD_HELLO, 0, &in_data.to_le_bytes(), &mut req).unwrap();
        f.start();
        for &b in &req[..n] {
            f.feed(b);
        }
        let mut resp = [0u8; 32];
        let pr = parse_response(drain(&mut f, &mut resp)).unwrap();
        assert_eq!(pr.result, EC_RES_SUCCESS);
        let out = u32::from_le_bytes([pr.data[0], pr.data[1], pr.data[2], pr.data[3]]);
        assert_eq!(out, in_data.wrapping_add(0x0102_0304));
        f.stop();
    }

    #[test]
    fn framer_unknown_command_returns_invalid() {
        let mut f = EcSlave::new();
        let mut req = [0u8; 16];
        let n = encode_request(0x7FFF, 0, &[], &mut req).unwrap();
        f.start();
        for &b in &req[..n] {
            f.feed(b);
        }
        let mut resp = [0u8; 16];
        let pr = parse_response(drain(&mut f, &mut resp)).unwrap();
        assert_eq!(pr.result, EC_RES_INVALID_COMMAND);
        assert!(pr.data.is_empty());
    }

    #[test]
    fn framer_resets_between_transactions() {
        let mut f = EcSlave::new();
        let mut req = [0u8; 32];
        // txn 1
        let n1 = encode_request(EC_CMD_HELLO, 0, &0u32.to_le_bytes(), &mut req).unwrap();
        f.start();
        for &b in &req[..n1] {
            f.feed(b);
        }
        let _ = f.next_tx();
        f.stop();
        // txn 2 — different in_data must not see stale req/resp from txn 1
        let n2 = encode_request(EC_CMD_HELLO, 0, &0xDEAD_BEEFu32.to_le_bytes(), &mut req).unwrap();
        f.start();
        for &b in &req[..n2] {
            f.feed(b);
        }
        let mut resp = [0u8; 32];
        let pr = parse_response(drain(&mut f, &mut resp)).unwrap();
        let out = u32::from_le_bytes([pr.data[0], pr.data[1], pr.data[2], pr.data[3]]);
        assert_eq!(out, 0xDEAD_BEEFu32.wrapping_add(0x0102_0304));
    }

    #[test]
    fn framer_bad_checksum_reports_invalid_checksum() {
        let mut f = EcSlave::new();
        let mut req = [0u8; 32];
        let n = encode_request(EC_CMD_HELLO, 0, &0u32.to_le_bytes(), &mut req).unwrap();
        req[n - 1] ^= 0xFF; // corrupt a payload byte
        f.start();
        for &b in &req[..n] {
            f.feed(b);
        }
        let mut resp = [0u8; 32];
        let pr = parse_response(drain(&mut f, &mut resp)).unwrap();
        assert_eq!(pr.result, EC_RES_INVALID_CHECKSUM);
    }

    #[test]
    fn framer_read_before_request_yields_filler() {
        let mut f = EcSlave::new();
        f.start();
        assert_eq!(f.next_tx(), EC_FILLER); // no request staged yet
    }

    #[test]
    fn empty_payload_round_trips() {
        let mut req = [0u8; 16];
        let n = encode_request(EC_CMD_GET_PROTOCOL_INFO, 0, &[], &mut req).unwrap();
        let r = parse_request(&req[..n]).unwrap();
        assert_eq!(r.command, EC_CMD_GET_PROTOCOL_INFO);
        assert!(r.data.is_empty());
        let mut resp = [0u8; 16];
        let m = encode_response(EC_RES_SUCCESS, &[], &mut resp).unwrap();
        let pr = parse_response(&resp[..m]).unwrap();
        assert_eq!(pr.result, EC_RES_SUCCESS);
        assert!(pr.data.is_empty());
    }
}
