#![cfg(target_os = "linux")]

//! A Raspberry Pi driver for the ST7789V2-based WaveShare 1.69" LCD display.
//!
//! This crate provides a high-level interface for controlling a [WaveShare 1.69" LCD display][lcd]
//! connected to a Raspberry Pi over SPI.
//!
//! It implements both a simple "image"-based interface and [`embedded_graphics`]' traits.
//!
//! [lcd]: https://www.waveshare.com/wiki/1.69inch_LCD_Module
//!
//! # Example
//!
//! ```no_run
//! # use embedded_graphics::pixelcolor::Rgb565;
//! # use rpi_st7789v2_driver::{Driver, Result};
//! # fn main() -> Result<()> {
//! let mut lcd = Driver::new(Default::default())?;
//! lcd.init()?;
//! lcd.probe_buffer_length()?;
//!
//! let mut image = lcd.image();
//! image.solid(Rgb565::new(255, 0, 255));
//! lcd.print((0, 0), &image)?;
//! # Ok(()) }
//! ```

#[doc(inline)]
pub use commands::Command;

#[doc(inline)]
pub use error::{Error, Result};

#[doc(inline)]
pub use helpers::*;

#[doc(inline)]
pub use io::*;

#[doc(inline)]
pub use simple::*;

mod buffer;
mod commands;
mod error;
mod graphics;
mod helpers;
mod io;
mod simple;
