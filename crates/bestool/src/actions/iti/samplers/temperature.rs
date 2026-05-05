use miette::{IntoDiagnostic, Result, WrapErr};
use tracing::instrument;

/// Read the SoC core temperature in degrees Celsius via `vcgencmd measure_temp`.
#[instrument(level = "debug")]
pub fn sample() -> Result<f32> {
	duct::cmd!("vcgencmd", "measure_temp")
		.read()
		.into_diagnostic()
		.wrap_err("vcgencmd: measure_temp")?
		.trim_start_matches("temp=")
		.trim_end_matches("'C")
		.parse::<f32>()
		.into_diagnostic()
		.wrap_err("vcgencmd: parse output")
}
