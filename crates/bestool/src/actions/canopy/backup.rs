//! Config-driven, canopy-managed backups.
//!
//! A backup def (`/etc/bestool/backups/*.toml`) names a `type` (the Canopy
//! label), optional `pre`/`post` hooks and `tags`, and exactly one method
//! ([`method::Method`]). The driver fetches creds + target from Canopy, runs the
//! `pre` hooks, [`method::Method::prepare`]s a source, kopia-snapshots it with
//! the canopy-* tags, cleans up (always), runs `post`, and reports the outcome.
//!
//! The run logic lives in [`run_backup`] so the standalone `bestool canopy
//! backup` subcommand and (later) the in-process alertd trigger drive the same
//! code.

pub mod config;
pub mod creds;
pub mod method;

use std::{collections::BTreeMap, path::Path, sync::Arc};

use bestool_canopy::{
	BackupReport, CanopyClient, DEFAULT_CANOPY_URL, Outcome, Purpose, TargetOutcome,
	registration::Registration,
};
use bestool_kopia::{
	S3KopiaEnv, args_repository_connect_s3, args_snapshot_create, build_kopia_command_with_s3,
	find_kopia_binary,
};
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result, bail, miette};
use reqwest::Url;
use tracing::{info, warn};
use uuid::Uuid;

use self::{
	config::{BackupDef, Hook},
	creds::CredsServer,
};
use crate::actions::Context;

/// Run a configured backup, driving kopia and reporting to Canopy.
#[derive(Debug, Clone, Parser)]
pub struct BackupArgs {
	/// The backup type to run.
	///
	/// Must have a definition in the backups directory (a `*.toml` whose `type`
	/// matches).
	#[arg(long = "type", value_name = "TYPE")]
	pub backup_type: String,

	/// Override the registration directory (matching `register`/`export`).
	#[arg(long, value_name = "DIR")]
	pub config: Option<std::path::PathBuf>,

	/// Override the backups definition directory.
	#[arg(long, value_name = "DIR")]
	pub backups_dir: Option<std::path::PathBuf>,
}

pub async fn run(args: BackupArgs, _ctx: Context) -> Result<()> {
	run_backup(
		&args.backup_type,
		args.config.as_deref(),
		args.backups_dir.as_deref(),
	)
	.await
}

/// Parsed bits of a finished kopia `snapshot create --json` we report to Canopy.
#[derive(Debug, Default, PartialEq, Eq)]
struct SnapshotResult {
	id: Option<String>,
	bytes_uploaded: Option<i64>,
}

/// The per-run kopia env values (loopback creds endpoint + repo password),
/// owned so they outlive the borrow of the lease.
struct LeaseEnv {
	uri: String,
	token: String,
	password: String,
}

/// Drive one backup run end-to-end.
///
/// On a dormant target (the device isn't authorised for backups yet) this logs
/// and returns `Ok(())` without reporting. Otherwise it always reports the
/// outcome to Canopy once kopia has started.
pub async fn run_backup(
	backup_type: &str,
	registration_dir: Option<&Path>,
	backups_dir: Option<&Path>,
) -> Result<()> {
	let run_id = Uuid::new_v4().to_string();

	// Resolve the def first: fail fast (and without touching the network) if this
	// host has no definition for the requested type.
	let dir = backups_dir
		.map(|d| d.to_path_buf())
		.unwrap_or_else(config::backups_dir);
	let def = config::find_def(&dir, backup_type)
		.await?
		.ok_or_else(|| miette!("no backup def for type '{backup_type}' in {}", dir.display()))?;
	info!(backup_type, method = def.method.name(), %run_id, "starting backup");

	let reg = load_registration(registration_dir)
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
	let device_id = reg
		.device_id
		.clone()
		.ok_or_else(|| miette!("registration has no device id"))?;
	let base_url = base_url_of(&reg)?;

	let client = build_client(&device_key).await?;

	let target = match client.backup_target(&base_url).await? {
		TargetOutcome::Dormant => {
			info!(
				backup_type,
				"nothing to do: device not yet authorised for backups"
			);
			return Ok(());
		}
		TargetOutcome::Ready(target) => target,
	};

	let creds_server = CredsServer::start().await?;
	let lease = {
		let client = client.clone();
		let base_url = base_url.clone();
		let backup_type = backup_type.to_owned();
		creds_server.lease(Arc::new(move || {
			let client = client.clone();
			let base_url = base_url.clone();
			let backup_type = backup_type.clone();
			Box::pin(async move {
				client
					.backup_credentials(&base_url, &backup_type, Purpose::Backup)
					.await
					.map_err(|err| format!("{err}"))
			})
		}))
	};

	let env = LeaseEnv {
		uri: lease.uri().to_owned(),
		token: lease.token().to_owned(),
		password: target.repo_password.0.clone(),
	};
	let outcome = run_kopia_backup(&def, &target, &env, &server_id, &device_id, &run_id).await;

	// Report whatever happened, then surface the original error (if any).
	let report = match &outcome {
		Ok(snapshot) => BackupReport {
			run_id: &run_id,
			r#type: backup_type,
			purpose: Purpose::Backup,
			outcome: Outcome::Success,
			error: None,
			bytes_uploaded: snapshot.bytes_uploaded,
			snapshot_id: snapshot.id.as_deref(),
		},
		Err(err) => BackupReport {
			run_id: &run_id,
			r#type: backup_type,
			purpose: Purpose::Backup,
			outcome: Outcome::Failure,
			error: Some(&trim_error(err)),
			bytes_uploaded: None,
			snapshot_id: None,
		},
	};
	client
		.backup_report(&base_url, &report)
		.await
		.wrap_err("reporting backup outcome to canopy")?;

	outcome.map(|_| ())
}

