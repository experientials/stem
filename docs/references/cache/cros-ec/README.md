# Cached cros_ec public specs (full source, not extracts)

Full upstream source files cached locally so protocol details are offline-checkable against the
**complete** spec, not just the quotes in `../../cros-ec-wire-framing.md`. Per `save-sources-locally`
rule: grep these files directly for anything not in the extract.

| File | Upstream path | Fetched |
|---|---|---|
| `cros_ec_commands.h` | `torvalds/linux:include/linux/platform_data/cros_ec_commands.h` | 2026-10-07 |
| `cros_ec_i2c.c` | `torvalds/linux:drivers/platform/chrome/cros_ec_i2c.c` | 2026-10-07 |
| `cros_ec_proto.c` | `torvalds/linux:drivers/platform/chrome/cros_ec_proto.c` | 2026-10-07 |

Source: `https://raw.githubusercontent.com/torvalds/linux/master/<path>` (the kernel's mirror of the
ChromeOS `platform/ec` headers — what Linux actually builds against).

**Snapshot of `master` on the fetch date** — line numbers in the extract doc track THIS snapshot
(verified: HELLO @949, `struct ec_host_request` @755 / `ec_host_response` @775, `EC_RES_SUCCESS` @467 in
`cros_ec_commands.h`; wrapper structs @25/@39 in `cros_ec_i2c.c`; `request->checksum = -csum` @87 in
`cros_ec_proto.c`). If re-fetched, line numbers may drift — the file content is the authority, grep by
symbol not by line.

Re-fetch: `curl -fsSL -A "Mozilla/5.0" <raw-url> -o <file>`. GPL-2.0 source, cached verbatim for reference.
