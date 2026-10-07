# cros_ec wire framing — local spec capture (for the FR2433+Pico HELLO milestone)

Saved locally per standing practice (don't re-derive protocol bytes from memory — see
`.claude/rules/save-sources-locally.md`). Source-of-truth = the ChromeOS EC protocol as implemented by
the mainline Linux driver + the kernel's mirror of `ec_commands.h`. Verbatim quotes + line refs below;
**precision markers** flag what is quoted vs described vs to-confirm.

## Sources (fetched 2026-10-07)
- `struct`/`#define` definitions: **`include/linux/platform_data/cros_ec_commands.h`** —
  https://codebrowser.dev/linux/linux/include/linux/platform_data/cros_ec_commands.h.html
- I²C transport framing + checksum: **`drivers/platform/chrome/cros_ec_i2c.c`** —
  https://codebrowser.dev/linux/linux/drivers/platform/chrome/cros_ec_i2c.c.html
- Protocol overview (no byte-level detail): https://chromium.googlesource.com/chromiumos/platform/ec/+/HEAD/docs/ap-ec-comm.md

## The v3 packet headers — VERBATIM (cros_ec_commands.h)
```c
// line 755
struct ec_host_request {
  uint8_t  struct_version;   // = EC_HOST_REQUEST_VERSION (3)
  uint8_t  checksum;         // byte that makes the whole request sum to 0 (mod 256)
  uint16_t command;          // EC_CMD_*
  uint8_t  command_version;
  uint8_t  reserved;
  uint16_t data_len;         // payload length AFTER this header
} __ec_align4;               // TOTAL 8 bytes

// line 775
struct ec_host_response {
  uint8_t  struct_version;   // = EC_HOST_RESPONSE_VERSION (3)
  uint8_t  checksum;         // byte that makes the whole response sum to 0 (mod 256)
  uint16_t result;           // EC_RES_* status
  uint16_t data_len;         // payload length AFTER this header
  uint16_t reserved;
} __ec_align4;               // TOTAL 8 bytes
```
```c
#define EC_HOST_REQUEST_VERSION  3   // line 741
#define EC_HOST_RESPONSE_VERSION 3   // line 764
```
All multi-byte fields are **little-endian** (x86/ARM EC convention). `__ec_align4` = 4-byte aligned; the
layout above is already naturally packed to 8 bytes with no padding.

## Checksum rule — BOTH directions VERBATIM (confirmed 2026-10-07)
Same rule each way: the checksum byte is set so **(header + payload + checksum field) ≡ 0 mod 256**.

**Request (`cros_ec_prepare_tx()`, cros_ec_proto.c — VERBATIM):**
```c
request->struct_version = EC_HOST_REQUEST_VERSION;
request->checksum = 0;
request->command = msg->command;
request->command_version = msg->version;
request->reserved = 0;
request->data_len = msg->outsize;

for (i = 0; i < sizeof(*request); i++)   /* sum the 8 header bytes (checksum field currently 0) */
  csum += out[i];
memcpy(out + sizeof(*request), msg->data, msg->outsize);
for (i = 0; i < msg->outsize; i++)       /* + every payload byte */
  csum += msg->data[i];
request->checksum = -csum;                /* two's complement → whole packet sums to 0 mod 256 */
```
So: zero the checksum field, sum header+payload as u8, store `(-csum) & 0xff`.

**Response (`cros_ec_pkt_xfer_i2c()`, cros_ec_i2c.c lines ~162–173 — VERBATIM):** the master re-sums and
rejects a non-zero total:
```c
sum = 0;
for (i = 0; i < sizeof(struct ec_host_response); i++) sum += ((u8 *)ec_response)[i];
memcpy(msg->data, in_buf + response_header_size, ec_response->data_len);
for (i = 0; i < ec_response->data_len; i++) sum += msg->data[i];
if (sum) /* -> error (EC_RES_INVALID_CHECKSUM path) */
```
The EC builds its response checksum the same way (`= -csum`). **NB: the I²C wrapper's `result`/
`packet_length` bytes are NOT part of the `ec_host_response` checksum — only the 8-byte header + payload.**

