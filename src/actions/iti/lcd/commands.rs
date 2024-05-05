/// LCD display commands
///
/// This is a subset of the ST7789V2 command set, just enough to drive the display.
/// Descriptions are derived from usage and [the datasheet for the ST7789V2 chip][ST7789V2].
///
/// [ST7789V2]: https://files.waveshare.com/upload/c/c9/ST7789V2.pdf
#[derive(Debug, Copy, Clone)]
#[repr(u8)]
pub enum Command {
	/// No-op (NOP).
	///
	/// This command does nothing, and can be used to terminate a data stream early.
	Nop = 0x00,

	/// Memory addressing control (MADCTL).
	///
	/// 6 bits: MY, MX, MV, ML, BGR, MH.
	/// - MY: row address order (0=ttb, 1=btt)
	/// - MX: column address order (0=ltr, 1=rtl)
	/// - MV: row/column order (0=normal, 1=reverse)
	/// - ML: vertical refresh order (0=ttb, 1=btt)
	/// - BGR: RGB/BGR order (0=RGB, 1=BGR)
	/// - MH: horizontal refresh order (0=ltr, 1=rtl)
	MemoryAddressingControl = 0x36,

	/// Interface pixel format (COLMOD).
	///
	/// 2 nibbles:
	/// - RGB interface colour format:
	///   - 0b0101: 65K
	///   - 0b0110: 262K
	/// - control interface colour format:
	///   - 0b0011: 12 bit/pixel
	///   - 0b0101: 16 bit/pixel
	///   - 0b0110: 18 bit/pixel
	///   - 0b0111: 16M truncated
	InterfacePixelFormat = 0x3A,

	/// Porch settings (PORCTRL).
	///
	/// Porch is the time around the sync pulse. The front porch is padding before the sync pulse,
	/// and the back porch is padding after the sync pulse, before the start of the active pixels.
	///
	/// 5 bytes:
	/// - back porch (7 bits)
	/// - front porch (7 bits)
	/// - enable separate porch control (1 bit)
	/// - idle mode porch (2 nibbles):
	///   - back porch (4 bits)
	///   - front porch (4 bits)
	/// - partial mode porch (2 nibbles):
	///   - back porch (4 bits)
	///   - front porch (4 bits)
	///
	/// Each setting has a minimum value of 1.
	PorchSettings = 0xB2,

	/// Gate voltage control (GCTRL).
	///
	/// Voltage levels for the gate driver.
	///
	/// 2 nibbles:
	/// - VGH: gate high voltage level (4 bits)
	/// - VGL: gate low voltage level (4 bits)
	///
	/// Use the [`gate_voltages()`](super::helpers::gate_voltages) helper function to set these.
	GateVoltages = 0xB7,

	/// VCOM setting (VCOMS).
	///
	/// 1 byte:
	/// - VCOMS: VCOM selection (6 bits)
	///
	/// VCOM is the common voltage level for the display. It's used to set the zero reference for
	/// the pixel voltages. Refer to the datasheet for the correct value.
	VcomSetting = 0xBB,

	/// LCM control (LCMCTRL).
	///
	/// 7 bits.
	LcmControl = 0xC0,

	/// VDV and VRH command enable (VDVVRHEN).
	///
	/// 2 bytes:
	/// - 1 bit: command enable (0=value comes from NVM, 1=value comes from commands)
	/// - 8 bits, all ones
	VdvVrhEnable = 0xC2,

	/// VRH setting (VRHS).
	///
	/// 1 byte:
	/// - VRHS: VRH selection (6 bits)
	VrhSetting = 0xC3,

	/// VDV setting (VDVS).
	///
	/// 1 byte:
	/// - VDVS: VDV selection (6 bits)
	VdvSetting = 0xC4,

	/// Frame rate control in normal mode (FRCTRL2).
	///
	/// 2 values in 1 byte:
	/// - 3 bits: inversion selection
	///   - 0b000: dot inversion
	///   - 0b111: column inversion
	/// - 5 bits: frame rate
	///   - 0x00: 119Hz
	///   - 0x1F: 39Hz
	FrameRateControl = 0xC6,

	/// Power control 1 (PWCTRL1).
	///
	/// 3 values in 2 bytes:
	/// - 1 byte: always 0b10100100
	/// - 2 bits: AVDD voltage level
	/// - 2 bits: AVCL voltage level
	/// - 2 bits: always 00
	/// - 2 bits: VDDS voltage level
	///
	/// Use the [`power_control1()`](super::helpers::power_control1) helper function to set these.
	PowerControl1 = 0xD0,

	/// Power control 2 (PWCTRL2).
	///
	/// Undocumented, going from the Waveshare driver.
	///
	/// 3 values in 1 byte:
	/// - 2 bits: AVDD voltage level
	/// - 2 bits: AVCL voltage level
	/// - 2 bits: always 00
	/// - 2 bits: VDDS voltage level
	///
	/// Use the [`power_control2()`](super::helpers::power_control2) helper function to set these,
	/// with the same values as PWCTRL1.
	PowerControl2 = 0xD6,

	/// Positive voltage gamma control (PGAMCTRL).
	///
	/// 14 bytes. Refer to the datasheet.
	PositiveGammaControl = 0xE0,

	/// Negative voltage gamma control (NGAMCTRL).
	///
	/// 14 bytes. Refer to the datasheet.
	NegativeGammaControl = 0xE1,

	/// Gate control (GATECTRL).
	///
	/// 5 values in 3 bytes:
	/// - 6 bits: number of gate line (L=8(N+1), where N is the value and L is the line number)
	/// - 6 bits: first scan line (L=8N, where N is the value and L is the line number)
	/// - flags:
	///   - reserved (0)
	///   - reserved (0)
	///   - reserved (0)
	///   - mirror selection (0=local, 1=full)
	///   - reserved (0)
	///   - interlace selection (0=interlaced, 1=progressive)
	///   - reserved (0)
	///   - scan direction (0=ascending, 1=descending)
	///
	/// Use the [`gate_control()`](super::helpers::gate_control) helper function to set these.
	GateControl = 0xE4,

	/// Switch on display inversion (INVON).
	InversionOn = 0x21,

	/// Sleep (SLPIN).
	///
	/// This must be followed by a delay of at least 5ms.
	Sleep = 0x10,

	/// Wake up (SLPOUT).
	///
	/// This must be followed by a delay of at least 120ms.
	WakeUp = 0x11,

	/// Turn display off (DISPOFF).
	DisplayOff = 0x28,

	/// Turn display on (DISPON).
	DisplayOn = 0x29,

	/// Set column addresses (CASET).
	///
	/// This sets the area of the screen the display will write to.
	///
	/// 2 u16s:
	/// - start column
	/// - end column
	ColumnAddressSet = 0x2A,

	/// Set row addresses (RASET).
	///
	/// This sets the area of the screen the display will write to.
	///
	/// 2 u16s:
	/// - start row
	/// - end row
	RowAddressSet = 0x2B,

	/// Memory write (RAMWR).
	///
	/// This will consider the next bytes as pixel data to write to the screen.
	/// Either send all the data as expected by window size
	/// (`width * height * 2 bytes`), or send a NOP to end the write.
	MemoryWrite = 0x2C,
}
