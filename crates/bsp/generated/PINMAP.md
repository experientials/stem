<!-- GENERATED — DO NOT EDIT. Source: crates/bsp/connections.toml. Regenerate: `just bsp docs` (scripts/gen-bsp-docs.py). -->

# MSP430 BSP — pin map (bob-929)

> Generated from [`crates/bsp/connections.toml`](../connections.toml) by `scripts/gen-bsp-docs.py`. Edit the TOML, then run `just bsp docs` — never edit this file by hand.

**Chip:** `MSP430FR2476` · **package:** RHA VQFN-40? · **datasheet:** datasheets/msp430fr2476.pdf

16 connections. `status`: active = wired/verified · planned = product-intent, confirm before activating · reserved = programming/debug.

## Active (6)

| ID | Signal | Pin | Function | Module | Dir | Net |
|---|---|---|---|---|---|---|
| i2c_sda | Sensor/LED I2C SDA | P1.2 | UCB0SDA | eUSCI_B0 | bidir | IS31FL3730 @0x61 + APDS-9960 @0x39 + MC6470 eCompass @0x4C(accel)/0x0C(mag) (shared sensor bus) |
| i2c_scl | Sensor/LED I2C SCL | P1.3 | UCB0SCL | eUSCI_B0 | bidir | IS31FL3730 @0x61 + APDS-9960 @0x39 + MC6470 eCompass @0x4C(accel)/0x0C(mag) (shared sensor bus) |
| is31_sdb | IS31FL3730 SDB (shutdown, active-low) | P2.5 | GPIO | PORT | out | IS31FL3730 SDB — drive HIGH to enable (examples/i2c-is31) |
| rcwl_out | RCWL-0516 microwave radar motion OUT | P2.4 | GPIO | PORT | in | RCWL-0516 OUT -> 2 kohm series -> P2.4. Digital: idles LOW, push-pull HIGH (~3.3 V) ~2 s per trigger, retriggerable. Sensor VIN=5 V, GND shared; 3V3 (regulator tap, an OUTPUT) and CDS pins left open. |
| uart_tx | Backchannel UART TX | P1.4 | UCA0TXD | eUSCI_A0 | out | eZ-FET -> /dev/cu.usbmodem* (9600 8N1) |
| uart_rx | Backchannel UART RX | P1.5 | UCA0RXD | eUSCI_A0 | in | eZ-FET backchannel |

## Planned (7)

| ID | Signal | Pin | Function | Module | Dir | Net |
|---|---|---|---|---|---|---|
| vsom_adc | VSOM (LiPo) voltage sense | P1.6 | A6? | ADC | analog | LiPo battery (nRF52 side), 2.8-5.0 V |
| charge_adc | CHARGE (VBUS) voltage sense | P1.7 | A7? | ADC | analog | VBUS (nRF52 side) |
| stem_sda | Stem I2C SDA (MCU = SLAVE; SoM/RP2040 masters) | P3.2 | UCB1SDA | eUSCI_B1 | bidir | Stem bus to SoM/other MCUs — the PCA9698 register + debug surface (prod/src/regmap.rs) |
| stem_scl | Stem I2C SCL (MCU = SLAVE) | P3.6 | UCB1SCL | eUSCI_B1 | bidir | Stem bus |
| stem_msg | Stem MSG (1-Wire-over-UART) | P5.1/P5.2? | UCA0RXD/UCA0TXD | eUSCI_A0 | bidir | shared 1-wire coordination line |
| sensor_int | Sensor interrupt in | TBD | GPIO | PORT | in | sensor INT (e.g. APDS-9960 / IMU) |
| som_wake | SoM wake — PMIC_ON_REQ OP-bank bit (the MSP IS the I/O expander) | TBD | GPIO | PORT | out | MSP GPIO / regmap OP-bank bit → PCA9450 PMIC_ON_REQ (assert HIGH to power-on the SoM). MSP replaces the old PCA9555/FR2032 expanders. |

## Reserved (3)

| ID | Signal | Pin | Function | Module | Dir | Net |
|---|---|---|---|---|---|---|
| msp_rst | Reset / SBW | RST/NMI/SBWTDIO | RESET | SYS | bidir | eZ-FET SBW |
| msp_test | Test / SBW clock | TEST/SBWTCK | TEST | SYS | in | eZ-FET SBW |
| s1_button | S1 user button (diag skip / mode-cycle) | P4.0 | GPIO | PORT | in | P4.0_S1 — active-low, 47 kohm external pull-up to 3V3 (FR2476 LaunchPad). |
