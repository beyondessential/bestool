//! This is a set of utilities to setup a temporary Postgres cluster. The design is inspired by
//! [`pg_test`](https://github.com/rubenv/pgtest) and
//! [`pgtemp`](https://github.com/boustrophedon/pgtemp). This is more lightweight than containers
//! and cleaner than simply creating databases. The code is lightly adapted from `pgtemp`
//! (MIT license) with handlable errors.

use std::fs;

use bestool::find_postgres::find_postgres_bin;
use miette::{Context, IntoDiagnostic, Result};
use tempfile::TempDir;

/// Execute the `initdb` binary.
pub fn init_db() -> Result<TempDir> {
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

/// Execute the `pg_ctl start`.
///
/// The Postgres server and resources get cleaned when the returned handle drops.
pub fn run_db(temp_dir: TempDir) -> Result<impl Drop> {
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
		if cfg!(unix) {
			// Setting "unix_socket_directories" is necessary as creating socket files fail in permission error for some systems.
			// Instead, this forces the use of TCP/IP over domain sockets.
			"-c autovacuum=off -c full_page_writes=off -c fsync=off -c unix_socket_directories='' -c synchronous_commit=off"
		} else {
			"-c autovacuum=off -c full_page_writes=off -c fsync=off -c synchronous_commit=off"
		},
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

	load_database().wrap_err("loading fixture database")?;

	Ok(Handle(Some(temp_dir)))
}

fn load_database() -> Result<()> {
	duct::cmd!(
		find_postgres_bin("psql")?,
		"--host",
		"localhost",
		"--username",
		"postgres",
		"--file",
		"tests/fixture.sql",
	)
	.env("PGPASSWORD", "password")
	.stdout_null()
	.run()
	.into_diagnostic()
	.wrap_err("running psql")?;

	Ok(())
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
