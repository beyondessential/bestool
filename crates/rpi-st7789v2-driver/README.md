# rpi-st7789v2-driver

A Raspberry Pi driver for the [WaveShare 1.69" 240×280 LCD module][wiki], which
uses Sitronix's ST7789V2 TFT controller over SPI.

[wiki]: https://www.waveshare.com/wiki/1.69inch_LCD_Module

The crate exposes two flavours of interface:

- A simple `SimpleImage` buffer (`Driver::image()` → `solid`, `pixel`, `print`)
  for the common case of "draw something and push the whole frame".
- The full [`embedded-graphics`] `DrawTarget` trait, so you can use any of the
  existing graphics primitives, fonts, and image decoders from that ecosystem.

[`embedded-graphics`]: https://docs.rs/embedded-graphics

## Use

```toml
[dependencies]
rpi-st7789v2-driver = "0.3"
```

```rust,no_run
use embedded_graphics::pixelcolor::Rgb565;
use rpi_st7789v2_driver::{Driver, Result};

fn main() -> Result<()> {
    let mut lcd = Driver::new(Default::default())?;
    lcd.init()?;
    lcd.probe_buffer_length()?;

    let mut image = lcd.image();
    image.solid(Rgb565::new(255, 0, 255));
    lcd.print((0, 0), &image)?;
    Ok(())
}
```

`DriverArgs::default()` matches WaveShare's [reference wiring][wiki]:
SPI0/CE0, DC on GPIO 25, RESET on GPIO 27, backlight on GPIO 18, 20 MHz SCK.
Override any field you have wired differently.

## Features

- `miette` — derive `miette::Diagnostic` on the crate's error type, for nicer
  CLI error reporting.

## Platform

Linux-only; uses [`rppal`] to talk to `/dev/spidev` and `/dev/gpiochip`.
Targeted at the Raspberry Pi family, but should work on any SBC `rppal`
supports.

[`rppal`]: https://docs.rs/rppal

## License

GPL-3.0-or-later.
