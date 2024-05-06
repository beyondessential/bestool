use clap::Parser;
use miette::{IntoDiagnostic, Result, WrapErr};
use rppal::{gpio::Gpio, i2c::I2c};
use tracing::instrument;

use crate::actions::Context;

/// Get battery information from the X1201 board.
#[derive(Debug, Clone, Parser)]
pub struct BatteryArgs {
	/// Output in JSON format.
	#[arg(long)]
	pub json: bool,
}

pub async fn run(ctx: Context<BatteryArgs>) -> Result<()> {
	let BatteryArgs { json } = ctx.args_top;

	let gpio = Gpio::new().into_diagnostic().wrap_err("gpio: init")?;
	let powered = gpio
		.get(6)
		.into_diagnostic()
		.wrap_err("gpio: read pin=6")?
		.into_input()
		.is_high();

	let mut i2c = I2c::new().into_diagnostic().wrap_err("i2c: init")?;
	i2c.set_slave_address(0x36)
		.into_diagnostic()
		.wrap_err("i2c: set address")?;

	// https://www.analog.com/media/en/technical-documentation/data-sheets/MAX17048-MAX17049.pdf
	let vcell = (read(&mut i2c, 0x2)? as f64) * 1.25 / 1000.0 / 16.0;
	let capacity = (read(&mut i2c, 0x4)? as f64) / 256.0;
	let version = read(&mut i2c, 0x8)?;

	if json {
		println!(
			"{}",
			serde_json::json!({ "powered": powered, "vcell": vcell, "capacity": capacity, "version": version })
		);
	} else {
		println!("Powered: {}", powered);
		println!("Version: {}", version);
		println!("Voltage: {:.2} V", vcell);
		println!("Battery: {:.2}%", capacity);
	}

	Ok(())
}

#[instrument(level = "debug", skip(i2c))]
fn read(i2c: &mut I2c, addr: u8) -> Result<u16> {
	let data = i2c
		.smbus_read_word(addr)
		.into_diagnostic()
		.wrap_err(format!("i2c: read {addr:2X?}"))?;
	Ok(u16::from_le_bytes(data.to_be_bytes()))
}