## I²C transport wrapper — struct declarations VERBATIM (cros_ec_i2c.c, confirmed 2026-10-07)
`EC_COMMAND_PROTOCOL_3 = 0xda` (quoted). Both wrappers are `__packed` (no padding → the byte offsets below
are exact):
```c
struct ec_host_request_i2c {
  /* Always 0xda to backward compatible with v2 struct */
  uint8_t command_protocol;
  struct ec_host_request ec_request;
} __packed;

struct ec_host_response_i2c {
  uint8_t result;
  uint8_t packet_length;
  struct ec_host_response ec_response;
} __packed;
```
`request_header_size  = sizeof(struct ec_host_request_i2c)`  = 1 + 8 = **9**
`response_header_size = sizeof(struct ec_host_response_i2c)` = 1 + 1 + 8 = **10**

**Master → EC (request on the wire):**
| offset | field |
|---|---|
| 0 | `command_protocol` = `0xda` |
| 1..8 | `struct ec_host_request` (8 bytes; byte 2 of it = its checksum) |
| 9.. | command payload (`data_len` bytes) |

**EC → master (response read back):**
| offset | field |
|---|---|
| 0 | `result` (EC_RES_*) — the I²C-level result |
| 1 | `packet_length` (response payload size) |
| 2..9 | `struct ec_host_response` (8 bytes) |
| 10.. | response payload (`data_len` bytes) |

On I²C the **EC clock-stretches** until the response is staged (our MSP430 eUSCI slave does this natively,
slau445i §24.3) and returns filler on over-reads.

## Result codes — `enum ec_status` VERBATIM (cros_ec_commands.h lines 466–488, confirmed 2026-10-07)
```c
enum ec_status {
  EC_RES_SUCCESS = 0,            EC_RES_INVALID_COMMAND = 1,   EC_RES_ERROR = 2,
  EC_RES_INVALID_PARAM = 3,      EC_RES_ACCESS_DENIED = 4,     EC_RES_INVALID_RESPONSE = 5,
  EC_RES_INVALID_VERSION = 6,    EC_RES_INVALID_CHECKSUM = 7,  EC_RES_IN_PROGRESS = 8,
  EC_RES_UNAVAILABLE = 9,        EC_RES_TIMEOUT = 10,          EC_RES_OVERFLOW = 11,
  EC_RES_INVALID_HEADER = 12,    EC_RES_REQUEST_TRUNCATED = 13, EC_RES_RESPONSE_TOO_BIG = 14,
  EC_RES_BUS_ERROR = 15,         EC_RES_BUSY = 16,             EC_RES_INVALID_HEADER_VERSION = 17,
  EC_RES_INVALID_HEADER_CRC = 18, EC_RES_INVALID_DATA_CRC = 19, EC_RES_DUP_UNAVAILABLE = 20,
};
```
v0 slice uses: `SUCCESS=0`, `INVALID_COMMAND=1`, `INVALID_PARAM=3`, `INVALID_CHECKSUM=7`.

## Commands for the milestone — VERBATIM (cros_ec_commands.h)
```c
#define EC_CMD_HELLO              0x0001   // NB: 0x0001, NOT 0x00 (corrected 2026-10-07)
#define EC_CMD_GET_VERSION        0x0002
#define EC_CMD_GET_BUILD_INFO     0x0004
#define EC_CMD_GET_PROTOCOL_INFO  0x000B

struct ec_params_hello   { uint32_t in_data;  } __ec_align4; // 4 bytes
struct ec_response_hello { uint32_t out_data; } __ec_align4; // out_data = in_data + 0x01020304

struct ec_response_get_protocol_info {   // 12 bytes
  uint32_t protocol_versions;
  uint16_t max_request_packet_size;
  uint16_t max_response_packet_size;
  uint32_t flags;
} __ec_align4;
```

## Thinnest-slice relevance (COMMANDS.md §0 milestone)
- **HELLO (0x0001)** = the proof-of-framing round-trip (send `in_data`, expect `in_data + 0x01020304`).
  Exercises the v3 header, checksum, and I²C clock-stretch with the simplest payload. FR2433 = EC slave,
  Pico = master.
- **GET_PROTOCOL_INFO (0x000B)** is what the real Linux `cros_ec` driver probes FIRST to bind v3 — so it's
  the next command after HELLO on the road to kernel-binding (the Pico slice doesn't need it).
- **GET_VERSION / GET_BUILD_INFO** = the sysfs/`ectool version` freebie (COMMANDS.md §0 decision).
