use std::str::FromStr;

/// Theme selection for syntax highlighting
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
#[cfg_attr(feature = "cli", clap(rename_all = "lowercase"))]
pub enum Theme {
	Light,
	Dark,
	/// Auto-detect terminal theme
	#[default]
	Auto,
}

impl Theme {
	/// Detect terminal theme by checking background color
	///
	/// Falls back to Dark if detection fails or is not supported
	pub fn detect_terminal_theme() -> Self {
		use std::time::Duration;
		use terminal_colorsaurus::{QueryOptions, ThemeMode};

		let mut options = QueryOptions::default();

		// Use longer timeout for SSH connections or Windows
		let is_ssh = std::env::var("SSH_CONNECTION").is_ok()
			|| std::env::var("SSH_CLIENT").is_ok()
			|| std::env::var("SSH_TTY").is_ok();
		let timeout = if is_ssh || cfg!(windows) {
			Duration::from_millis(1000)
		} else {
			Duration::from_millis(200)
		};
		options.timeout = timeout;

		match terminal_colorsaurus::theme_mode(options) {
			Ok(ThemeMode::Light) => Theme::Light,
			Ok(ThemeMode::Dark) => Theme::Dark,
			Err(_) => Theme::Dark,
		}
	}
}

impl FromStr for Theme {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_lowercase().as_str() {
			"light" => Ok(Theme::Light),
			"dark" => Ok(Theme::Dark),
			"auto" => Ok(Theme::Auto),
			_ => Err(format!(
				"invalid theme: '{}', must be 'light', 'dark', or 'auto'",
				s
			)),
		}
	}
}

impl Theme {
	/// Resolve the theme to a concrete Light or Dark value
	///
	/// If the theme is Auto, performs terminal detection
	pub fn resolve(&self) -> Theme {
		match self {
			Theme::Auto => Self::detect_terminal_theme(),
			other => *other,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_theme_parsing() {
		assert_eq!("light".parse::<Theme>().unwrap(), Theme::Light);
		assert_eq!("Light".parse::<Theme>().unwrap(), Theme::Light);
		assert_eq!("LIGHT".parse::<Theme>().unwrap(), Theme::Light);
		assert_eq!("dark".parse::<Theme>().unwrap(), Theme::Dark);
		assert_eq!("Dark".parse::<Theme>().unwrap(), Theme::Dark);
		assert_eq!("auto".parse::<Theme>().unwrap(), Theme::Auto);
		assert!("invalid".parse::<Theme>().is_err());
	}

	#[test]
	fn test_theme_resolve() {
		assert_eq!(Theme::Light.resolve(), Theme::Light);
		assert_eq!(Theme::Dark.resolve(), Theme::Dark);
		// Auto resolves to either Light or Dark depending on terminal
		let resolved = Theme::Auto.resolve();
		assert!(resolved == Theme::Light || resolved == Theme::Dark);
	}
}
