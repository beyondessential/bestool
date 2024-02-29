use std::{io::Read, thread::sleep, time::Duration};

use rppal::{
	gpio::{Gpio, InputPin, Level, OutputPin},
	spi::{Bus, Mode, SlaveSelect, Spi},
};
use tracing::{instrument, trace};

use super::{chip::Chip, pixels::Pixels, EinkArgs};

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
#[error("I/O error")]
pub enum EinkIoError {
	#[diagnostic(help("GPIO error, check the pin numbers"))]
	Gpio(#[from] rppal::gpio::Error),

	#[diagnostic(help("SPI error, check settings or increase spidev.bufsiz"))]
	Spi(#[from] rppal::spi::Error),

	#[diagnostic(help("local (non-SPI/GPIO) I/O error"))]
	Io(#[from] std::io::Error),
}

#[derive(Debug)]
pub struct EinkIo {
	spi: Spi,
	busy: InputPin,
	dc: OutputPin,
	reset: OutputPin,
	chip: Chip,
	sleeping: bool,
}

impl EinkIo {
	/// Connect to the Eink display I/O.
	///
	/// This performs the necessary setup for the GPIO and SPI pins, but doesn't touch the display
	/// otherwise. Usually you'll want to call `wake()` right after, unless you know the display is
	/// currently not in deep sleep.
	#[instrument(level = "debug")]
	pub fn new(eink: &EinkArgs) -> Result<Self, EinkIoError> {
		let gpio = Gpio::new()?;
		let busy = gpio.get(eink.busy)?.into_input_pullup();
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
			busy,
			dc,
			reset,
			chip: eink.chip,
			sleeping: true,
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

	/// Wake the display from deep sleep.
	#[instrument(level = "trace", skip(self))]
	pub fn wake(&mut self) {
		self.set_reset(Level::High);
		sleep(Duration::from_millis(50));
		self.set_reset(Level::Low);
		sleep(Duration::from_millis(50));
		self.set_reset(Level::High);
		self.sleeping = false;
	}

	/// Perform the init sequence.
	///
	/// Init sequence is from both <https://github.com/marko-pi/parallel/blob/main/SSD1680.py> and
	/// <https://github.com/WeActStudio/WeActStudio.EpaperModule/blob/master/Example/EpaperModuleTest_AT32F403A/Epaper/epaper.c>.
	#[instrument(level = "trace", skip(self))]
	pub fn init(&mut self) -> Result<(), EinkIoError> {
		if self.sleeping {
			self.wake();
		}

		self.wait_for_idle();
		self.command(Command::SoftwareReset)?;
		sleep(Duration::from_millis(10));
		self.wait_for_idle();

		self.command_with_data(
			Command::DriverOutputControl,
			&self.chip.driver_output_control()[..],
		)?;

		self.command_with_data(
			Command::DataEntryMode,
			// top to bottom, left to right
			&[(DataEntryMode::X_INCREMENT | DataEntryMode::Y_INCREMENT).bits()][..],
		)?;
		self.command_with_data(Command::XAddressRange, &self.chip.x_range()[..])?;
		self.command_with_data(Command::YAddressRange, &self.chip.y_range()[..])?;

		// TODO: figure out what 0x05 means
		self.command_with_data(Command::BorderWaveformControl, &[0x05][..])?;

		if self.chip == Chip::SSD1680 {
			// 0x00 for the first byte sets ram usage to "normal" (no fill, no inversion, default)
			// 0b1 in the second byte sets the source output mode, only for SSD1680
			self.command_with_data(Command::DisplayUpdateControl, &[0x00, 0b1000_0000][..])?;
		}

		self.command_with_data(
			Command::TemperatureSensorControl,
			&[TEMPERATURE_SENSOR_INTERNAL][..],
		)?;

		self.set_position(0, 0)?;

		self.power_on()?;

		Ok(())
	}

	pub fn power_on(&mut self) -> Result<(), EinkIoError> {
		self.command_with_data(
			Command::ConfigureSequence,
			&[DisplaySequence::POWER_ON.bits()][..],
		)?;
		self.command(Command::Update)?;
		self.wait_for_idle();
		Ok(())
	}

	pub fn power_off(&mut self) -> Result<(), EinkIoError> {
		self.command_with_data(
			Command::ConfigureSequence,
			&[DisplaySequence::POWER_OFF.bits()][..],
		)?;
		self.command(Command::Update)?;
		self.wait_for_idle();
		Ok(())
	}

	/// Set the position for the next pixel data.
	///
	/// # Panics
	///
	/// Panics if the coordinates are out of bounds.
	#[instrument(level = "trace", skip(self))]
	pub fn set_position(&mut self, x: u16, y: u16) -> Result<(), EinkIoError> {
		assert!(x < self.chip.width());
		assert!(y < self.chip.height());

		let x = u8::try_from(x / 8).unwrap();
		self.command_with_data(Command::XAddressCounter, &[x][..])?;

		let y = self.chip.height() - 1 - y;
		self.command_with_data(
			Command::YAddressCounter,
			&[(y & 0xFF) as u8, (y >> 8 & 0x01) as u8][..],
		)?;
		Ok(())
	}

	/// Enter deep sleep.
	#[instrument(level = "trace", skip(self))]
	pub fn deep_sleep(&mut self) -> Result<(), EinkIoError> {
		self.power_off()?;
		self.command_with_data(Command::DeepSleep, &[0x01_u8][..])?;
		self.sleeping = true;
		Ok(())
	}

	/// Send a command with associated data.
	#[instrument(level = "trace", skip(self, data))]
	pub fn command_with_data(
		&mut self,
		command: Command,
		mut data: impl Read,
	) -> Result<(), EinkIoError> {
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
	pub fn command(&mut self, command: Command) -> Result<(), EinkIoError> {
		self.set_dc(Level::Low);
		self.spi.write(&[command as u8])?;
		self.set_dc(Level::High);
		Ok(())
	}

	/// Block until the display is idle.
	// TODO: 40s timeout
	#[instrument(level = "trace", skip(self))]
	pub fn wait_for_idle(&self) {
		if self.busy.read() == Level::High {
			print!("h");
		} else {
			print!("l");
		}

		while self.busy.read() == Level::High {
			print!("H");
			sleep(Duration::from_millis(1));
		}
		println!("L");

		// I'm not getting a BUSY high at all, but waiting like this seems to work
		sleep(Duration::from_millis(50));
	}

	pub fn update_full(&mut self) -> Result<(), EinkIoError> {
		use DisplaySequence as Ds;
		self.command_with_data(
			Command::ConfigureSequence,
			&[match self.chip {
				Chip::SSD1680 => {
					Ds::ENABLE_CLOCK
						| Ds::ENABLE_ANALOG | Ds::LOAD_TEMPERATURE
						| Ds::LOAD_DEFAULT_LUT | Ds::DISPLAY
						| Ds::DISABLE_ANALOG | Ds::DISABLE_CLOCK
				}
				Chip::SSD1681 => {
					Ds::ENABLE_CLOCK
						| Ds::ENABLE_ANALOG | Ds::LOAD_TEMPERATURE
						| Ds::LOAD_DEFAULT_LUT | Ds::DISPLAY
				}
			}
			.bits()][..],
		)?;
		self.command(Command::Update)?;
		self.wait_for_idle();
		Ok(())
	}

	pub fn update_partial(&mut self) -> Result<(), EinkIoError> {
		use DisplaySequence as Ds;
		self.command_with_data(
			Command::ConfigureSequence,
			&[match self.chip {
				Chip::SSD1680 => {
					Ds::ENABLE_CLOCK | Ds::ENABLE_ANALOG | Ds::PARTIAL_MODE | Ds::DISPLAY
				}
				Chip::SSD1681 => {
					Ds::ENABLE_CLOCK
						| Ds::ENABLE_ANALOG | Ds::LOAD_TEMPERATURE
						| Ds::LOAD_DEFAULT_LUT | Ds::PARTIAL_MODE
						| Ds::DISPLAY
				}
			}
			.bits()][..],
		)?;
		self.command(Command::Update)?;
		self.wait_for_idle();
		Ok(())
	}

	/// Full display update, with black/white and red data.
	///
	/// This takes about 10 seconds to complete, and flashes the display many times.
	#[instrument(level = "trace", skip(self, bw, red))]
	pub fn display_bichrome(&mut self, bw: impl Read, red: impl Read) -> Result<(), EinkIoError> {
		self.set_position(0, 0)?;
		self.command_with_data(Command::LoadBlackWhite, bw)?;
		self.wait_for_idle();

		self.set_position(0, 0)?;
		self.command_with_data(Command::LoadRed, red)?;
		self.wait_for_idle();

		self.update_full()
	}

	/// Full display update, with black/white data only.
	///
	/// This takes about 3 seconds to complete, and flashes the display several times.
	#[instrument(level = "trace", skip(self, bw))]
	pub fn display_monochrome(&mut self, bw: impl Read) -> Result<(), EinkIoError> {
		self.set_position(0, 0)?;
		self.command_with_data(Command::LoadBlackWhite, bw)?;
		self.wait_for_idle();

		self.set_position(0, 0)?;
		self.command_with_data(Command::LoadRed, Pixels::new_for(self.chip).as_reader())?;
		self.wait_for_idle();

		self.update_full()
	}

	/// Partial display update, with black/white data only.
	///
	/// This takes about a second to complete, and doesn't flash the display.
	#[instrument(level = "trace", skip(self, bw))]
	pub fn display_partial_monochrome(&mut self, bw: impl Read) -> Result<(), EinkIoError> {
		self.set_position(0, 0)?;
		self.command_with_data(Command::LoadBlackWhite, bw)?;
		self.wait_for_idle();

		self.update_partial()?;

		self.set_position(0, 0)?;
		self.command_with_data(Command::LoadRed, Pixels::new_for(self.chip).as_reader())?;
		self.wait_for_idle();

		Ok(())
	}
}

#[allow(dead_code)]
pub const TEMPERATURE_SENSOR_EXTERNAL: u8 = 0x48;
pub const TEMPERATURE_SENSOR_INTERNAL: u8 = 0x80;

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
