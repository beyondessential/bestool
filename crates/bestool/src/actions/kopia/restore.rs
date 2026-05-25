use std::path::{Path, PathBuf};

use bestool_kopia::{SnapshotSelectorArgs, format_snapshot_line, human_bytes};
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result, bail};
use sysinfo::Disks;
use tracing::{info, warn};

use super::{
	KopiaArgs,
	common::{current_hostname, kopia_binary},
};
use crate::actions::Context;

/// Restore a kopia snapshot to a destination directory.
///
/// Without `--snapshot` or `--latest`, opens an interactive picker over the
/// matching snapshots (filtered by `--source-host` / `--tag` / `--path` /
/// `--since`). `--latest` picks the newest match without prompting —
/// required when stdout isn't a terminal, and requires `--tag` or `--path` so
/// the chosen snapshot is unambiguous.
#[derive(Debug, Clone, Parser)]
pub struct RestoreArgs {
	/// Destination directory. Kopia creates this directory; with
	/// `--overwrite` it'll restore into an existing one.
	#[arg(value_name = "DESTINATION")]
	pub destination: PathBuf,

	#[command(flatten)]
	pub selector: SnapshotSelectorArgs,

	/// Resolve the snapshot to restore and print it — don't invoke `kopia
	/// snapshot restore`.
	#[arg(long)]
	pub dry_run: bool,

	/// Allow restoring into a destination that already exists or already has
	/// files. Passes `--overwrite-directories --overwrite-files` to kopia.
	#[arg(long)]
	pub overwrite: bool,

	/// Skip the pre-flight free-space check.
	///
	/// By default the command compares the snapshot's stated size against
	/// the available free space on the destination's filesystem and refuses
	/// to proceed if the snapshot won't fit. Pass this to skip the check —
	/// useful when the snapshot's recorded size is an over-estimate (kopia
	/// can deduplicate within and across snapshots, so the actual restored
	/// bytes may be less than the summed size).
	#[arg(long)]
	pub no_space_check: bool,

	/// Emit the resolved snapshot as JSON on stdout (works with `--dry-run`).
	#[arg(long)]
	pub json: bool,
}

pub async fn run(args: RestoreArgs, ctx: Context) -> Result<()> {
	let kopia = ctx.require::<KopiaArgs>();
	let bin = kopia_binary(kopia)?;

	let chosen = args
		.selector
		.resolve(&bin, current_hostname(), "Choose a snapshot to restore")?;

	if args.json {
		serde_json::to_writer_pretty(std::io::stdout().lock(), &chosen)
			.into_diagnostic()
			.wrap_err("emitting JSON")?;
		println!();
	}

	if args.dry_run {
		if !args.json {
			println!("Would restore: {}", format_snapshot_line(&chosen));
			println!("Into: {}", args.destination.display());
		}
		return Ok(());
	}

	if !args.no_space_check {
		check_disk_space(&args.destination, chosen.total_size())?;
	}

	invoke_kopia_restore(&bin, &chosen.id, &args.destination, args.overwrite)
}

/// Refuse to proceed if the destination filesystem doesn't have enough free
/// space to hold the snapshot's recorded size.
///
/// `snapshot_size` can be `None` if kopia didn't record one — we skip the
/// check with a warning rather than refuse, since we have no evidence either
/// way.
fn check_disk_space(destination: &Path, snapshot_size: Option<i64>) -> Result<()> {
	let Some(size) = snapshot_size.filter(|s| *s > 0) else {
		warn!("snapshot size unknown; skipping pre-flight disk-space check");
		return Ok(());
	};

	let check_path = existing_ancestor(destination);
	let Some(free) = available_bytes_for(&check_path) else {
		warn!(
			path = %check_path.display(),
			"could not determine free space at destination; skipping pre-flight disk-space check"
		);
		return Ok(());
	};

	if size as u64 > free {
		bail!(
			"snapshot is {} but only {} free at {} — pass --no-space-check to proceed anyway (kopia dedup can mean the actual on-disk size is smaller)",
			human_bytes(size),
			human_bytes(free as i64),
			check_path.display(),
		);
	}

	info!(
		snapshot_size = %human_bytes(size),
		free = %human_bytes(free as i64),
		path = %check_path.display(),
		"pre-flight disk-space check passed"
	);
	Ok(())
}

/// Walk up the destination until we find a path that exists, so we can stat
/// the filesystem it lives on. Kopia creates the destination itself, so the
/// path the operator passed may not exist yet.
fn existing_ancestor(path: &Path) -> PathBuf {
	let mut p: PathBuf = path.to_path_buf();
	while !p.exists() {
		match p.parent() {
			Some(parent) if parent != p => p = parent.to_path_buf(),
			_ => return PathBuf::from("/"),
		}
	}
	p
}

/// Free bytes available on the filesystem hosting `path`. Picks the longest-
/// match mount point — same logic as the doctor's `disk_free` check.
fn available_bytes_for(path: &Path) -> Option<u64> {
	let disks = Disks::new_with_refreshed_list();
	disks
		.iter()
		.filter(|d| path.starts_with(d.mount_point()))
		.max_by_key(|d| d.mount_point().as_os_str().len())
		.map(|d| d.available_space())
}

fn invoke_kopia_restore(
	bin: &Path,
	snapshot_id: &str,
	destination: &Path,
	overwrite: bool,
) -> Result<()> {
	let mut cmd = std::process::Command::new(bin);
	cmd.arg("snapshot")
		.arg("restore")
		.arg(snapshot_id)
		.arg(destination)
		.env("KOPIA_CHECK_FOR_UPDATES", "false");
	if overwrite {
		cmd.arg("--overwrite-directories").arg("--overwrite-files");
	}

	let status = cmd
		.status()
		.into_diagnostic()
		.wrap_err_with(|| format!("invoking {}", bin.display()))?;
	if !status.success() {
		std::process::exit(status.code().unwrap_or(1));
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn existing_ancestor_of_existing_path() {
		// `/` always exists.
		assert_eq!(existing_ancestor(Path::new("/")), PathBuf::from("/"));
	}

	#[test]
	fn existing_ancestor_walks_up() {
		let p = Path::new("/nonexistent/sub/dir/here");
		let got = existing_ancestor(p);
		assert!(got.exists(), "{} doesn't exist", got.display());
	}
}
