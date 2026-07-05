//! `bestool canopy kopia`: run an arbitrary kopia command against the
//! canopy-managed repository.
//!
//! Fetches short-lived credentials for a backup type and purpose, connects
//! kopia to the repository through the loopback proxy, and execs the kopia
//! arguments that follow `--` with the operator's own stdio. Useful for
//! inspection and maintenance (`snapshot list`, `content stats`, `maintenance
//! run`, …) without hand-wiring credentials.

use std::path::PathBuf;

use bestool_canopy::{TargetOutcome, schema::BackupPurpose};
use bestool_kopia::{
	RunAs, S3KopiaEnv, build_kopia_command_with_s3, find_kopia_binary,
};
use clap::{Parser, ValueEnum};
use miette::{Context as _, IntoDiagnostic as _, Result, bail, miette};

use super::backup::{base_url_of, build_client, connect_repo, load_registration, spawn_proxy};
use crate::actions::Context;

/// Run a kopia command against Canopy's repository.
///
/// Everything after `--` is passed to kopia verbatim; its output and exit
/// status are the kopia command's own.
#[derive(Debug, Clone, Parser)]
pub struct KopiaArgs {
	/// The backup type whose credentials to use.
	#[arg(long = "type", value_name = "TYPE")]
	pub backup_type: String,

	/// Credential scope: read-only `restore`, or write-without-delete `backup`.
	#[arg(long, value_enum, default_value_t = Purpose::Restore)]
	pub purpose: Purpose,

	/// Override the registration directory.
	#[arg(long, value_name = "DIR")]
	pub config: Option<PathBuf>,

	/// The kopia arguments to run (everything after `--`).
	#[arg(
		trailing_var_arg = true,
		allow_hyphen_values = true,
		value_name = "KOPIA_ARGS"
	)]
	pub args: Vec<String>,
}

/// Which credential scope to request from Canopy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Purpose {
	/// Read-only credentials.
	Restore,
	/// Write-without-delete credentials.
	Backup,
}

impl From<Purpose> for BackupPurpose {
	fn from(purpose: Purpose) -> Self {
		match purpose {
			Purpose::Restore => BackupPurpose::Restore,
			Purpose::Backup => BackupPurpose::Backup,
		}
	}
}

pub async fn run(args: KopiaArgs, _ctx: Context) -> Result<()> {
	if args.args.is_empty() {
		bail!("no kopia command given; pass the kopia arguments after `--`");
	}

	let reg = load_registration(args.config.as_deref())
		.await?
		.ok_or_else(|| miette!("not registered with canopy; run `bestool canopy register` first"))?;
	let device_key = reg
		.device_key
		.clone()
		.ok_or_else(|| miette!("registration has no device key"))?;
	let server_id = reg
		.server_id
		.clone()
		.ok_or_else(|| miette!("registration has no server id"))?;
	let client = build_client(base_url_of(&reg)?, &device_key).await?;

	let target = match TargetOutcome::from_result(client.backup_target().await)? {
		TargetOutcome::Ready(target) => target,
		TargetOutcome::Dormant => {
			bail!("device is not authorised for this backup repository")
		}
	};

	// The proxy serves for the whole command; held in scope to the end.
	let proxy = spawn_proxy(
		client.clone(),
		args.backup_type.clone(),
		args.purpose.into(),
		&target.region,
	)
	.await?;
	let config_dir = tempfile::tempdir()
		.into_diagnostic()
		.wrap_err("creating transient kopia config dir")?;
	let config_path = config_dir.path().join("repository.config");
	let s3env = S3KopiaEnv {
		password: &target.repo_password.0,
		config_path: &config_path,
	};
	let kopia = find_kopia_binary(None).ok_or_else(|| miette!("could not find the kopia binary"))?;
	connect_repo(
		&kopia,
		&s3env,
		&target,
		&proxy.endpoint(),
		&server_id,
		RunAs::CurrentUser,
	)
	.await?;

	let mut cmd = build_kopia_command_with_s3(&kopia, &s3env, RunAs::CurrentUser)
		.map_err(|e| miette!("{e}"))?;
	cmd.args(&args.args);
	let status = tokio::process::Command::from(cmd)
		.status()
		.await
		.into_diagnostic()
		.wrap_err("running kopia")?;
	if !status.success() {
		bail!("kopia exited with {status}");
	}
	Ok(())
}
