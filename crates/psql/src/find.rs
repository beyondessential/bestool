use std::{
	ffi::OsString,
	fs,
	path::{Path, PathBuf},
};

use miette::{miette, IntoDiagnostic as _, Result};
use tracing::instrument;

#[instrument(level = "debug")]
pub fn find_postgres_bin(name: &str) -> Result<OsString> {
	if Path::new(name).is_absolute() {
		return Ok(name.into());
	}

	// On Windows, find `psql` assuming the standard installation using the installer
	// because PATH on Windows is not reliable.
	// See https://github.com/rust-lang/rust/issues/37519
	#[cfg(windows)]
	return find_from_installation(r"C:\Program Files\PostgreSQL", name);

	#[cfg(unix)]
	if is_in_path(name).is_some() {
		Ok(name.into())
	} else {
		// Ubuntu recommends to use pg_ctlcluster over pg_ctl and doesn't put pg_ctl in PATH.
		find_from_installation(r"/usr/lib/postgresql", name)
	}

	#[cfg(not(any(windows, unix)))]
	return Ok(name.into());
}

#[cfg(any(windows, unix))]
#[tracing::instrument(level = "debug")]
fn find_from_installation(root: &str, name: &str) -> Result<OsString> {
	let version = fs::read_dir(root)
		.into_diagnostic()?
		.filter_map(|res| {
			res.map(|dir| {
				dir.file_name()
					.into_string()
					.ok()
					.filter(|name| name.parse::<u32>().is_ok())
			})
			.transpose()
		})
		// Use `u32::MAX` in case of `Err` so that we always catch IO errors.
		.max_by_key(|res| {
			res.as_ref()
				.cloned()
				.map(|n| n.parse::<u32>().unwrap())
				.unwrap_or(u32::MAX)
		})
		.ok_or_else(|| miette!("the Postgres root {root} is empty"))?
		.into_diagnostic()?;

	let exec_file_name = if cfg!(windows) {
		format!("{name}.exe")
	} else {
		name.to_string()
	};
	Ok([root, version.as_str(), "bin", &exec_file_name]
		.iter()
		.collect::<PathBuf>()
		.into())
}

#[cfg(unix)]
fn is_in_path(name: &str) -> Option<PathBuf> {
	let var = std::env::var_os("PATH")?;

	// Separate PATH value into paths
	let paths_iter = std::env::split_paths(&var);

	// Attempt to read each path as a directory
	let dirs_iter = paths_iter.filter_map(|path| fs::read_dir(path).ok());

	for dir in dirs_iter {
		let mut matches_iter = dir
			.filter_map(|file| file.ok())
			.filter(|file| file.file_name() == name);
		if let Some(file) = matches_iter.next() {
			return Some(file.path());
		}
	}

	None
}
