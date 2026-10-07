## I2C Expander API

> This is the **transport/register-map contract** (PCA9698 emulation, extended/debug window, and the
> role-addressing table). The **cross-MCU command model** layered on it — the `postcard` `Command`
> vocabulary over I²C/UART, common to all MCU modules — is in **[docs/COMMANDS.md](docs/COMMANDS.md)**.

The firmware supports the emulation of a PCA9698 expanded to support 8 pin ports.
In principle this supports 64 GPIO pins, but due to some being assigned to other functions
the maximum with be ~52 pins.
In addition the API will also support Voltage metering and I2C sensor proxying.


Special pins needed at a minimum are

- Stem SCL
- Stem SDA
- Stem MSG
- min. 1 interrupt output pins
- MSP RST
- MSP TEST

The MSP430 P1 - P8 ports are mapped directly to I/O Expander I2C ports.
For each port a mask must also be set to screen out pins that should be left alone.

Registers Input, Output and Config must directly drive off the native pin registers.
The Polarity & Mask Interrupt must be maintained by the firmware as there is no native support.

The open-drain interrupt output is activated when one of the port pins changes state and the pin is configured as an input. The interrupt is deactivated when the input returns to its previous state or the Input Port register is read (see Figure 12). A pin configured as an output cannot cause an interrupt. Since each 8-bit port is read independently, the interrupt caused by Port 0 will not be cleared by a read of Port 1 or the other way around.

The I/O Expander I2C API is enabled by calling `stemI2C(scl_pin, sda_pin, i2c_address)`.

### Stem address is ROLE-BASED (CANONICAL — verbatim, Henrik 2026-10-01)

> The address for MCUs on the stem bus should be determined on the role they play, so for the FR2476
> we need to give it an alt address value indicating that it isn't the real thing(or some other
> excuse). It would have the same firmware as FR2355. FR2433 would have an I/O expander role, which
> for now is a placeholder

**Derivation (subordinate to the block above).** The node's Stem I²C slave address is **chosen at boot
from the detected model → its role**, not fixed. A master (SoM / RP2040) can therefore tell *what it is
talking to* from the address alone — and a dev stand-in cannot masquerade as the production supervisor.
Values are within the PCA9698 strap group 0x20–0x27:

| Role | Model(s) | Stem addr | Note |
|---|---|---|---|
| **Supervisor** (production) | FR2355, FR2155 | **0x20** | the genuine sleep-supervisor; PCA9698 group base |
| **Supervisor — DEV stand-in** | FR2476 / FR2475 | **0x21** | SAME firmware as FR2355, but "not the real thing" → distinct address so it can't pose as the production supervisor |
| **I/O-expander** (placeholder) | FR2433 | **0x22** | plays the dumb-expander role; PLACEHOLDER value until the role is defined |
| **Power controller** | CYPM1111 (EZ-PD PMG1-S1) | **0x23** | the PD controller **is an MCU → a full Stem-bus node** (Henrik 2026-10-03): addressable like the others, **and it masters the Stem bus while the SoM is off** (the SoM-off-regime master). Requires the Stem bus to reach the power module (292) across the B2B. |

*(Address values 0x21/0x22/0x23 are a derivation — the canonical block fixes the scheme, not the exact
numbers; adjust here if the role map changes. Firmware: `Model::stem_addr()` in `prod/src/model.rs`.)*

**Masters on the Stem bus** (bus mastership, not a side-channel handshake): the **SoM/M7** while it is
alive; the **PD controller (CYPM1111)** while the SoM is off; the **RP2040** on the dev bench (SoM
stand-in). Only one master at a time — the SoM→off handoff passes mastership to the PD controller.


### From the PCA9698 Datasheet

### 7.3 Command Register / Part

Following the successful acknowledgement of the slave address + R/W bit, the bus master will send a byte to the PCA9698, which will be stored in the Command register.

The lowest 6 bits are used as a pointer to determine which register will be accessed.
Registers are divided into 2 categories: 5-bank register category, and 1-bank register category.

### 7.4 Register definitions

![Register definitions](./datasheets/reg-defs-1.png)
![Register definitions](./datasheets/reg-defs-2.png)

