/* COMBINED single image (fr2xxx) — ONE memory map valid on BOTH FR2476 and FR2355/FR2155 (their common
 * intersection). NOT generated from a single TI .ld: it is the hand-reconciled intersection of
 * memory-fr2476.x and memory-fr2355.x (both derived verbatim from TI's device linker scripts). Emitted
 * to OUT_DIR as memory.x by prod/build.rs. See docs/spikes/combined-image.md + STEM-DIRECTION.md.
 *
 * RAM     : 4 KB — the SMALLER of the two (FR2355 = 4 KB, FR2476 = 8 KB). Taking the intersection keeps
 *           the one image valid on both parts (the FR2476's upper 4 KB simply goes unused).
 * ROM     : 0x8000/0x7F80 — program FRAM main window, END 0xFF7F. IDENTICAL on both parts (both TI .ld).
 * VECTORS : 0xFF80/0x0080 — the FR2476-shaped 64-word (0x80) SUPERSET table, which COVERS BOTH chips:
 *           FR2476 EUSCI_B1 @ 0xFFDA (word idx 45) AND FR2355 EUSCI_B1 @ 0xFFDE (word idx 47). The
 *           combined PAC (msp430fr2xxx-dual, `rt`) emits a `__INTERRUPTS` placing the EUSCI_B1 handler at
 *           BOTH slots, so the one image's wake ISR fires on whichever part it boots. Table ends at
 *           0x10000 (msp430-rt link.x ASSERT), reset @ 0xFFFE. HW-proven (S2b.1, spike/combined-mech).
 */
MEMORY
{
  RAM     : ORIGIN = 0x2000, LENGTH = 0x1000   /* 4 KB SRAM (FR2355 — the common floor) */
  ROM     : ORIGIN = 0x8000, LENGTH = 0x7F80   /* program FRAM (main window), END 0xFF7F */
  VECTORS : ORIGIN = 0xFF80, LENGTH = 0x0080   /* FR2476-shaped superset: covers EUSCI_B1 @ 0xFFDA + 0xFFDE */
}

/* Checksum region (single source of truth): prod/src/post.rs reads these as extern symbols to CRC the
   program FRAM per sector — DESIGN.md console log. */
PROVIDE(__fram_start = ORIGIN(ROM));
PROVIDE(__fram_size  = LENGTH(ROM));
