use std::fs;

use miette::{Context, IntoDiagnostic, Result};
use tempfile::TempDir;

#[test]
fn cli_tests() {
	let cases = trycmd::TestCases::new();
	cases
		.env("BESTOOL_TIMELESS", "1")
		.env("RUST_LOG", "warn")
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
		"initdb",
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
		"pg_ctl",
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

	duct::cmd!("pg_ctl", "stop", "-D", data_dir, "--wait", "--silent")
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
