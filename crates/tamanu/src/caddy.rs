//! Locating the caddy executable and its config file.

#[cfg(any(windows, test))]
use std::path::Path;
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

/// The on-disk Caddyfile to read or reload.
///
/// On Windows the file lives under `C:\Caddy`, but a stock install often leaves
/// it as `Caddyfile.txt` (Notepad appends `.txt`, and browser downloads keep
/// the extension) rather than the extensionless `Caddyfile`; prefer whichever
/// exists, falling back to the extensionless name so error messages name the
/// canonical path. Everywhere else it's the standard `/etc/caddy/Caddyfile`.
pub fn caddyfile_path() -> PathBuf {
	#[cfg(windows)]
	{
		resolve_caddyfile(Path::new(r"C:\Caddy"))
	}
	#[cfg(not(windows))]
	PathBuf::from("/etc/caddy/Caddyfile")
}

/// Within `dir`, prefer an existing `Caddyfile`, then an existing
/// `Caddyfile.txt`, then the extensionless `Caddyfile` (so a missing-file error
/// names the canonical path).
#[cfg(any(windows, test))]
fn resolve_caddyfile(dir: &Path) -> PathBuf {
	let default = dir.join("Caddyfile");
	if default.is_file() {
		return default;
	}
	let with_txt = dir.join("Caddyfile.txt");
	if with_txt.is_file() {
		return with_txt;
	}
	default
}

#[cfg(test)]
mod tests {
	use std::fs;

	use tempfile::tempdir;

	use super::*;

	#[test]
	fn prefers_extensionless_caddyfile() {
		let dir = tempdir().unwrap();
		fs::write(dir.path().join("Caddyfile"), b"a").unwrap();
		fs::write(dir.path().join("Caddyfile.txt"), b"b").unwrap();
		assert_eq!(resolve_caddyfile(dir.path()), dir.path().join("Caddyfile"));
	}

	#[test]
	fn falls_back_to_txt_when_only_txt_exists() {
		let dir = tempdir().unwrap();
		fs::write(dir.path().join("Caddyfile.txt"), b"b").unwrap();
		assert_eq!(
			resolve_caddyfile(dir.path()),
			dir.path().join("Caddyfile.txt")
		);
	}

	#[test]
	fn defaults_to_extensionless_when_neither_exists() {
		let dir = tempdir().unwrap();
		assert_eq!(resolve_caddyfile(dir.path()), dir.path().join("Caddyfile"));
	}
}
