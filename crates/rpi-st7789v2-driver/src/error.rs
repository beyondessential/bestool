/// Error type for driver operations.
#[derive(Debug, thiserror::Error)]
#[cfg_attr(feature = "miette", derive(miette::Diagnostic))]
#[error("I/O error")]
pub enum Error {
	#[cfg_attr(
		feature = "miette",
		diagnostic(help("GPIO error, check the pin numbers"))
	)]
	Gpio(#[from] rppal::gpio::Error),

	#[cfg_attr(
		feature = "miette",
		diagnostic(help("SPI error, check settings or increase spidev.bufsiz"))
	)]
	Spi(#[from] rppal::spi::Error),

	#[cfg_attr(feature = "miette", diagnostic(help("local (non-SPI/GPIO) I/O error")))]
	Io(#[from] std::io::Error),
}

/// Convenience type for Results in this crate.
pub type Result<T> = std::result::Result<T, Error>;
