use std::{io::Read, thread::sleep, time::Duration};

use rppal::{
	gpio::{Gpio, Level, OutputPin},
	spi::{Bus, Mode, SlaveSelect, Spi},
};
use tracing::{instrument, trace};

use super::LcdArgs;

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
}

impl LcdIo {
	/// Connect to the LCD display I/O.
	///
	/// This performs the necessary setup for the GPIO and SPI pins, but doesn't touch the display
	/// otherwise. Usually you'll want to call `wake()` right after, unless you know the display is
	/// currently not in deep sleep.
	#[instrument(level = "debug")]
	pub fn new(eink: &LcdArgs) -> Result<Self, LcdIoError> {
		let gpio = Gpio::new()?;
		let backlight = gpio.get(eink.backlight)?.into_output();
		let dc = gpio.get(eink.dc)?.into_output();
		let reset = gpio.get(eink.reset)?.into_output();

		let spi = Spi::new(
			match eink.spi {
				0 => Bus::Spi0,
				1 => Bus::Spi1,
				2 => Bus::Spi2,
				3 => Bus::Spi3,
				4 => Bus::Spi4,
				5 => Bus::Spi5,
				6 => Bus::Spi6,
				_ => unreachable!("SPI bus number out of range"),
			},
			match eink.ce {
				0 => SlaveSelect::Ss0,
				1 => SlaveSelect::Ss1,
				2 => SlaveSelect::Ss2,
				_ => unreachable!("SPI CE number out of range"),
			},
			eink.frequency,
			Mode::Mode0,
		)?;

		Ok(Self {
			spi,
			backlight,
			dc,
			reset,
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
	/// Init sequence is from both <https://github.com/marko-pi/parallel/blob/main/SSD1680.py> and
	/// <https://github.com/WeActStudio/WeActStudio.EpaperModule/blob/master/Example/EpaperModuleTest_AT32F403A/Epaper/epaper.c>.
	#[instrument(level = "trace", skip(self))]
	pub fn init(&mut self) -> Result<(), LcdIoError> {
		self.set_reset(Level::High);
		sleep(Duration::from_millis(100));
		self.set_reset(Level::Low);
		sleep(Duration::from_millis(100));
		self.set_reset(Level::High);

		todo!();
		Ok(())
	}

	/// Send a command with associated data.
	#[instrument(level = "trace", skip(self, data))]
	pub fn command_with_data(
		&mut self,
		command: Command,
		mut data: impl Read,
	) -> Result<(), LcdIoError> {
		self.set_dc(Level::Low);
		self.spi.write(&[command as u8])?;
		self.set_dc(Level::High);

		let mut buf = Vec::new();
		data.read_to_end(&mut buf)?;
		trace!(length = buf.len(), data=%format!("{buf:02X?}"), "writing some bytes to SPI");
		self.spi.write(&buf)?;

		self.command(Command::Nop)?; // signal end of command

		Ok(())
	}

	/// Send a data-less command.
	#[instrument(level = "trace", skip(self))]
	pub fn command(&mut self, command: Command) -> Result<(), LcdIoError> {
		self.set_dc(Level::Low);
		self.spi.write(&[command as u8])?;
		self.set_dc(Level::High);
		Ok(())
	}
}

/// Eink display commands
///
/// This is a subset of the SSD1681 command set, just enough to drive the display.
/// Descriptions are derived from usage and [the datasheet for the SSD1681 chip][SSD1681].
///
/// The WeAct SPI interface combines MOSI and MISO, which makes write commands dangerous to use, so
/// these are deliberately not included. See <https://www.pinteric.com/displays.html#ssd>.
///
/// [SSD1681]: https://github.com/WeActStudio/WeActStudio.EpaperModule/blob/master/Doc/SSD1681.pdf
#[derive(Debug, Copy, Clone)]
#[repr(u8)]
pub enum Command {
	/// Select the driver output mode.
	///
	/// This is used in the init sequence.
	DriverOutputControl = 0x01,

	/// Select the border waveform.
	///
	/// This is used in the init sequence.
	BorderWaveformControl = 0x3C,

	/// Select the display update mode.
	///
	/// This is used in the init sequence.
	DisplayUpdateControl = 0x21,

	/// Select the temperature sensor.
	///
	/// This is used in the init sequence.
	TemperatureSensorControl = 0x18,

	/// Select the data entry mode.
	///
	/// This is used in the init sequence.
	DataEntryMode = 0x11,

	/// Select X address range.
	///
	/// This is used in the init sequence.
	XAddressRange = 0x44,

	/// Select Y address range.
	///
	/// This is used in the init sequence.
	YAddressRange = 0x45,

	/// Set the initial X address.
	///
	/// This is used in the init sequence.
	XAddressCounter = 0x4E,

	/// Set the initial Y address.
	///
	/// This is used in the init sequence.
	YAddressCounter = 0x4F,

	/// Enter deep sleep mode.
	///
	/// This will keep BUSY high, so `wait_for_idle()` will block forever if called. To wake the
	/// display, hardware reset is required (with `hw_reset()` method).
	DeepSleep = 0x10,

	/// Software reset.
	///
	/// This is used in the init sequence.
	SoftwareReset = 0x12,

	/// Write black/white data to RAM.
	///
	/// This is followed by a write of _bits_ (one bit per pixel): 0 for black, 1 for white.
	LoadBlackWhite = 0x24,

	/// Write red data to RAM.
	///
	/// This is followed by a write of _bits_ (one bit per pixel): 0 for "transparent", 1 for red.
	///
	/// "Transparent" means "use the black/white data instead of the red data for this pixel.
	LoadRed = 0x26,

	/// Write to the LUT register.
	#[allow(dead_code)] // unused for now
	LoadLut = 0x32,

	/// Set display update sequence options.
	///
	/// This must be followed by a [`DisplaySequence`] byte.
	ConfigureSequence = 0x22,

	/// Start the display update sequence.
	Update = 0x20,

	/// Noop.
	///
	/// Used to signal the end of a write.
	Nop = 0x7F,
}

bitflags::bitflags! {
	/// Represents which operations in the update sequence should be performed.
	///
	/// The update sequence is a series of operations that are performed to update the display. The
	/// operations are always performed in the same order, but they can be skipped by setting their
	/// corresponding bit to zero.
	///
	/// The exception is bit 5, `PARTIAL_MODE`, which is a toggle: if it's set, the display will
	/// update in partial mode, otherwise it will update in full mode.
	#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
	pub struct DisplaySequence: u8 {
		const ENABLE_CLOCK     = 0b10000000;
		const ENABLE_ANALOG    = 0b01000000;
		const LOAD_TEMPERATURE = 0b00100000;
		const LOAD_DEFAULT_LUT = 0b00010000;
		const PARTIAL_MODE     = 0b00001000;
		const DISPLAY          = 0b00000100;
		const DISABLE_ANALOG   = 0b00000010;
		const DISABLE_CLOCK    = 0b00000001;

		/// Power on sequence
		const POWER_ON =
			Self::ENABLE_CLOCK.bits() |
			Self::ENABLE_ANALOG.bits() |
			Self::LOAD_TEMPERATURE.bits() |
			Self::LOAD_DEFAULT_LUT.bits() |
			Self::PARTIAL_MODE.bits();

		/// Power off sequence
		const POWER_OFF =
			Self::ENABLE_CLOCK.bits() |
			Self::DISABLE_ANALOG.bits() |
			Self::DISABLE_CLOCK.bits();
	}
}

bitflags::bitflags! {
	/// Represents the possible directions of data entry for the X and Y axes.
	#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
	pub struct DataEntryMode: u8 {
		const X_INCREMENT     = 0b01;
		const Y_INCREMENT     = 0b10;
	}
}
