/* SPIKE combined-mech — ONE memory map for BOTH FR2476 and FR2355 (their common intersection).
 *
 * RAM     : 0x2000/0x1000  — 4 KB, the SMALLER of the two (FR2355 has 4 KB, FR2476 has 8 KB). Taking
 *                            the intersection keeps the one image valid on both parts.
 * ROM     : 0x8000/0x7F80  — program FRAM main window, END 0xFF7F. IDENTICAL on both parts (from the TI
 *                            .ld files; see prod/memory-fr2476.x + prod/memory-fr2355.x).
 * VECTORS : 0xFF80/0x0080  — a 0xFF80-based 64-word (0x80) SUPERSET table that covers BOTH chips:
 *                            FR2476 EUSCI_B1 @ 0xFFDA (word idx 45), FR2355 EUSCI_B1 @ 0xFFDE (word idx 47).
 *                            Our custom __INTERRUPTS (63 words) + msp430-rt's __RESET_VECTOR (1 word,
 *                            reset @ 0xFFFE) = 64 words, so the table ends exactly at 0x10000 — the
 *                            msp430-rt link.x ASSERT (ADDR+SIZEOF == __VECTORS_END_ADDR = 0x10000).
 */
MEMORY
{
  RAM     : ORIGIN = 0x2000, LENGTH = 0x1000   /* 4 KB SRAM (FR2355 — the common floor) */
  ROM     : ORIGIN = 0x8000, LENGTH = 0x7F80   /* program FRAM (main window), END 0xFF7F */
  VECTORS : ORIGIN = 0xFF80, LENGTH = 0x0080   /* 64 words: 63 custom __INTERRUPTS + 1 reset */
}
