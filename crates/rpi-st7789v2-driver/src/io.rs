use std::{thread::sleep, time::Duration};

use rppal::{
	gpio::{Gpio, Level, OutputPin},
	spi::{Bus, Mode, SlaveSelect, Spi},
};
use tracing::{instrument, trace};

use super::{commands::*, error::Result, helpers::*};

/// Driver for the LCD display.
#[derive(Debug)]
pub struct Driver {
	pub(crate) spi: Spi,
	pub(crate) backlight: OutputPin,
	pub(crate) dc: OutputPin,
	pub(crate) reset: OutputPin,
	pub(crate) width: u16,
	pub(crate) height: u16,
	pub(crate) x_offset: u16,
	pub(crate) y_offset: u16,
	pub(crate) buffer: Vec<u8>,
	pub(crate) awake: bool,
}

/// Arguments to create a new LCD driver.
///
/// This is a struct to hold the arguments for the LCD driver: SPI port and frequency, GPIO pins.
///
/// It implements [`Default`] with the default wiring as per [Waveshare's documentation][wiki].
///
/// [wiki]: https://www.waveshare.com/wiki/1.69inch_LCD_Module#Hardware_Connection
#[derive(Debug, Clone)]
pub struct DriverArgs {
	/// SPI port to use.
	///
	/// Defaults to 0.
	pub spi: u8,

	/// GPIO pin number for the display's backlight control pin.
	///
	/// Defaults to 18.
	pub backlight: u8,

	/// GPIO pin number for the display's reset pin.
	///
	/// Defaults to 27.
	pub reset: u8,

	/// GPIO pin number for the display's data/command pin.
	///
	/// Defaults to 25.
	pub dc: u8,

	/// SPI CE number for the display's chip select pin.
	///
	/// Defaults to 0.
	pub ce: u8,

	/// SPI frequency in Hz.
	///
	/// Defaults to 20 MHz.
	pub frequency: u32,
}

impl Default for DriverArgs {
	fn default() -> Self {
		Self {
			spi: 0,
			backlight: 18,
			reset: 27,
			dc: 25,
			ce: 0,
			frequency: 20_000_000,
		}
	}
}

impl Driver {
	/// Connect to the LCD display I/O.
	///
	/// This performs the necessary setup for the GPIO and SPI pins, but doesn't touch the display
	/// otherwise. Usually you'll want to call `probe_buffer_length()` right after, then `init()`.
	#[instrument(level = "debug")]
	pub fn new(args: DriverArgs) -> Result<Self> {
		let gpio = Gpio::new()?;
		let backlight = gpio.get(args.backlight)?.into_output();
		let dc = gpio.get(args.dc)?.into_output();
		let reset = gpio.get(args.reset)?.into_output();

		let spi = Spi::new(
			match args.spi {
				0 => Bus::Spi0,
				1 => Bus::Spi1,
				2 => Bus::Spi2,
				3 => Bus::Spi3,
				4 => Bus::Spi4,
				5 => Bus::Spi5,
				6 => Bus::Spi6,
				_ => unreachable!("SPI bus number out of range"),
			},
			match args.ce {
				0 => SlaveSelect::Ss0,
				1 => SlaveSelect::Ss1,
				2 => SlaveSelect::Ss2,
				_ => unreachable!("SPI CE number out of range"),
			},
			args.frequency,
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
			awake: false,
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
	pub fn init(&mut self) -> Result<()> {
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

	/// Turn the display on or off.
	#[instrument(level = "trace", skip(self))]
	pub fn display(&mut self, on: bool) -> Result<()> {
		if on {
			self.command(Command::DisplayOn)
		} else {
			self.command(Command::DisplayOff)
		}
	}

	/// Send a command.
	#[instrument(level = "trace", skip(self, command))]
	pub fn command(&mut self, command: Command) -> Result<()> {
		self.set_dc(Level::Low);
		trace!(byte=%format!("{:02X?}", command as u8), "writing command byte to SPI");
		self.spi.write(&[command as u8])?;
		Ok(())
	}

	/// Write some data.
	#[instrument(level = "trace", skip(self, bytes))]
	pub fn write_data(&mut self, bytes: &[u8]) -> Result<()> {
		self.set_dc(Level::High);
		// trace!(length = bytes.len(), data=%format!("{bytes:02X?}"), "writing some bytes to SPI");
		trace!(length = bytes.len(), "writing some bytes to SPI");
		self.spi.write(bytes)?;
		Ok(())
	}

	/// Go to sleep.
	#[instrument(level = "trace", skip(self))]
	pub fn sleep(&mut self) -> Result<()> {
		if self.awake {
			self.command(Command::Sleep)?;
			sleep(Duration::from_millis(5));
			self.awake = false;
		}

		Ok(())
	}

	/// Wake up from sleep.
	#[instrument(level = "trace", skip(self))]
	pub fn wake(&mut self) -> Result<()> {
		if !self.awake {
			self.command(Command::WakeUp)?;
			sleep(Duration::from_millis(120));
			self.awake = true;
		}

		Ok(())
	}
}
