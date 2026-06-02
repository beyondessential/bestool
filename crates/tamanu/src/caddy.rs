//! Locating the caddy executable.

use std::path::PathBuf;

/// The caddy executable to invoke.
///
/// On Windows, caddy is installed at `C:\Caddy\caddy.exe` and isn't
/// necessarily on `PATH`; prefer that explicit location when it's present so
/// the version healthcheck and the reload work on a stock install rather than
/// silently skipping. Everywhere else — and as the Windows fallback — we rely
/// on `caddy` being resolvable via `PATH`.
pub fn program() -> PathBuf {
	#[cfg(windows)]
	{
		let installed = PathBuf::from(r"C:\Caddy\caddy.exe");
		if installed.is_file() {
			return installed;
		}
	}
	PathBuf::from("caddy")
}