The standard register allocation has reserved entries for bank 5-8. The firmware will support these.

If the Auto-Increment flag is set (AI = 1), the 3 least significant bits are automatically incremented after a read or write. This allows the user to program and/or read the
8 register banks sequentially.

5-bank register category

* IP – Input registers
* OP – Output registers
* PI – Polarity Inversion registers
* IOC – I/O Configuration registers 
* MSK – Mask interrupt registers

1-bank register category

* OUTCONF - Output Structure Configuration register
* ALLBNK - All Bank Control register
* MODE - Mode Selection register

If more than 1 byte of data is written or read, previous data in the same register is overwritten independently of the value of AI.

#### 7.4.4 IOC0 to IOC4 - I/O Configuration registers
These registers configure the direction of the I/O pins.

    Cx[y] = 0: The corresponding port pin is an output. 
    Cx[y] = 1: The corresponding port pin is an input.

Where ‘x’ refers to the bank number (0 to 4); ‘y’ refers to the bit number (0 to 7).

Pins are configured as input by default.
Note that this is the inverse bit values of the MSP430 configuration registers.


#### 7.4.5 MSK0 to MSK4 - Mask interrupt registers

These registers mask the interrupt due to a change in the I/O pins configured as inputs. ‘x’ refers to the bank number (0 to 4); ‘y’ refers to the bit number (0 to 7).

    Mx[y] = 0: A level change at the I/O will generate an interrupt if IOx_y defined as input (Cx[y] in IOC register = 1).
    Mx[y] = 1: A level change in the input port will not generate an interrupt if IOx_y defined as input (Cx[y] in IOC register = 1).


### Voltage and more as Misc. Register

Additional registers are added within the PCA9698 command framework.

#### 2Bh 101011 - Init Code Binary

Allows setting the Init function called on startup/reset.
The code will be persisted.

#### 2Ch 101100 - VSOM voltage (read)

Reading this register must return a value reflecting the volate at the P1.6 pin(configurable).

#### 2Dh 101101 - CHARGE voltage (read)

Reading this register must return a value reflecting the volate at the P1.7 pin(configurable).

#### 2Eh 101110 - VSOM voltage threshold (16 bit write)

A message is sent if the threshold is passed.
Up to 3 x 16 bit thresholds can be set. LSB/MSB to be defined.

#### 2Fh 101111 - free

It could offer a way to set baud rate from master


### Extended Registers

#### 30h 110000 - Debug / identity / status window (0x30–0x3E)

See `prod/src/regmap.rs` (DBG_* constants): `0x30` iface magic (`0xD0`), `0x31/0x32` model id,
`0x33–0x36` build id, `0x37` status, `0x38` dev-count, `0x39` known-present bitmap, `0x3A` fault,
`0x3B–0x3D` fw version (maj/min/patch), `0x3E` MODE_CTRL (R: current mode; W: request Passive/Sensing).

#### 3Fh 111111 - DBG_CRC: per-sector FRAM CRC16 readout (integrity; ADR-0010)

A **windowed** register (the 6-bit command pointer leaves no room for the 8×u16 CRCs as separate
addresses). Computed in POST on **every** boot (incl. the silent production image) — `post::fram_crcs`.

- **WRITE `0x3F` = byte index 0..15** → moves the read cursor (`idx = 2·sector + {0=hi,1=lo}`).
- **READ `0x3F`** → returns the CRC byte at the cursor.
- So the M7 reads all 16 bytes as 16 × (write idx, read byte) and gets `[sec0_hi, sec0_lo, … sec7_hi,
  sec7_lo]` = per-sector CRC16-CCITT (poly 0x1021, init 0xFFFF) over the program-FRAM window.

**The MSP only reports; the M7 judges** (ADR-0010): the M7 holds the golden per-sector CRCs for the image
it flashed, compares, and reflashes a mismatched sector over BSL. A buggy/corrupt MSP verifier can only
cause a false "mismatch" (harmless reflash), never a false "OK". Reading it needs no UART (works on the
silent build); the console echoes the same values when `console`/`--debug` is built.

### Custom Registers

- Custom registers

