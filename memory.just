# Per-chip linker memory maps.  `just memory gen` | `just memory lint`
# Files: modules/deepsleep/memory-<chip>.x — RAM/ROM(FRAM) GENERATED from TI's device .ld (authoritative memory map);
# VECTORS is an msp430-rt/PAC property (link-ASSERTed). See scripts/gen-memory-x.sh + scripts/lint-memory-x.sh.

# regenerate modules/deepsleep/memory-<chip>.x from TI's msp430<chip>.ld (needs msp430-gcc/CCS; set MSP430_LD_DIR to override)
gen:
    scripts/gen-memory-x.sh

# validate the committed maps: VECTORS ends at 0x10000, PAC vector cross-check, fr2155==fr2355, + .ld drift if reachable
lint:
    scripts/lint-memory-x.sh
