use std::path::{Path, PathBuf};

use bestool_canopy::registration;
use bestool_tamanu::server_info::{
	standard_device_key_path, standard_server_id_path, standard_tags_path,
};
use clap::Parser;
use miette::{IntoDiagnostic as _, Result, WrapErr as _};
use tracing::warn;

use crate::actions::Context;

/// Erase this machine's canopy enrolment from every place it's stored.
///
/// Removes the encrypted registration, the legacy Tamanu identity files
/// (`device-key.pem`, `server-id`), the cached tags, and — when the Tamanu
/// database is reachable — the legacy `deviceKey` / `metaServerId` rows in
/// `local_system_facts`. After this the host can be enrolled afresh with
/// `bestool canopy register`.
///
/// The daemon caches the registration in memory for its lifetime, so restart it
/// afterwards for the removal to take effect.
#[derive(Debug, Clone, Parser)]
pub struct UnregisterArgs {
	/// Directory holding the encrypted canopy registration.
	///
	/// Defaults to the platform's machine-global config directory
	/// (`/etc/bestool`, or `%ProgramData%\bestool` on Windows).
	#[arg(long, value_name = "DIR")]
	pub config: Option<PathBuf>,

	/// Skip the confirmation prompt.
	#[arg(long, short = 'y')]
	pub yes: bool,
}

pub async fn run(args: UnregisterArgs, _ctx: Context) -> Result<()> {
	let UnregisterArgs { config, yes } = args;
	let dir = config.unwrap_or_else(registration::default_dir);
	let tags_path = dir.join("tags.json");

	// Elevate before touching root/admin-owned files, matching register.
	super::ensure_writable_or_reexec(&dir)?;

	if !yes && !confirm(&dir, &tags_path)? {
		println!("Aborted; nothing was removed.");
		return Ok(());
	}

	let mut removed = Vec::new();

	// The encrypted registration: the source of truth.
	if registration::delete_in(&dir)
		.await
		.wrap_err("removing canopy registration")?
	{
		removed.push(format!("canopy registration ({})", dir.display()));
	}

	// The legacy plaintext identity files and the cached tags (new + legacy).
	for path in [
		standard_device_key_path(),
		standard_server_id_path(),
		tags_path,
		standard_tags_path(),
	] {
		if remove_file(&path)? {
			removed.push(path.display().to_string());
		}
	}

	// The legacy DB rows, gated on the database being reachable: without them
	// the daemon could re-seed the old key onto disk.
	match delete_db_rows().await {
		DbOutcome::Deleted(n) if n > 0 => {
			removed.push(format!("{n} local_system_facts row(s) (deviceKey/metaServerId)"))
		}
		DbOutcome::Deleted(_) => {}
		DbOutcome::Skipped(why) => {
			warn!("{why}; leaving any deviceKey/metaServerId DB rows in place")
		}
	}

	if removed.is_empty() {
		println!("No canopy enrolment found; nothing to remove.");
	} else {
		println!("Removed:");
		for item in &removed {
			println!("  {item}");
		}
		println!();
		println!("Restart the alertd daemon so it drops the cached registration.");
	}
	Ok(())
}

/// Prompt the operator to confirm the erasure, listing every target. Any answer
/// other than `y`/`yes` (including a closed stdin) aborts.
fn confirm(dir: &Path, tags_path: &Path) -> Result<bool> {
	use std::io::Write as _;

	println!("This erases this host's canopy enrolment from:");
	println!("  canopy registration ({})", dir.display());
	println!("  {}", standard_device_key_path().display());
	println!("  {}", standard_server_id_path().display());
	println!("  {} (cached tags)", tags_path.display());
	println!("  {} (legacy cached tags)", standard_tags_path().display());
	println!("  deviceKey/metaServerId rows in the Tamanu database (if reachable)");
	print!("Proceed? [y/N] ");
	std::io::stdout().flush().into_diagnostic()?;

	let mut line = String::new();
	std::io::stdin().read_line(&mut line).into_diagnostic()?;
	Ok(matches!(
		line.trim().to_ascii_lowercase().as_str(),
		"y" | "yes"
	))
}

fn remove_file(path: &Path) -> Result<bool> {
	match std::fs::remove_file(path) {
		Ok(()) => Ok(true),
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
		Err(err) => Err(err)
			.into_diagnostic()
			.wrap_err_with(|| format!("removing {}", path.display())),
	}
}

/// Outcome of the gated DB-row deletion.
enum DbOutcome {
	/// Connected and issued the delete; carries the number of rows removed.
	Deleted(u64),
	/// The database wasn't reachable (or couldn't be located); the reason is
	/// surfaced to the operator so they know the rows were left alone.
	Skipped(String),
}

async fn delete_db_rows() -> DbOutcome {
	let url = match resolve_database_url().await {
		Ok(url) => url,
		Err(why) => return DbOutcome::Skipped(why),
	};

	let client = match bestool_postgres::pool::connect_one(&url, "bestool-canopy-unregister").await {
		Ok(client) => client,
		Err(err) => {
			return DbOutcome::Skipped(format!("could not connect to the Tamanu database: {err}"));
		}
	};

	match client
		.execute(
			"DELETE FROM local_system_facts WHERE key IN ('deviceKey', 'metaServerId')",
			&[],
		)
		.await
	{
		Ok(n) => DbOutcome::Deleted(n),
		Err(err) => DbOutcome::Skipped(format!("could not delete the DB rows: {err}")),
	}
}

/// Resolve the Tamanu database URL from `TAMANU_DATABASE_URL` or, failing that,
/// the discovered Tamanu install's config. Returns the reason as an error
/// string when neither is available, for the operator-facing skip message.
async fn resolve_database_url() -> Result<String, String> {
	use bestool_tamanu::config::{database_url_override, load_config};

	if let Some(url) = database_url_override() {
		return Ok(url);
	}

	match bestool_tamanu::try_find_tamanu(None).await {
		Ok(Some((_, root))) => match load_config(&root, None) {
			Ok(config) => Ok(config.database_url()),
			Err(err) => Err(format!("could not load Tamanu config: {err}")),
		},
		Ok(None) => Err("no Tamanu install found and TAMANU_DATABASE_URL not set".into()),
		Err(err) => Err(format!("could not locate Tamanu: {err}")),
	}
}
