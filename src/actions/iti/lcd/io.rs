use std::{thread::sleep, time::Duration};

use embedded_graphics::{
	draw_target::DrawTarget,
	geometry::{Dimensions, Point, Size},
	pixelcolor::{
		raw::{RawData, RawU16},
		Rgb565,
	},
	primitives::Rectangle,
	Pixel,
};
use itertools::Itertools;
use rppal::{
	gpio::{Gpio, Level, OutputPin},
	spi::{Bus, Mode, SlaveSelect, Spi},
};
use tracing::{instrument, trace};

use super::{commands::*, helpers::*, simple::*, LcdArgs};

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
#[error("I/O error")]
pub enum LcdIoError {
	#[diagnostic(help("GPIO error, check the pin numbers"))]
	Gpio(#[from] rppal::gpio::Error),

	#[diagnostic(help("SPI error, check settings or increase spidev.bufsiz"))]
	Spi(#[from] rppal::spi::Error),

	#[diagnostic(help("local (non-SPI/GPIO) I/O error"))]
	Io(#[from] std::io::Error),
}

#[derive(Debug)]
pub struct LcdIo {
	spi: Spi,
	backlight: OutputPin,
	dc: OutputPin,
	reset: OutputPin,
	width: u16,
	height: u16,
	pub(crate) x_offset: u16,
	pub(crate) y_offset: u16,
	buffer: Vec<u8>,
}

impl LcdIo {
	/// Connect to the LCD display I/O.
	///
	/// This performs the necessary setup for the GPIO and SPI pins, but doesn't touch the display
	/// otherwise. Usually you'll want to call `wake()` right after, unless you know the display is
	/// currently not in deep sleep.
	#[instrument(level = "debug")]
	pub fn new(lcd: &LcdArgs) -> Result<Self, LcdIoError> {
		let gpio = Gpio::new()?;
		let backlight = gpio.get(lcd.backlight)?.into_output();
		let dc = gpio.get(lcd.dc)?.into_output();
		let reset = gpio.get(lcd.reset)?.into_output();

		let spi = Spi::new(
			match lcd.spi {
				0 => Bus::Spi0,
				1 => Bus::Spi1,
				2 => Bus::Spi2,
				3 => Bus::Spi3,
				4 => Bus::Spi4,
				5 => Bus::Spi5,
				6 => Bus::Spi6,
				_ => unreachable!("SPI bus number out of range"),
			},
			match lcd.ce {
				0 => SlaveSelect::Ss0,
				1 => SlaveSelect::Ss1,
				2 => SlaveSelect::Ss2,
				_ => unreachable!("SPI CE number out of range"),
			},
			lcd.frequency,
			Mode::Mode0,
		)?;

		Ok(Self {
			spi,
			backlight,
			dc,
			reset,
			width: 280,
			height: 240,
			x_offset: 20,
			y_offset: 0,
			buffer: Vec::with_capacity(4092),
		})
	}

	#[instrument(level = "trace", skip(self))]
	fn set_dc(&mut self, level: Level) {
		self.dc.write(level);
	}

	#[instrument(level = "trace", skip(self))]
	fn set_reset(&mut self, level: Level) {
		self.reset.write(level);
	}

	/// Perform the init sequence.
	///
	/// Init sequence is from the Waveshare driver, simplified a bit to avoid redundant operations.
	#[instrument(level = "debug", skip(self))]
	pub fn init(&mut self) -> Result<(), LcdIoError> {
		// reset
		self.set_reset(Level::High);
		sleep(Duration::from_millis(20));
		self.set_reset(Level::Low);
		sleep(Duration::from_millis(20));
		self.set_reset(Level::High);
		sleep(Duration::from_millis(120)); // wait past cancel period

		self.spi.write(&vec![0xaa; 3])?;
		self.command(Command::MemoryAccessControl)?;
		self.write_data(&[MemoryAccessControl::default().into()])?;

		self.command(Command::InterfacePixelFormat)?;
		self.write_data(&[COLMOD_RGB_65K << 4 | COLMOD_16BPP])?;

		self.command(Command::PorchSettings)?;
		self.write_data(&[0x0B, 0x0B, 0, 0b0011 << 4 | 0b0011, 0b0011 << 4 | 0b0101])?;

		self.command(Command::GateVoltages)?;
		self.write_data(&[gate_voltages(12540, 7670)])?;

		self.command(Command::VcomSetting)?;
		self.write_data(&[0x35])?; // 1.425V

		self.command(Command::LcmControl)?;
		self.write_data(&[0b0100110])?;

		self.command(Command::VdvVrhEnable)?;
		self.write_data(&[0x01, 0xFF])?;

		self.command(Command::VrhSetting)?;
		self.write_data(&[0x0D])?;

		self.command(Command::VdvSetting)?;
		self.write_data(&[0x20])?;

		self.command(Command::FrameRateControl)?;
		self.write_data(&[0b000 << 5 | frame_rate(53)])?;

		self.command(Command::PowerControl1)?;
		self.write_data(&power_control1(68, 48, 23))?;

		self.command(Command::PowerControl2)?;
		self.write_data(&[power_control2(68, 48, 23)])?;

		self.command(Command::PositiveGammaControl)?;
		self.write_data(&[
			// from the Waveshare driver
			0xF0, 0x06, 0x0B, 0x0A, 0x09, 0x26, 0x29, 0x33, 0x41, 0x18, 0x16, 0x15, 0x29, 0x2D,
		])?;

		self.command(Command::NegativeGammaControl)?;
		self.write_data(&[
			// from the Waveshare driver
			0xF0, 0x04, 0x08, 0x08, 0x07, 0x03, 0x28, 0x32, 0x40, 0x3B, 0x19, 0x18, 0x2A, 0x2E,
		])?;

		self.command(Command::GateControl)?;
		self.write_data(&gate_control(304, 0, GateFlags::default()))?;

		self.command(Command::InversionOn)?;

		self.backlight(true);
		self.wake()?;
		self.command(Command::DisplayOn)?;

		Ok(())
	}

	/// Turn the backlight on or off.
	#[instrument(level = "trace", skip(self))]
	pub fn backlight(&mut self, on: bool) {
		self.backlight
			.write(if on { Level::High } else { Level::Low });
	}

	/// Send a command.
	#[instrument(level = "trace", skip(self))]
	pub fn command(&mut self, command: Command) -> Result<(), LcdIoError> {
		self.set_dc(Level::Low);
		self.spi.write(&[command as u8])?;
		Ok(())
	}

	/// Write some data.
	#[instrument(level = "trace", skip(self))]
	pub fn write_data(&mut self, bytes: &[u8]) -> Result<(), LcdIoError> {
		self.set_dc(Level::High);
		trace!(length = bytes.len(), data=%format!("{bytes:02X?}"), "writing some bytes to SPI");
		self.spi.write(bytes)?;
		Ok(())
	}

	/// Go to sleep.
	#[instrument(level = "trace", skip(self))]
	pub fn sleep(&mut self) -> Result<(), LcdIoError> {
		self.command(Command::Sleep)?;
		sleep(Duration::from_millis(5));
		Ok(())
	}

	/// Wake up from sleep.
	#[instrument(level = "trace", skip(self))]
	pub fn wake(&mut self) -> Result<(), LcdIoError> {
		self.command(Command::WakeUp)?;
		sleep(Duration::from_millis(120));
		Ok(())
	}

	/// Get a new image buffer sized for the screen.
	pub fn image(&self) -> SimpleImage {
		SimpleImage::new(self.width, self.height)
	}

	/// Set the area of the screen to draw to.
	pub(crate) fn set_window(
		&mut self,
		start: (u16, u16),
		end: (u16, u16),
	) -> Result<(), LcdIoError> {
		if (start.0 > end.0) || (start.1 > end.1) {
			return Err(LcdIoError::Io(std::io::Error::new(
				std::io::ErrorKind::InvalidInput,
				"window start must be equal or before end",
			)));
		}

		if (self.width < end.0) || (self.height < end.1) {
			return Err(LcdIoError::Io(std::io::Error::new(
				std::io::ErrorKind::InvalidInput,
				"window exceeds screen size",
			)));
		}

		// if this doesn't work, let's have another look at the c driver code
		self.command(Command::ColumnAddressSet)?;
		self.write_data(&(self.x_offset + start.0).to_be_bytes())?;
		self.write_data(&(self.x_offset + end.0).to_be_bytes())?;
		self.command(Command::RowAddressSet)?;
		self.write_data(&(self.y_offset + start.1).to_be_bytes())?;
		self.write_data(&(self.y_offset + end.1).to_be_bytes())?;

		Ok(())
	}

	/// Probe how many bytes we can send at once.
	pub fn probe_buffer_length(&mut self) -> Result<(), LcdIoError> {
		self.flush_buffer()?;

		let mut n = 2048;

		// increase exponentially until we hit the limit
		loop {
			let data = vec![0; n];
			let result = self.write_data(&data);
			self.command(Command::Nop)?;
			n *= 2;
			match result {
				Ok(_) => {}
				Err(LcdIoError::Spi(rppal::spi::Error::Io(_))) => {
					break;
				}
				Err(e) => {
					return Err(e);
				}
			}
		}

		// decrease linearly until we can send again
		loop {
			n -= 64;
			let data = vec![0; n];
			let result = self.write_data(&data);
			self.command(Command::Nop)?;
			match result {
				Ok(_) => {
					break;
				}
				Err(LcdIoError::Spi(rppal::spi::Error::Io(_))) => {
					continue;
				}
				Err(e) => {
					return Err(e);
				}
			}
		}

		tracing::debug!(n, "probed max usable spi buffer length");
		self.buffer = Vec::with_capacity(n);
		Ok(())
	}

	pub fn clear_buffer(&mut self) {
		self.buffer.clear();
	}

	pub fn flush_buffer(&mut self) -> Result<(), LcdIoError> {
		if self.buffer.is_empty() {
			return Ok(());
		}

		let new = Vec::with_capacity(self.buffer.capacity());
		let buf = std::mem::replace(&mut self.buffer, new);
		self.write_data(&buf)?;
		Ok(())
	}

	pub fn write_data_buffered(&mut self, bytes: &[u8]) -> Result<(), LcdIoError> {
		let remaining = self.buffer.capacity() - self.buffer.len();
		if bytes.len() > remaining {
			self.flush_buffer()?;
		}

		for chunk in bytes.chunks(self.buffer.capacity()) {
			self.buffer.extend_from_slice(chunk);
			if self.buffer.len() == self.buffer.capacity() {
				self.flush_buffer()?;
			}
		}

		Ok(())
	}

	/// Write an image to the screen, buffered.
	#[instrument(level = "trace", skip(self))]
	pub fn print(&mut self, image: &SimpleImage) -> Result<(), LcdIoError> {
		self.set_window((0, 0), (image.width, image.height))?;
		self.command(Command::MemoryWrite)?;
		self.clear_buffer();
		for line in &image.data().chunks((image.width as usize) * 2) {
			self.write_data_buffered(&line.collect::<Vec<u8>>())?;
		}
		self.flush_buffer()?;
		self.command(Command::Nop)?;
		Ok(())
	}

	/// Write a pixel to the screen, unbuffered.
	#[instrument(level = "trace", skip(self))]
	pub fn pixel(&mut self, x: u16, y: u16, colour: Rgb565) -> Result<(), LcdIoError> {
		if x >= self.width || y >= self.height {
			return Err(LcdIoError::Io(std::io::Error::new(
				std::io::ErrorKind::InvalidInput,
				"pixel out of bounds",
			)));
		}

		self.set_window((x, y), (x, y))?;
		self.command(Command::MemoryWrite)?;
		self.write_data(&RawU16::from(colour).into_inner().to_be_bytes())?;
		self.command(Command::Nop)?;
		Ok(())
	}
}

impl Dimensions for LcdIo {
	fn bounding_box(&self) -> Rectangle {
		Rectangle::new(
			Point::new(0, 0),
			Size::new(self.width.into(), self.height.into()),
		)
	}
}

impl DrawTarget for LcdIo {
	type Color = Rgb565;
	type Error = LcdIoError;

	fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
	where
		I: IntoIterator<Item = Pixel<Self::Color>>,
	{
		for Pixel(coord, color) in pixels.into_iter() {
			let Ok(x) = u16::try_from(coord.x) else {
				continue;
			};
			let Ok(y) = u16::try_from(coord.y) else {
				continue;
			};

			if x >= self.width || y >= self.height {
				continue;
			}

			self.pixel(x, y, color)?;
		}

		Ok(())
	}

	// TODO: implement other methods to accelerate rendering
}
