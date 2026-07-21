use std::path::{Path, PathBuf};

use bestool_canopy::registration;
use bestool_tamanu::server_info::{
	standard_device_key_path, standard_server_id_path, standard_tags_path,
};
use clap::Parser;
use miette::{IntoDiagnostic as _, Result, WrapErr as _};

use crate::actions::Context;

/// Erase this machine's canopy enrolment from every place it's stored.
///
/// Removes the encrypted registration, the legacy Tamanu identity files
/// (`device-key.pem`, `server-id`), the cached tags, and — when the Tamanu
/// database is reachable — the legacy `deviceKey` / `metaServerId` rows in
/// `local_system_facts`. After this the host can be enrolled afresh with
/// `bestool canopy register`.
///
/// A running alertd daemon is asked to restart afterwards so it drops the
/// removed identity; the daemon caches the registration for its lifetime
/// otherwise.
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

	// The legacy plaintext identity files, cached tags, and DB rows.
	removed.extend(super::clear_legacy_identity(&tags_path).await);

	if removed.is_empty() {
		println!("No canopy enrolment found; nothing to remove.");
	} else {
		println!("Removed:");
		for item in &removed {
			println!("  {item}");
		}
	}

	// Drop the now-removed identity from the running daemon; it caches the
	// registration for its lifetime otherwise.
	super::restart_daemon_for_registration_change().await;
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
