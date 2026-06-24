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
pub mod method;
pub mod postgresql;
pub mod provider;

use std::{collections::BTreeMap, path::Path, sync::Arc};

use bestool_canopy::{
	BackupReport, CanopyClient, DEFAULT_CANOPY_URL, Outcome, Purpose, TargetOutcome,
	registration::Registration,
};
use bestool_kopia::{
	S3KopiaEnv, args_policy_set_ignores, args_repository_connect_s3, args_snapshot_create,
	build_kopia_command_with_s3, find_kopia_binary,
	proxy::{self, RunningProxy, S3ProxyConfig},
};
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result, bail, miette};
use reqwest::Url;
use tracing::{info, warn};
use uuid::Uuid;

use self::{
	config::{BackupDef, Hook},
	provider::CanopyCredentialProvider,
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

	/// Run the backup in this process instead of delegating to the alertd daemon.
	///
	/// By default, when the daemon is running, the backup is run by it and its
	/// progress is streamed here; this forces a local run.
	#[arg(long)]
	pub no_daemon: bool,
}

pub async fn run(args: BackupArgs, _ctx: Context) -> Result<()> {
	let run_local = || {
		run_backup(
			&args.backup_type,
			args.config.as_deref(),
			args.backups_dir.as_deref(),
			None,
		)
	};

	if args.no_daemon {
		return run_local().await;
	}

	match run_via_daemon(&args.backup_type).await {
		Ok(()) => Ok(()),
		Err(DaemonError::Failed(message)) => bail!("backup failed: {message}"),
		Err(DaemonError::Unreachable(err)) => {
			info!(%err, "alertd daemon not reachable; running the backup locally");
			run_local().await
		}
	}
}

/// Why a delegated run didn't yield a clean success.
enum DaemonError {
	/// The daemon couldn't be reached (or doesn't expose the endpoint); the
	/// caller should run locally.
	Unreachable(String),
	/// The daemon ran the backup and it failed, or the stream was lost mid-run.
	Failed(String),
}

const DAEMON_BASE: &str = "http://127.0.0.1:8271";

/// Ask the running daemon to run the backup (starting a run or attaching to one
/// already in flight) and render its streamed status. Returns `Unreachable` if
/// the daemon isn't there, so the caller can fall back to a local run.
async fn run_via_daemon(backup_type: &str) -> std::result::Result<(), DaemonError> {
	use futures::StreamExt as _;

	let url = format!("{DAEMON_BASE}/tasks/backup/run?type={backup_type}");
	let response = crate::http::client()
		.get(&url)
		.send()
		.await
		.map_err(|err| DaemonError::Unreachable(err.to_string()))?;
	if !response.status().is_success() {
		return Err(DaemonError::Unreachable(format!(
			"alertd returned {}",
			response.status()
		)));
	}

	let mut stream = response.bytes_stream();
	let mut buffer = Vec::<u8>::new();
	let mut terminal: Option<std::result::Result<(), String>> = None;
	while let Some(chunk) = stream.next().await {
		let chunk = chunk.map_err(|err| DaemonError::Failed(format!("daemon stream: {err}")))?;
		buffer.extend_from_slice(&chunk);
		while let Some(nl) = buffer.iter().position(|&b| b == b'\n') {
			let line: Vec<u8> = buffer.drain(..=nl).collect();
			let Ok(event) = serde_json::from_slice::<serde_json::Value>(&line) else {
				continue;
			};
			if let Some(outcome) = render_daemon_event(backup_type, &event) {
				terminal = Some(outcome);
			}
		}
	}

	match terminal {
		Some(Ok(())) => Ok(()),
		Some(Err(message)) => Err(DaemonError::Failed(message)),
		None => Err(DaemonError::Failed(
			"lost the connection to the daemon; the backup may still be running".into(),
		)),
	}
}

