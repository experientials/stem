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

## Checksum rule — response VERBATIM, request TO-CONFIRM
**Response (quoted, cros_ec_i2c.c lines ~162–173):** sum every byte of `ec_host_response` + every payload
byte; the total must be **0 mod 256** or the driver errors.
```c
sum = 0;
for (i = 0; i < sizeof(struct ec_host_response); i++) sum += ((u8 *)ec_response)[i];
// ... memcpy payload ...
for (i = 0; i < ec_response->data_len; i++) sum += msg->data[i];
if (sum) /* -> EC_RES error */
```
**Request (DESCRIBED, not line-quoted — TO-CONFIRM at implementation):** the symmetric rule — the EC/driver
sets `ec_host_request.checksum` so that (header + payload) sums to 0 mod 256. Confirm against the request-
build lines in `cros_ec_pkt_xfer_i2c()` (~line 93+) before shipping the encoder.

## I²C transport wrapper — DESCRIBED from cros_ec_i2c.c (struct lines not quoted — TO-CONFIRM)
`EC_COMMAND_PROTOCOL_3 = 0xda` (quoted).

**Master → EC (request on the wire):**
| offset | field |
|---|---|
| 0 | `command_protocol` = `0xda` |
| 1..8 | `struct ec_host_request` (8 bytes) |
| 9.. | command payload (`data_len` bytes) |

**EC → master (response read back):**
| offset | field |
|---|---|
| 0 | `result` (EC_RES_*) |
| 1 | packet length (payload size) |
| 2..9 | `struct ec_host_response` (8 bytes) |
| 10.. | response payload (`data_len` bytes) |

> ⚠️ The exact `struct ec_host_request_i2c` / `ec_host_response_i2c` field declarations were *described* by
> the fetch, not line-quoted. The offsets above match the driver's buffer arithmetic (req byte0=0xda then
> header; resp byte0=result, byte1=len, then header). **Quote the two wrapper structs verbatim before the
> encoder/decoder is finalized** so our byte offsets are right by construction.

On I²C the **EC clock-stretches** until the response is staged (our MSP430 eUSCI slave does this natively,
slau445i §24.3) and returns filler on over-reads.

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
