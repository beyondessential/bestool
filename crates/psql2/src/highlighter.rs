//! SQL syntax highlighting theme configuration

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
		// Try to detect terminal background using OSC 11 query
		// This is best-effort and may not work on all terminals
		#[cfg(unix)]
		{
			use std::io::{Read, Write};
			use std::os::unix::io::AsRawFd;

			// Try to query terminal background color
			let stdin_fd = std::io::stdin().as_raw_fd();

			// Save terminal state
			let mut termios: libc::termios = unsafe { std::mem::zeroed() };
			if unsafe { libc::tcgetattr(stdin_fd, &mut termios) } != 0 {
				return Theme::Dark;
			}

			let original_termios = termios;

			// Set raw mode for reading response
			unsafe {
				libc::cfmakeraw(&mut termios);
				termios.c_cc[libc::VMIN] = 0;
				termios.c_cc[libc::VTIME] = 1; // 0.1 second timeout
				if libc::tcsetattr(stdin_fd, libc::TCSANOW, &termios) != 0 {
					return Theme::Dark;
				}
			}

			// Query background color
			let query = b"\x1b]11;?\x1b\\";
			let mut stdout = std::io::stdout();
			let result = (|| -> Option<Theme> {
				stdout.write_all(query).ok()?;
				stdout.flush().ok()?;

				// Read response (timeout after 100ms via VTIME)
				let mut stdin = std::io::stdin();
				let mut buf = [0u8; 256];
				let n = stdin.read(&mut buf).ok()?;

				if n == 0 {
					return None;
				}

				let response = String::from_utf8_lossy(&buf[..n]);

				// Parse response: ESC ] 11 ; rgb:RRRR/GGGG/BBBB ESC \
				if let Some(rgb_start) = response.find("rgb:") {
					let rgb_part = &response[rgb_start + 4..];
					if let Some(slash1) = rgb_part.find('/') {
						let r_str = &rgb_part[..slash1];
						if let Ok(r) = u16::from_str_radix(&r_str[..r_str.len().min(4)], 16) {
							// Calculate brightness (simple average of RGB)
							// If R component is high, likely light background
							let brightness = (r as f32 / 65535.0) * 100.0;
							return Some(if brightness > 50.0 {
								Theme::Light
							} else {
								Theme::Dark
							});
						}
					}
				}

				None
			})();

			// Restore terminal
			unsafe {
				libc::tcsetattr(stdin_fd, libc::TCSANOW, &original_termios);
			}

			result.unwrap_or(Theme::Dark)
		}

		#[cfg(not(unix))]
		{
			// Windows and other platforms: default to dark
			Theme::Dark
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