/// Render one streamed status event; returns `Some` for a terminal event.
fn render_daemon_event(
	backup_type: &str,
	event: &serde_json::Value,
) -> Option<std::result::Result<(), String>> {
	let field = |key| event.get(key).and_then(serde_json::Value::as_str);
	match field("event") {
		Some("started") => {
			info!(backup_type, run_id = field("runId"), "daemon started the backup");
			None
		}
		Some("attached") => {
			info!(
				backup_type,
				run_id = field("runId"),
				started_at = field("startedAt"),
				"attached to a backup already running on the daemon"
			);
			None
		}
		Some("phase") => {
			info!(backup_type, phase = field("phase"), "backup phase");
			None
		}
		Some("heartbeat") => None,
		Some("done") => {
			info!(
				backup_type,
				snapshot_id = field("snapshotId"),
				"daemon finished the backup"
			);
			Some(Ok(()))
		}
		Some("error") => Some(Err(field("message").unwrap_or("unknown error").to_owned())),
		_ => None,
	}
}

/// Parsed bits of a finished kopia `snapshot create --json` we report to Canopy.
#[derive(Debug, Default, PartialEq, Eq)]
struct SnapshotResult {
	id: Option<String>,
	bytes_uploaded: Option<i64>,
}

/// A status event emitted by [`run_backup`] when it's given a progress sink, so
/// the daemon's backup task can stream a run's progress to an attached client.
#[derive(Debug, Clone)]
pub enum BackupEvent {
	/// The run acquired its per-type lock and started.
	Started { run_id: String },
	/// Entered a named phase: `prepare`, `snapshot`, or `report`.
	Phase(&'static str),
	/// The run finished successfully.
	Done {
		snapshot_id: Option<String>,
		bytes_uploaded: Option<i64>,
	},
	/// The run failed.
	Failed { error: String },
}

/// Sink for [`BackupEvent`]s. Unbounded so emitting never blocks the run on a
/// slow consumer; events are best-effort and dropped once the receiver is gone.
pub type BackupProgress = tokio::sync::mpsc::UnboundedSender<BackupEvent>;

fn emit(sink: &Option<BackupProgress>, event: BackupEvent) {
	if let Some(tx) = sink {
		let _ = tx.send(event);
	}
}

/// Connection details for a kopia run: the loopback proxy endpoint and the repo
/// passphrase, owned so they outlive the borrows in [`S3KopiaEnv`].
pub(super) struct RepoConn {
	pub endpoint: String,
	pub password: String,
}

/// Spawn the loopback re-signing proxy for `(backup_type, purpose)`, drawing
/// live credentials from Canopy. The returned proxy serves until dropped.
pub(super) async fn spawn_proxy(
	client: Arc<CanopyClient>,
	base_url: Url,
	backup_type: String,
	purpose: Purpose,
	region: &str,
) -> Result<RunningProxy> {
	let provider = Arc::new(CanopyCredentialProvider::new(
		client,
		base_url,
		backup_type,
		purpose,
	));
	let upstream_host = format!("s3.{region}.amazonaws.com");
	let config = S3ProxyConfig {
		upstream: format!("https://{upstream_host}"),
		upstream_host,
		region: region.to_owned(),
	};
	proxy::spawn(config, provider)
		.await
		.into_diagnostic()
		.wrap_err("starting the S3 re-signing proxy")
}

/// Connect kopia to the canopy-managed repo through the proxy (source host =
/// server id).
pub(super) async fn connect_repo(
	kopia: &Path,
	s3env: &S3KopiaEnv<'_>,
	target: &bestool_canopy::BackupTarget,
	endpoint: &str,
	server_id: &str,
) -> Result<()> {
	let mut connect = build_kopia_command_with_s3(kopia, s3env).map_err(|e| miette!("{e}"))?;
	args_repository_connect_s3(
		&mut connect,
		&target.bucket,
		&target.prefix,
		&target.region,
		endpoint,
		"canopy",
		server_id,
	);
	run_kopia(connect, "repository connect").await?;
	Ok(())
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
	progress: Option<BackupProgress>,
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

	// Cross-process guard: a run holds an exclusive lock for its type for its
	// whole duration, so a re-emitted "back up now" or a manual run racing the
	// daemon doesn't start a second concurrent kopia. Held until the function
	// returns (the OS releases it if we crash).
	let Some(_lock) = try_acquire_lock(&lock_path(backup_type)).await? else {
		info!(backup_type, "a backup of this type is already running; skipping");
		return Ok(());
	};
	info!(backup_type, method = def.method.name(), %run_id, "starting backup");
	emit(&progress, BackupEvent::Started { run_id: run_id.clone() });

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

	// The proxy serves for the whole run; held in scope until reporting is done.
	let proxy = spawn_proxy(
		client.clone(),
		base_url.clone(),
		backup_type.to_owned(),
		Purpose::Backup,
		&target.region,
	)
	.await?;
	let conn = RepoConn {
		endpoint: proxy.endpoint(),
		password: target.repo_password.0.clone(),
	};
	let outcome =
		run_kopia_backup(&def, &target, &conn, &server_id, &device_id, &run_id, &progress).await;
	emit(&progress, BackupEvent::Phase("report"));

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

	match &outcome {
		Ok(snapshot) => emit(
			&progress,
			BackupEvent::Done {
				snapshot_id: snapshot.id.clone(),
				bytes_uploaded: snapshot.bytes_uploaded,
			},
		),
		Err(err) => emit(&progress, BackupEvent::Failed { error: trim_error(err) }),
	}

	outcome.map(|_| ())
}

/// Connect kopia to the repo, snapshot the prepared source, parse the result.
///
/// Wraps the method's `prepare`/`cleanup` in the def's `pre`/`post` hooks, and
/// always runs cleanup + post even when the snapshot fails.
async fn run_kopia_backup(
	def: &BackupDef,
	target: &bestool_canopy::BackupTarget,
	conn: &RepoConn,
	server_id: &str,
	device_id: &str,
	run_id: &str,
	progress: &Option<BackupProgress>,
) -> Result<SnapshotResult> {
	run_hooks(&def.pre, true).await?;

	emit(progress, BackupEvent::Phase("prepare"));
	let prepared = def.method.prepare(&def.r#type).await?;
	let source_path = prepared.path.clone();
	let tags = assemble_tags(&def.tags, &prepared.extra_tags, device_id, run_id, &def.r#type);

	emit(progress, BackupEvent::Phase("snapshot"));
	let result = snapshot(target, conn, &source_path, server_id, &tags, &prepared.ignore).await;

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
	conn: &RepoConn,
	source_path: &Path,
	server_id: &str,
	tags: &BTreeMap<String, String>,
	ignore: &[String],
) -> Result<SnapshotResult> {
	let kopia = find_kopia_binary(None).ok_or_else(|| miette!("could not find the kopia binary"))?;

	// A transient kopia config so the bucket/password never persist on the device.
	let config_dir = tempfile::tempdir()
		.into_diagnostic()
		.wrap_err("creating transient kopia config dir")?;
	let config_path = config_dir.path().join("repository.config");
	let s3env = S3KopiaEnv {
		password: &conn.password,
		config_path: &config_path,
	};

	connect_repo(&kopia, &s3env, target, &conn.endpoint, server_id).await?;

	if !ignore.is_empty() {
		let mut policy = build_kopia_command_with_s3(&kopia, &s3env).map_err(|e| miette!("{e}"))?;
		args_policy_set_ignores(&mut policy, source_path, ignore);
		run_kopia(policy, "policy set").await?;
	}

	let mut create = build_kopia_command_with_s3(&kopia, &s3env).map_err(|e| miette!("{e}"))?;
	args_snapshot_create(&mut create, source_path, tags);
	let stdout = run_kopia(create, "snapshot create").await?;
	Ok(parse_snapshot_output(&stdout))
}

/// Run a kopia command, returning its stdout on success.
pub(super) async fn run_kopia(cmd: std::process::Command, what: &str) -> Result<String> {
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

/// Resolve the canopy base URL from the registration (or the default).
pub(super) fn base_url_of(reg: &Registration) -> Result<Url> {
	reg.api_url
		.as_deref()
		.unwrap_or(DEFAULT_CANOPY_URL)
		.parse()
		.into_diagnostic()
		.wrap_err("parsing canopy api_url")
}

/// Build a canopy client for an already-enrolled host (tailscale, then mTLS).
pub(super) async fn build_client(device_key: &str) -> Result<Arc<CanopyClient>> {
	let version = env!("CARGO_PKG_VERSION");
	let client = CanopyClient::new(version, Some(device_key), move || {
		bestool_canopy::client_builder(version)
	})
	.await?
	.ok_or_else(|| miette!("could not build a canopy client (no auth path available)"))?;
	Ok(Arc::new(client))
}

/// Load the registration, honouring an explicit `--config` dir.
pub(super) async fn load_registration(config: Option<&Path>) -> Result<Option<Registration>> {
	match config {
		Some(dir) => bestool_canopy::registration::load_from(dir).await,
		None => bestool_canopy::registration::load().await,
	}
}

fn trim_error(err: &miette::Report) -> String {
	let msg = format!("{err}");
	msg.chars().take(500).collect()
}

/// Per-type lockfile path, in a runtime dir (tmpfs on Linux, so it's cleared on
/// reboot and never stale across crashes).
fn lock_path(backup_type: &str) -> std::path::PathBuf {
	let name = format!("backup-{}.lock", backup_type.replace(['/', '\\'], "_"));
	#[cfg(unix)]
	{
		std::path::PathBuf::from("/run/bestool").join(name)
	}
	#[cfg(not(unix))]
	{
		std::env::temp_dir().join(format!("bestool-{name}"))
	}
}

/// Try to take the exclusive per-run lock. `Ok(Some(file))` holds the lock for
/// as long as the returned handle lives; `Ok(None)` means another run holds it.
async fn try_acquire_lock(path: &Path) -> Result<Option<tokio::fs::File>> {
	use fs4::tokio::AsyncFileExt as _;

	if let Some(parent) = path.parent() {
		tokio::fs::create_dir_all(parent).await.ok();
	}
	let file = tokio::fs::OpenOptions::new()
		.create(true)
		.write(true)
		.truncate(false)
		.open(path)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("opening backup lockfile {}", path.display()))?;
	match file.try_lock() {
		Ok(()) => Ok(Some(file)),
		Err(fs4::TryLockError::WouldBlock) => Ok(None),
		Err(fs4::TryLockError::Error(err)) => Err(err)
			.into_diagnostic()
			.wrap_err_with(|| format!("locking backup lockfile {}", path.display())),
	}
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
	fn lock_path_names_per_type_and_sanitises_separators() {
		assert!(
			lock_path("tamanu-postgres")
				.to_string_lossy()
				.ends_with("backup-tamanu-postgres.lock")
		);
		assert!(
			lock_path("a/b")
				.to_string_lossy()
				.ends_with("backup-a_b.lock")
		);
	}

	#[tokio::test]
	async fn lock_is_exclusive_and_releases_on_drop() {
		let tmp = tempfile::tempdir().unwrap();
		let path = tmp.path().join("backup-test.lock");
		let held = try_acquire_lock(&path).await.unwrap();
		assert!(held.is_some(), "first acquire takes the lock");
		// A second attempt while the first is held is refused.
		assert!(
			try_acquire_lock(&path).await.unwrap().is_none(),
			"a concurrent run is locked out"
		);
		drop(held);
		// Released → acquirable again.
		assert!(try_acquire_lock(&path).await.unwrap().is_some());
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