/// Connect kopia to the repo, snapshot the prepared source, parse the result.
///
/// Wraps the method's `prepare`/`cleanup` in the def's `pre`/`post` hooks, and
/// always runs cleanup + post even when the snapshot fails.
async fn run_kopia_backup(
	def: &BackupDef,
	target: &bestool_canopy::BackupTarget,
	env: &LeaseEnv,
	server_id: &str,
	device_id: &str,
	run_id: &str,
) -> Result<SnapshotResult> {
	run_hooks(&def.pre, true).await?;

	let prepared = def.method.prepare().await?;
	let source_path = prepared.path.clone();
	let tags = assemble_tags(&def.tags, &prepared.extra_tags, device_id, run_id, &def.r#type);

	let result = snapshot(target, env, &source_path, server_id, &tags).await;

	// Cleanup and post-hooks run regardless of the snapshot outcome.
	let cleanup = def.method.cleanup(prepared).await;
	run_hooks(&def.post, false).await.ok();

	let snapshot = result?;
	cleanup?;
	Ok(snapshot)
}

/// Connect to the repo and create the snapshot.
async fn snapshot(
	target: &bestool_canopy::BackupTarget,
	env: &LeaseEnv,
	source_path: &Path,
	server_id: &str,
	tags: &BTreeMap<String, String>,
) -> Result<SnapshotResult> {
	let kopia = find_kopia_binary(None).ok_or_else(|| miette!("could not find the kopia binary"))?;

	// A transient kopia config so the bucket/password never persist on the device.
	let config_dir = tempfile::tempdir()
		.into_diagnostic()
		.wrap_err("creating transient kopia config dir")?;
	let config_path = config_dir.path().join("repository.config");
	let s3env = S3KopiaEnv {
		full_uri: &env.uri,
		token: &env.token,
		password: &env.password,
		config_path: &config_path,
	};

	let mut connect = build_kopia_command_with_s3(&kopia, &s3env).map_err(|e| miette!("{e}"))?;
	args_repository_connect_s3(
		&mut connect,
		&target.bucket,
		&target.prefix,
		&target.region,
		"canopy",
		server_id,
	);
	run_kopia(connect, "repository connect").await?;

	let mut create = build_kopia_command_with_s3(&kopia, &s3env).map_err(|e| miette!("{e}"))?;
	args_snapshot_create(&mut create, source_path, tags);
	let stdout = run_kopia(create, "snapshot create").await?;
	Ok(parse_snapshot_output(&stdout))
}

/// Run a kopia command, returning its stdout on success.
async fn run_kopia(cmd: std::process::Command, what: &str) -> Result<String> {
	let output = tokio::process::Command::from(cmd)
		.output()
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("spawning kopia {what}"))?;
	if !output.status.success() {
		let stderr = String::from_utf8_lossy(&output.stderr);
		bail!("kopia {what} failed: {}", stderr.trim());
	}
	Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Run a sequence of hooks. `fail_fast` aborts on the first failure (pre-hooks);
/// otherwise failures are logged and the rest still run (post-hooks).
async fn run_hooks(hooks: &[Hook], fail_fast: bool) -> Result<()> {
	for hook in hooks {
		if let Err(err) = run_hook(hook).await {
			if fail_fast {
				return Err(err);
			}
			warn!("post-hook failed (continuing): {err}");
		}
	}
	Ok(())
}

async fn run_hook(hook: &Hook) -> Result<()> {
	let Some((program, args)) = hook.command.split_first() else {
		bail!("hook has an empty command");
	};
	let status = tokio::process::Command::new(program)
		.args(args)
		.status()
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("running hook {program}"))?;
	if !status.success() {
		bail!("hook {program} exited with {status}");
	}
	Ok(())
}

