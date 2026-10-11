# Datasheets & app notes — index

Hardware reference PDFs for the `stem` firmware. Firmware-**architecture** implementation notes (Samek,
Koopman, Ganssle, …) are indexed separately in [`../docs/references/firmware-architecture.md`](../docs/references/firmware-architecture.md).
TI doc numbers in parentheses; cite the number + page/section when you quote one (per the cite-sources rule).

## MSP430 — the parts we ship / develop on
| File | Doc | What it is / why it's here |
|---|---|---|
| `msp430fr2476.pdf` | FR2476 device DS | Primary dev target (dual-I²C). Pin tables (P3.2/P3.6 Stem UCB1), eCOMP ×1, memory. |
| `msp430fr2355.pdf` | FR2355 device DS (SLASEC4D) | Dev LaunchPad; FR2155 superset. P4.6/P4.7 Stem UCB1, eCOMP ×2. |
| `msp430fr2433.pdf` | FR2433 device DS (SLASE59) | Single-I²C passive-only node (P1.2/P1.3 UCB0). Port tables 6-17/6-18 (GPIO bank backing). |
| `msp430fr2422.pdf` | FR2422 device DS | Small FR24xx family member (reference for the low end). |
| `User's Guide slau445i.pdf` | **SLAU445i** — MSP430 FR2xx/4xx family UG | **THE authority** for eUSCI_B I²C (slave mode, §24), I²C-slave **LPM4 wake** (§24.3.10), the single shared eUSCI_B vector (§24.3.11), clocks, WDT, LPM. Cite this for all peripheral behaviour. |
| `slau680.pdf` / `slau680-msp-exp430fr2355-launchpad.pdf` | SLAU680 | FR2355 LaunchPad (MSP-EXP430FR2355) user guide — board wiring / headers (BoosterPack pin map for the bench). |
| `MSP430 family slau144j.pdf` | SLAU144 | **OLD x2xx family** UG — NOT FR2xx. Legacy reference only; prefer slau445i. |
| `MSP430™ FRAM Devices Bootloader slau550ab.pdf` | SLAU550 | FRAM BSL — the host-reflash path (OTA root-of-trust; see memory `msp-fw-ota-host-root-of-trust`). |
| `Designing With the ADC slaa890a.pdf` (`slaa890a.pdf`) | SLAA890 | ADC design (rail sense / analog triggers for Passive-mode wake). |
| `slaa689b.pdf` | SLAA689 | MSP430 low-power / design app note. |
| `Easily Solve Design Challenges … sszt426.pdf` (`sszt426.pdf`) | SSZT426 | MSP430 overview article. |

## I²C / I/O-expander (the Stem-slave register-map model)
| File | Doc | Why |
|---|---|---|
| `PCA9698.pdf` | NXP PCA9698 | The 40-bit I²C GPIO expander whose register layout our Stem slave emulates (bank addressing + command-pointer + auto-increment). Reserved-layout constraint — `DEEPSLEEP-EXTRACTION.md` §2. |
| `I2C I:O Expander slaa997.pdf` | SLAA997 | TI I²C I/O-expander app note. |
| `UART-to-I2C Bridge slaa908a.pdf` | SLAA908 | UART↔I²C bridging (relevant to the console/command-over-both-buses idea). |
| `USART-in-One-Wire-Mode-ApplicationNote-DS00002658.pdf` | DS00002658 | One-wire USART (Microchip) — reference. |

## Peripherals / other boards on/near the bench
| File | Why |
|---|---|
| `IS31FL3730_DS.pdf`, `IS31FL3730_EB.pdf` | Lumissil LED matrix driver (a Leaf-bus device); see `docs/IS31FL3730_EB.md`. |
| `nRF52840_PS_v1.7.pdf`, `DS-16306/16309-…nRF52840_USB_Dongle.pdf`, `nRF52840Mini_Graphical_Datasheet.pdf`, `nrf52840-breakout-mdbt50q-v10.pdf`, `sparkfun-nrf52832-breakout-schematic-v10.pdf` | nRF52 (BLE networking module path — separate product module, not the MSP430 deepsleep node). |
| `Raytac_MDBT50Q.pdf` | nRF52840 module (BLE). |
| `reg-defs-1.png`, `reg-defs-2.png` | Register-definition screenshots (source-note the origin before relying on these). |

> Maintenance: when you add a PDF here, add a row. When you cite one in a doc/comment, use the TI/NXP doc
> number + page so a future reader can re-find it even if the file is renamed.
