use bitvec::{BitArr, bitarr};
use tracing::{debug, instrument};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemoryAccessControl(BitArr!(for 8, in u8));

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Vertical {
	TopToBottom,
	BottomToTop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Horizontal {
	LeftToRight,
	RightToLeft,
}

impl MemoryAccessControl {
	pub fn row_order(mut self, direction: Vertical) -> Self {
		self.0.set(0, match direction {
			Vertical::TopToBottom => false,
			Vertical::BottomToTop => true,
		});
		self
	}

	pub fn col_order(mut self, direction: Horizontal) -> Self {
		self.0.set(1, match direction {
			Horizontal::LeftToRight => false,
			Horizontal::RightToLeft => true,
		});
		self
	}

	pub fn normal(mut self) -> Self {
		self.0.set(2, false);
		self
	}

	pub fn inverted(mut self) -> Self {
		self.0.set(2, true);
		self
	}

	/// Vertical refresh order (aka Line Address Order).
	pub fn v_refresh(mut self, direction: Vertical) -> Self {
		self.0.set(3, match direction {
			Vertical::TopToBottom => false,
			Vertical::BottomToTop => true,
		});
		self
	}

	/// Horizontal refresh order (aka Data Latch Order).
	pub fn h_refresh(mut self, direction: Horizontal) -> Self {
		self.0.set(5, match direction {
			Horizontal::LeftToRight => false,
			Horizontal::RightToLeft => true,
		});
		self
	}

	pub fn rgb(mut self) -> Self {
		self.0.set(4, false);
		self
	}

	pub fn bgr(mut self) -> Self {
		self.0.set(4, true);
		self
	}
}

impl From<MemoryAccessControl> for u8 {
	fn from(control: MemoryAccessControl) -> u8 {
		let arr: [u8; 1] = control.0.into_inner();
		arr[0]
	}
}

pub const COLMOD_RGB_65K: u8 = 0b0101;
pub const COLMOD_RGB_262K: u8 = 0b0110;

pub const COLMOD_12BPP: u8 = 0b0011;
pub const COLMOD_16BPP: u8 = 0b0101;
pub const COLMOD_18BPP: u8 = 0b0110;
pub const COLMOD_16M_TRUNC: u8 = 0b0111;

/// Helper function to set the gate voltages.
///
/// Takes the high and low gate voltages in millivolts, and returns the
/// corresponding byte to send with [`Command::GateVoltages`].
#[instrument(level = "debug")]
pub fn gate_voltages(vgh: u16, vgl: u16) -> u8 {
	let vghs = if vgh <= 12200 {
		0
	} else if vgh <= 12540 {
		1
	} else if vgh <= 12890 {
		2
	} else if vgh <= 13260 {
		3
	} else if vgh <= 13650 {
		4
	} else if vgh <= 14060 {
		5
	} else if vgh <= 14500 {
		6
	} else {
		7
	};

	let vgls = if vgl <= 7160 {
		0
	} else if vgl <= 7670 {
		1
	} else if vgl <= 8230 {
		2
	} else if vgl <= 8870 {
		3
	} else if vgl <= 9600 {
		4
	} else if vgl <= 10430 {
		5
	} else if vgl <= 11380 {
		6
	} else {
		7
	};

	debug!(vghs, vgls, "gate voltage signals");
	(vghs << 4) | vgls
}

/// Helper function to set the frame rate.
///
/// Takes the frame rate in Hz, and returns the corresponding byte to send with
/// [`Command::FrameRateControl`].
#[instrument(level = "debug")]
pub fn frame_rate(rate: u8) -> u8 {
	let rate = rate.clamp(39, 119);
	match rate.clamp(39, 119) {
		low if low <= 39 => 0x1F,
		40 => 0x1E,
		41 => 0x1D,
		42 => 0x1C,
		43 => 0x1B,
		44 => 0x1A,
		45 => 0x19,
		46 => 0x18,
		47 | 48 => 0x17,
		49 => 0x16,
		50 | 51 => 0x15,
		52 => 0x14,
		53 | 54 => 0x13,
		55 | 56 => 0x12,
		57 => 0x11,
		58 | 59 => 0x10,
		60 | 61 => 0x0F,
		62 | 63 => 0x0E,
		64 | 65 | 66 => 0x0D,
		67 | 68 => 0x0C,
		69 | 70 | 71 => 0x0B,
		72 | 73 | 74 => 0x0A,
		75 | 76 | 77 => 0x09,
		78 | 79 | 80 | 81 => 0x08,
		82 | 83 | 84 | 85 => 0x07,
		86 | 87 | 88 | 89 => 0x06,
		90 | 91 | 92 | 93 => 0x05,
		94 | 95 | 96 | 97 | 98 => 0x04,
		99 | 100 | 101 | 102 | 103 | 104 => 0x03,
		105 | 106 | 107 | 108 | 109 | 110 => 0x02,
		111 | 112 | 113 | 114 | 115 | 116 | 117 | 118 => 0x01,
		_high => 0x00,
	};
	debug!(rate, "frame rate");
	rate
}

/// Helper function to set the power control 1 values.
///
/// Takes the AVDD, AVCL, and VDDS voltage levels in decivolts, and returns the corresponding bytes
/// to send with [`Command::PowerControl1`].
#[instrument(level = "debug")]
pub fn power_control1(avdd: u8, avcl: u8, vdds: u8) -> [u8; 2] {
	[0b10100100, power_control2(avdd, avcl, vdds)]
}

/// Helper function to set the power control 2 values.
///
/// Takes the AVDD, AVCL, and VDDS voltage levels in decivolts, and returns the corresponding byte
/// to send with [`Command::PowerControl2`].
#[instrument(level = "debug")]
pub fn power_control2(avdd: u8, avcl: u8, vdds: u8) -> u8 {
	let avdd = match avdd.clamp(64, 68) {
		64 | 65 => 0x0,
		66 | 67 => 0x1,
		68 => 0x2,
		_ => unreachable!(),
	};
	let avcl = match avcl.clamp(44, 50) {
		44 | 45 => 0x0,
		46 | 47 => 0x1,
		48 | 49 => 0x2,
		50 => 0x3,
		_ => unreachable!(),
	};
	let vdds = match vdds.clamp(21, 26) {
		21 | 22 => 0x0,
		23 => 0x1,
		24 => 0x2,
		25 | 26 => 0x3,
		_ => unreachable!(),
	};
	let byte = 0 | avdd << 6 | avcl << 4 | vdds;
	debug!(avdd, avcl, vdds, byte, "power control values");
	byte
}

/// Helper function to set the gate control values.
///
/// Takes the number of gate lines, the first scan line, and the flags, and returns the
/// corresponding bytes to send with [`Command::GateControl`].
#[instrument(level = "debug")]
pub fn gate_control(gate_lines: u16, first_scan_line: u16, flags: GateFlags) -> [u8; 3] {
	let nl = ((gate_lines.clamp(8, 320) - 8) / 8) as u8;
	let scn = (first_scan_line.clamp(0, 312) / 8) as u8;
	let flags = flags.into();
	debug!(nl, scn, flags, "gate control values");
	[nl, scn, flags]
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GateFlags {
	pub mirror: GateMirror,
	pub interlace: GateInterlace,
	pub scan_direction: GateScanDirection,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum GateMirror {
	#[default]
	Local,
	Full,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum GateInterlace {
	#[default]
	Interlaced,
	Progressive,
}

#[derive(Debug, Clone, Copy, Default)]
pub enum GateScanDirection {
	#[default]
	Ascending,
	Descending,
}

impl From<GateFlags> for u8 {
	fn from(flags: GateFlags) -> u8 {
		0 | (match flags.mirror {
			GateMirror::Local => 0,
			GateMirror::Full => 1,
		} << 4) | (match flags.interlace {
			GateInterlace::Interlaced => 0,
			GateInterlace::Progressive => 1,
		} << 2) | match flags.scan_direction {
			GateScanDirection::Ascending => 0,
			GateScanDirection::Descending => 1,
		}
	}
}
