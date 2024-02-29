use std::{io::Read, thread::sleep, time::Duration};

use rppal::{
	gpio::{Gpio, InputPin, Level, OutputPin},
	spi::{Bus, Mode, SlaveSelect, Spi},
};

use super::EinkArgs;

#[derive(Debug, thiserror::Error, miette::Diagnostic)]
#[error("I/O error")]
pub enum EinkIoError {
	Gpio(#[from] rppal::gpio::Error),
	Spi(#[from] rppal::spi::Error),
	Io(#[from] std::io::Error),
}

#[derive(Debug)]
pub struct EinkIo {
	spi: Spi,
	busy: InputPin,
	dc: OutputPin,
	reset: OutputPin,
	width: u16,
	height: u16,
}

impl EinkIo {
	/// Connect to the Eink display I/O.
	///
	/// This performs the necessary setup for the GPIO and SPI pins, but doesn't touch the display
	/// otherwise. Usually you'll want to call `wake()` right after, unless you know the display is
	/// currently not in deep sleep.
	pub fn new(eink: &EinkArgs) -> Result<Self, EinkIoError> {
		let gpio = Gpio::new()?;
		let busy = gpio.get(eink.busy)?.into_input();
		let reset = gpio.get(eink.reset)?.into_output();

		let mut dc = gpio.get(eink.dc)?.into_output();
		dc.set_high();

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
			width: eink.width,
			height: eink.height,
		})
	}

	/// Wake the display from deep sleep.
	pub fn wake(&mut self) {
		self.reset.set_high();
		sleep(Duration::from_millis(50));
		self.reset.set_low();
		sleep(Duration::from_millis(50));
		self.reset.set_high();

		self.wait_for_idle();
	}

	/// Enter deep sleep.
	pub fn deep_sleep(&mut self) -> Result<(), EinkIoError> {
		self.command_with_data(Command::DeepSleep, &[0x01_u8][..])
	}

	/// Send a command with associated data.
	pub fn command_with_data(
		&mut self,
		command: Command,
		mut data: impl Read,
	) -> Result<(), EinkIoError> {
		self.dc.set_low();
		self.spi.write(&[command as u8])?;
		self.dc.set_high();

		let mut buf = Vec::with_capacity((self.width * self.height) as usize);
		data.read_to_end(&mut buf)?;
		self.spi.write(&buf)?;

		Ok(())
	}

	/// Send a data-less command.
	pub fn command(&mut self, command: Command) -> Result<(), EinkIoError> {
		self.dc.set_low();
		self.spi.write(&[command as u8])?;
		self.dc.set_high();
		Ok(())
	}

	/// Block until the display is idle.
	pub fn wait_for_idle(&self) {
		while self.busy.read() == Level::High {
			sleep(Duration::from_millis(1));
		}
	}

	/// Full display update, with black/white and red data.
	///
	/// This takes about 15 seconds to complete, and flashes the display many times.
	pub fn display_bichrome(&mut self, bw: impl Read, red: impl Read) -> Result<(), EinkIoError> {
		// load pixel data
		self.command_with_data(Command::LoadBlackWhite, bw)?;
		self.wait_for_idle();
		self.command_with_data(Command::LoadRed, red)?;
		self.wait_for_idle();

		// display
		self.command_with_data(
			Command::ConfigureSequence,
			&[DisplaySequence::FULL_UPDATE.bits()][..],
		)?;
		self.command(Command::Update)?;
		self.wait_for_idle();

		Ok(())
	}

	/// Full display update, with black/white data only.
	///
	/// This takes about 5 seconds to complete, and flashes the display several times.
	pub fn display_monochrome(&mut self, bw: impl Read) -> Result<(), EinkIoError> {
		// load pixel data
		self.command_with_data(Command::LoadBlackWhite, bw)?;
		self.wait_for_idle();

		// custom LUT
		self.command_with_data(Command::LoadLut, &super::lut::MONOCHROME[..])?;
		self.wait_for_idle();

		// display
		self.command_with_data(
			Command::ConfigureSequence,
			&[DisplaySequence::MONO_UPDATE.bits()][..],
		)?;
		self.command(Command::Update)?;
		self.wait_for_idle();

		Ok(())
	}

	/// Partial display update, with black/white data only.
	///
	/// This takes about a second to complete, and doesn't flash the display.
	pub fn display_partial_monochrome(&mut self, bw: impl Read) -> Result<(), EinkIoError> {
		// load pixel data
		self.command_with_data(Command::LoadBlackWhite, bw)?;
		self.wait_for_idle();

		// custom LUT
		self.command_with_data(Command::LoadLut, &super::lut::PARTIAL[..])?;
		self.wait_for_idle();

		// display
		self.command_with_data(
			Command::ConfigureSequence,
			&[DisplaySequence::PARTIAL_UPDATE.bits()][..],
		)?;
		self.command(Command::Update)?;
		self.wait_for_idle();

		Ok(())
	}

	/// Get the display dimensions (W, H).
	pub fn dimensions(&self) -> (u16, u16) {
		(self.width, self.height)
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
	/// Enter deep sleep mode.
	///
	/// This will keep BUSY high, so `wait_for_idle()` will block forever if called. To wake the
	/// display, hardware reset is required (with `hw_reset()` method).
	DeepSleep = 0x10,

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
	LoadLut = 0x32,

	/// Set display update sequence options.
	///
	/// This must be followed by a [`DisplaySequence`] byte.
	ConfigureSequence = 0x22,

	/// Start the display update sequence.
	Update = 0x20,
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

		/// Full b/w/red update, reset the screen, with built-in LUT.
		const FULL_UPDATE =
			Self::ENABLE_CLOCK.bits() |
			Self::ENABLE_ANALOG.bits() |
			Self::LOAD_TEMPERATURE.bits() |
			Self::LOAD_DEFAULT_LUT.bits() |
			Self::DISPLAY.bits() |
			Self::DISABLE_ANALOG.bits() |
			Self::DISABLE_CLOCK.bits();

		/// B/W-only update, reset the screen, requires custom LUT.
		const MONO_UPDATE =
			Self::ENABLE_CLOCK.bits() |
			Self::ENABLE_ANALOG.bits() |
			Self::DISPLAY.bits() |
			Self::DISABLE_ANALOG.bits() |
			Self::DISABLE_CLOCK.bits();

		/// Only do the display bits, don't reset the screen, requires custom LUT.
		const PARTIAL_UPDATE =
			Self::ENABLE_CLOCK.bits() |
			Self::ENABLE_ANALOG.bits() |
			Self::PARTIAL_MODE.bits() |
			Self::DISPLAY.bits();
	}
}
