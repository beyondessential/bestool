use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Stdio;

use bestool_kopia::{SnapshotSelectorArgs, short_id};
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result};
#[cfg(not(unix))]
use miette::bail;

use super::{
	KopiaArgs,
	common::{current_hostname, kopia_binary},
};
use crate::actions::Context;

/// Mount a kopia snapshot read-only via FUSE.
///
/// Snapshot selection mirrors `restore`: explicit `--snapshot ID`, `--latest`
/// (which requires `--tag` or `--path`), or an interactive picker over the
/// filter flags when neither is given.
#[derive(Debug, Clone, Parser)]
pub struct MountArgs {
	/// Mountpoint. The directory must exist and be empty (kopia requirement).
	#[arg(value_name = "MOUNTPOINT")]
	pub mountpoint: PathBuf,

	#[command(flatten)]
	pub selector: SnapshotSelectorArgs,

	/// Detach the mount process and return immediately. Unix-only; on
	/// Windows the kopia mount stays in foreground regardless.
	#[arg(long)]
	pub background: bool,
}

pub async fn run(args: MountArgs, ctx: Context) -> Result<()> {
	let kopia = ctx.require::<KopiaArgs>();
	let bin = kopia_binary(kopia)?;

	let chosen = args
		.selector
		.resolve(&bin, current_hostname(), "Choose a snapshot to mount")?;

	if args.background {
		mount_background(&bin, &chosen.id, &args.mountpoint)
	} else {
		println!(
			"Mounting snapshot {} at {} (Ctrl+C to unmount)",
			short_id(&chosen.id),
			args.mountpoint.display()
		);
		mount_foreground(&bin, &chosen.id, &args.mountpoint)
	}
}

fn mount_foreground(bin: &Path, snapshot_id: &str, mountpoint: &Path) -> Result<()> {
	let status = std::process::Command::new(bin)
		.arg("mount")
		.arg(snapshot_id)
		.arg(mountpoint)
		.env("KOPIA_CHECK_FOR_UPDATES", "false")
		.status()
		.into_diagnostic()
		.wrap_err_with(|| format!("invoking {}", bin.display()))?;
	if !status.success() {
		std::process::exit(status.code().unwrap_or(1));
	}
	Ok(())
}

#[cfg(unix)]
fn mount_background(bin: &Path, snapshot_id: &str, mountpoint: &Path) -> Result<()> {
	// `setsid --fork` puts the spawned process in a new session AND forks once
	// so the resulting daemon isn't the session leader, which is the standard
	// recipe for detaching from a controlling terminal. setsid(1) is part of
	// util-linux and ships on every Linux distro.
	let child = std::process::Command::new("setsid")
		.arg("--fork")
		.arg(bin)
		.arg("mount")
		.arg(snapshot_id)
		.arg(mountpoint)
		.env("KOPIA_CHECK_FOR_UPDATES", "false")
		.stdin(Stdio::null())
		.stdout(Stdio::null())
		.stderr(Stdio::null())
		.spawn()
		.into_diagnostic()
		.wrap_err("spawning `setsid --fork`; install util-linux if missing")?;
	println!(
		"Mounted snapshot {} at {} in the background; the kopia process detached via setsid (parent setsid pid {})",
		short_id(snapshot_id),
		mountpoint.display(),
		child.id(),
	);
	Ok(())
}

#[cfg(not(unix))]
fn mount_background(_bin: &Path, _snapshot_id: &str, _mountpoint: &Path) -> Result<()> {
	bail!("--background mounts are only supported on Unix");
}