/// Merge the def's tags, the method's extra tags, and the canopy-* tags.
///
/// The canopy-* tags take precedence so a def can't accidentally override them.
fn assemble_tags(
	def_tags: &BTreeMap<String, String>,
	extra_tags: &BTreeMap<String, String>,
	device_id: &str,
	run_id: &str,
	backup_type: &str,
) -> BTreeMap<String, String> {
	let mut tags = def_tags.clone();
	tags.extend(extra_tags.iter().map(|(k, v)| (k.clone(), v.clone())));
	tags.insert("canopy-device".to_owned(), device_id.to_owned());
	tags.insert("canopy-run".to_owned(), run_id.to_owned());
	tags.insert("canopy-type".to_owned(), backup_type.to_owned());
	tags
}

/// Best-effort extraction of the snapshot id and uploaded bytes from
/// `kopia snapshot create --json` output.
fn parse_snapshot_output(stdout: &str) -> SnapshotResult {
	let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout.trim()) else {
		return SnapshotResult::default();
	};
	let id = value
		.get("id")
		.and_then(|v| v.as_str())
		.map(|s| s.to_owned());
	let bytes_uploaded = value
		.get("stats")
		.and_then(|s| s.get("totalSize"))
		.and_then(|v| v.as_i64());
	SnapshotResult { id, bytes_uploaded }
}

fn base_url_of(reg: &Registration) -> Result<Url> {
	reg.api_url
		.as_deref()
		.unwrap_or(DEFAULT_CANOPY_URL)
		.parse()
		.into_diagnostic()
		.wrap_err("parsing canopy api_url")
}

async fn build_client(device_key: &str) -> Result<Arc<CanopyClient>> {
	let version = env!("CARGO_PKG_VERSION");
	let client = CanopyClient::new(version, Some(device_key), move || {
		bestool_canopy::client_builder(version)
	})
	.await?
	.ok_or_else(|| miette!("could not build a canopy client (no auth path available)"))?;
	Ok(Arc::new(client))
}

async fn load_registration(config: Option<&Path>) -> Result<Option<Registration>> {
	match config {
		Some(dir) => bestool_canopy::registration::load_from(dir).await,
		None => bestool_canopy::registration::load().await,
	}
}

fn trim_error(err: &miette::Report) -> String {
	let msg = format!("{err}");
	msg.chars().take(500).collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn assemble_tags_merges_and_canopy_tags_win() {
		let mut def_tags = BTreeMap::new();
		def_tags.insert("app".to_owned(), "tamanu".to_owned());
		// A def must not be able to override the canopy-* tags.
		def_tags.insert("canopy-type".to_owned(), "spoofed".to_owned());
		let mut extra = BTreeMap::new();
		extra.insert("pg-version".to_owned(), "16".to_owned());

		let tags = assemble_tags(&def_tags, &extra, "device-uuid", "run-uuid", "tamanu-postgres");

		assert_eq!(tags.get("app").map(String::as_str), Some("tamanu"));
		assert_eq!(tags.get("pg-version").map(String::as_str), Some("16"));
		assert_eq!(tags.get("canopy-device").map(String::as_str), Some("device-uuid"));
		assert_eq!(tags.get("canopy-run").map(String::as_str), Some("run-uuid"));
		assert_eq!(
			tags.get("canopy-type").map(String::as_str),
			Some("tamanu-postgres")
		);
	}

	#[test]
	fn parse_snapshot_output_extracts_id_and_bytes() {
		let out = r#"{"id":"abc123","stats":{"totalSize":987654},"rootEntry":{}}"#;
		assert_eq!(
			parse_snapshot_output(out),
			SnapshotResult {
				id: Some("abc123".to_owned()),
				bytes_uploaded: Some(987654),
			}
		);
	}

	#[test]
	fn parse_snapshot_output_tolerates_missing_fields_and_garbage() {
		assert_eq!(parse_snapshot_output("not json"), SnapshotResult::default());
		assert_eq!(
			parse_snapshot_output(r#"{"id":"x"}"#),
			SnapshotResult {
				id: Some("x".to_owned()),
				bytes_uploaded: None,
			}
		);
	}
}
