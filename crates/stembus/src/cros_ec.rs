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
