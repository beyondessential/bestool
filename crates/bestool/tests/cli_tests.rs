use std::{env, ffi::OsString, fs, path::PathBuf};

use miette::{miette, Context, IntoDiagnostic, Result};
use tempfile::TempDir;

#[test]
fn cli_tests() {
	let cases = trycmd::TestCases::new();
	cases
		.env("BESTOOL_MOCK_TIME", "1")
		.env("NO_COLOR", "1")
		.case("tests/cmd/*.toml");

	let handle_res = init_db().and_then(run_db);

	if matches!(handle_res, Err(_)) {
		cases.skip("tests/cmd/alerts.toml");
	}

	cases.run();
}

/// Execute the `initdb` binary with the parameters configured in PgTempDBBuilder.
fn init_db() -> Result<TempDir> {
	let temp_dir = TempDir::with_prefix("bestool-").into_diagnostic()?;

	let data_dir = temp_dir.path().join("data");

	// write out password file for initdb
	let pwfile = temp_dir.path().join("user_password.txt");
	fs::write(&pwfile, "password")
		.into_diagnostic()
		.wrap_err("writing password file")?;

	duct::cmd!(
		find_postgres_bin("initdb")?,
		"--auth",
		"scram-sha-256",
		"--username",
		"postgres",
		"--pwfile",
		pwfile,
		"-D",
		data_dir,
	)
	.stdout_null()
	.run()
	.into_diagnostic()
	.wrap_err("running initdb")?;

	Ok(temp_dir)
}

fn run_db(temp_dir: TempDir) -> Result<impl Drop> {
	let data_dir = temp_dir.path().join("data");

	duct::cmd!(
		find_postgres_bin("pg_ctl")?,
		"start",
		"-D",
		data_dir,
		"--wait",
		"--silent",
		"--log",
		"log.txt",
		"--options",
		// https://www.postgresql.org/docs/current/non-durability.html
		// https://wiki.postgresql.org/wiki/Tuning_Your_PostgreSQL_Server
		std::concat!(
			"-c autovacuum=off ",
			"-c full_page_writes=off ",
			"-c fsync=off ",
			"-c unix_socket_directories='' ",
			"-c synchronous_commit=off",
		),
	)
	.run()
	.into_diagnostic()
	.wrap_err("running pg_ctl")?;

	struct Handle(Option<TempDir>);

	impl Drop for Handle {
		fn drop(&mut self) {
			let Some(temp_dir) = self.0.take() else {
				return;
			};
			if let Err(err) = stop_db(temp_dir) {
				eprintln!("{}", err);
			}
		}
	}

	Ok(Handle(Some(temp_dir)))
}

fn stop_db(temp_dir: TempDir) -> Result<()> {
	let data_dir = temp_dir.path().join("data");

	duct::cmd!(
		find_postgres_bin("pg_ctl")?,
		"stop",
		"-D",
		data_dir,
		"--wait",
		"--silent"
	)
	.run()
	.into_diagnostic()
	.wrap_err("running pg_ctl")?;

	// if we just used the default drop impl, errors would not be surfaced
	temp_dir
		.close()
		.into_diagnostic()
		.wrap_err("cleaning up the temp dir")?;

	Ok(())
}

pub fn find_postgres_bin(name: &str) -> Result<OsString> {
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

		Ok([root, version.as_str(), "bin", &format!("{name}.exe")]
			.iter()
			.collect::<PathBuf>()
			.into())
	}

	fn is_in_path(name: &str) -> Option<PathBuf> {
		let var = env::var_os("PATH")?;

		// Separate PATH value into paths
		let paths_iter = env::split_paths(&var);

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

	// On Windows, find `psql` assuming the standard installation using the installer
	// because PATH on Windows is not reliable.
	// See https://github.com/rust-lang/rust/issues/37519
	#[cfg(windows)]
	return find_from_installation(r"C:\Program Files\PostgreSQL", name);

	#[cfg(target_os = "linux")]
	if is_in_path(name).is_some() {
		return Ok(name.into());
	} else {
		// Ubuntu reccomends to use pg_ctlcluster over pg_ctl and doesn't put pg_ctl in PATH.
		// Still, it should be fine for temporary database.
		return find_from_installation(r"/usr/lib/postgresql", name);
	}

	#[cfg(not(any(windows, target_os = "linux")))]
	return Ok(name.into());
}
