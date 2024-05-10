/// Eink display chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Chip {
	SSD1680,
	SSD1681,
}

impl Chip {
	/// Get the display's width in pixels.
	///
	/// This is also called the "Source" size.
	pub const fn width(self) -> u16 {
		match self {
			Chip::SSD1680 => 176,
			Chip::SSD1681 => 200,
		}
	}

	/// Get the display's height in pixels.
	///
	/// This is also called the "Gate" size.
	pub const fn height(self) -> u16 {
		match self {
			Chip::SSD1680 => 296,
			Chip::SSD1681 => 200,
		}
	}

	/// Default driver output control.
	pub const fn driver_output_control(self) -> [u8; 3] {
		match self {
			Chip::SSD1680 => [0x27, 0x01, 0x01],
			Chip::SSD1681 => [0xC7, 0x00, 0x01],
		}
	}

	/// X address range.
	pub const fn x_range(self) -> [u8; 2] {
		match self {
			Chip::SSD1680 => [0x00, 0x0F],
			Chip::SSD1681 => [0x00, 0x28],
		}
	}

	/// Y address range.
	pub const fn y_range(self) -> [u8; 4] {
		match self {
			Chip::SSD1680 => [0x27, 0x01, 0x00, 0x00],
			Chip::SSD1681 => [0xC7, 0x00, 0x00, 0x00],
		}
	}
}
