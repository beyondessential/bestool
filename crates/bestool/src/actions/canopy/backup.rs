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
mod simple;

use std::{
	collections::BTreeMap,
	path::Path,
	sync::Arc,
	time::{Duration, Instant},
};

use bestool_canopy::{
	CanopyClient, DEFAULT_CANOPY_URL, TargetOutcome,
	registration::Registration,
	schema::{BackupPurpose, ReportArgs, RunOutcome},
};
use bestool_kopia::{
	RunAs, S3KopiaEnv, args_policy_set_ignores, args_repository_connect_s3, args_snapshot_create,
	build_kopia_command_with_s3, find_kopia_binary,
	proxy::{self, RunningProxy, S3ProxyConfig},
};
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result, bail, miette};
use reqwest::Url;
use tracing::{debug, error, info, warn};
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

/// Loopback bases the alertd daemon may be listening on. It binds the first that
/// is free (usually IPv6 `[::1]`), so a client fixed to one family can miss it;
/// we try both, in the daemon's own default order. Kept in step with the
/// daemon's `default_server_addrs`.
const DAEMON_BASES: [&str; 2] = ["http://[::1]:8271", "http://127.0.0.1:8271"];

/// Ask the running daemon to run the backup (starting a run or attaching to one
/// already in flight) and render its streamed status. Returns `Unreachable` if
/// the daemon isn't there, so the caller can fall back to a local run.
async fn run_via_daemon(backup_type: &str) -> std::result::Result<(), DaemonError> {
	use futures::StreamExt as _;

	let mut response = None;
	let mut last_err = String::from("no daemon address to try");
	for base in DAEMON_BASES {
		let url = format!("{base}/tasks/backup/run?type={backup_type}");
		match crate::http::client().get(&url).send().await {
			Ok(resp) => {
				response = Some(resp);
				break;
			}
			Err(err) => last_err = err.to_string(),
		}
	}
	let response = response.ok_or(DaemonError::Unreachable(last_err))?;
	if !response.status().is_success() {
		return Err(DaemonError::Unreachable(format!(
			"alertd returned {}",
			response.status()
		)));
	}

	let mut stream = response.bytes_stream();
	let mut buffer = Vec::<u8>::new();
	let mut terminal: Option<std::result::Result<(), String>> = None;
	let mut on_progress_line = false;
	while let Some(chunk) = stream.next().await {
		let chunk = chunk.map_err(|err| DaemonError::Failed(format!("daemon stream: {err}")))?;
		buffer.extend_from_slice(&chunk);
		while let Some(nl) = buffer.iter().position(|&b| b == b'\n') {
			let line: Vec<u8> = buffer.drain(..=nl).collect();
			let Ok(event) = serde_json::from_slice::<serde_json::Value>(&line) else {
				continue;
			};
			if event.get("event").and_then(serde_json::Value::as_str) == Some("progress") {
				if let Some(status) = event.get("status").and_then(serde_json::Value::as_str) {
					render_progress(status, &mut on_progress_line);
				}
				continue;
			}
			// Finish the in-place progress line before logging the next event.
			if std::mem::take(&mut on_progress_line) {
				eprintln!();
			}
			if let Some(outcome) = render_daemon_event(backup_type, &event) {
				terminal = Some(outcome);
			}
		}
	}
	if on_progress_line {
		eprintln!();
	}

	match terminal {
		Some(Ok(())) => Ok(()),
		Some(Err(message)) => Err(DaemonError::Failed(message)),
		None => Err(DaemonError::Failed(
			"lost the connection to the daemon; the backup may still be running".into(),
		)),
	}
}

/// Render a live progress line: in place (overwriting) on a terminal, or as a
/// log line otherwise.
fn render_progress(status: &str, on_progress_line: &mut bool) {
	use std::io::{IsTerminal as _, Write as _};

	let mut stderr = std::io::stderr();
	if stderr.is_terminal() {
		// Carriage return + clear-to-end-of-line: overwrite the previous update.
		let _ = write!(stderr, "\r{status}\x1b[K");
		let _ = stderr.flush();
		*on_progress_line = true;
	} else {
		info!(%status, "backup progress");
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
	/// A live progress line from kopia during the snapshot upload (its own
	/// human-readable status, e.g. "8 hashed (800 MB), uploaded 800 MB, 100%").
	Progress(String),
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
	backup_type: String,
	purpose: BackupPurpose,
	region: &str,
	run_id: Uuid,
) -> Result<RunningProxy> {
	let provider = Arc::new(CanopyCredentialProvider::new(
		client,
		backup_type,
		purpose,
		run_id,
	));
	let upstream_host = upstream_host_for(region).await;
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

fn dualstack_host(region: &str) -> String {
	format!("s3.dualstack.{region}.amazonaws.com")
}

fn plain_host(region: &str) -> String {
	format!("s3.{region}.amazonaws.com")
}

/// The S3 host the proxy connects to for `region`.
///
/// Prefer the dualstack endpoint: it carries AAAA records, so the proxy reaches
/// S3 over IPv6 where the host has it (and A records too, so IPv4-only hosts are
/// unaffected). Not every partition/region has a dualstack alias, so fall back
/// to the plain (IPv4-only) endpoint when the dualstack name doesn't resolve.
async fn upstream_host_for(region: &str) -> String {
	let dualstack = dualstack_host(region);
	let resolves = match tokio::net::lookup_host((dualstack.as_str(), 443)).await {
		Ok(mut addrs) => addrs.next().is_some(),
		Err(_) => false,
	};
	if resolves {
		debug!(host = %dualstack, "using dualstack S3 endpoint");
		dualstack
	} else {
		let plain = plain_host(region);
		debug!(host = %plain, "dualstack S3 endpoint unavailable; using plain endpoint");
		plain
	}
}

/// Connect kopia to the canopy-managed repo through the proxy (source host =
/// server id).
pub(super) async fn connect_repo(
	kopia: &Path,
	s3env: &S3KopiaEnv<'_>,
	target: &bestool_canopy::schema::BackupTarget,
	endpoint: &str,
	server_id: &str,
	run_as: RunAs,
) -> Result<()> {
	let mut connect = build_kopia_command_with_s3(kopia, s3env, run_as).map_err(|e| miette!("{e}"))?;
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

	let result = backup_after_start(&def, backup_type, &run_id, registration_dir, &progress).await;
	if let Err(err) = &result {
		error!(backup_type, %run_id, "backup failed: {}", trim_error(err));
	}
	result
}

/// The networked part of a run: resolve the canopy target, snapshot through the
/// proxy, and report the outcome. Separated so [`run_backup`] can log one
/// success/failure line covering every exit path — otherwise an outcome reaches
/// only canopy, never the daemon's own journal.
async fn backup_after_start(
	def: &BackupDef,
	backup_type: &str,
	run_id: &str,
	registration_dir: Option<&Path>,
	progress: &Option<BackupProgress>,
) -> Result<()> {
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
	// device_id is only a snapshot tag: canopy authenticates by the device cert,
	// the report doesn't carry it, and restore selects by snapshot id + type, not
	// device. Legacy-migrated registrations have no device_id (it's only set by
	// `canopy register` enrolment), so tag the snapshot when we know it and just
	// omit the tag otherwise.
	let device_id = reg.device_id.clone();

	let client = build_client(base_url_of(&reg)?, &device_key).await?;

	let target = match TargetOutcome::from_result(client.backup_target().await)? {
		TargetOutcome::Dormant => {
			info!(
				backup_type,
				"nothing to do: device not yet authorised for backups"
			);
			return Ok(());
		}
		TargetOutcome::Ready(target) => target,
	};

	let run_uuid: Uuid = run_id
		.parse()
		.into_diagnostic()
		.wrap_err("backup run_id is not a valid uuid")?;

	// The proxy serves for the whole run; held in scope until reporting is done.
	// The run id is carried on every credential issuance so Canopy can correlate
	// the whole session.
	let proxy = spawn_proxy(
		client.clone(),
		backup_type.to_owned(),
		BackupPurpose::Backup,
		&target.region,
		run_uuid,
	)
	.await?;
	let conn = RepoConn {
		endpoint: proxy.endpoint(),
		password: target.repo_password.0.clone(),
	};
	let outcome =
		run_kopia_backup(
			def,
			&target,
			&conn,
			&server_id,
			device_id.as_deref(),
			run_id,
			progress,
		)
		.await;
	emit(progress, BackupEvent::Phase("report"));

	// The proxy saw every S3 request this run made (success or failure), so its
	// tallies are a rough measure of the network/S3 traffic this run accounts for.
	let traffic = proxy.traffic();
	info!(
		backup_type,
		run_id,
		sent_raw = traffic.sent_raw,
		sent_payload = traffic.sent_payload,
		received_raw = traffic.received_raw,
		received_payload = traffic.received_payload,
		"s3 traffic for this run"
	);

	// Report whatever happened, then surface the original error (if any). The
	// traffic counts go on both outcomes: bytes flow whether or not it succeeds.
	let to_i64 = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
	let s3_sent_raw_bytes = Some(to_i64(traffic.sent_raw));
	let s3_sent_payload_bytes = Some(to_i64(traffic.sent_payload));
	let s3_received_raw_bytes = Some(to_i64(traffic.received_raw));
	let s3_received_payload_bytes = Some(to_i64(traffic.received_payload));
	let report = match &outcome {
		Ok(snapshot) => ReportArgs::builder()
			.run_id(run_uuid)
			.type_(backup_type.to_owned())
			.purpose(BackupPurpose::Backup)
			.outcome(RunOutcome::Success)
			.maybe_bytes_uploaded(snapshot.bytes_uploaded)
			.maybe_snapshot_id(snapshot.id.clone())
			.maybe_s3_sent_raw_bytes(s3_sent_raw_bytes)
			.maybe_s3_sent_payload_bytes(s3_sent_payload_bytes)
			.maybe_s3_received_raw_bytes(s3_received_raw_bytes)
			.maybe_s3_received_payload_bytes(s3_received_payload_bytes)
			.build(),
		Err(err) => ReportArgs::builder()
			.run_id(run_uuid)
			.type_(backup_type.to_owned())
			.purpose(BackupPurpose::Backup)
			.outcome(RunOutcome::Failure)
			.error(trim_error(err))
			.maybe_s3_sent_raw_bytes(s3_sent_raw_bytes)
			.maybe_s3_sent_payload_bytes(s3_sent_payload_bytes)
			.maybe_s3_received_raw_bytes(s3_received_raw_bytes)
			.maybe_s3_received_payload_bytes(s3_received_payload_bytes)
			.build(),
	};
	client
		.backup_report(&report)
		.await
		.wrap_err("reporting backup outcome to canopy")?;

	match &outcome {
		Ok(snapshot) => {
			info!(
				backup_type,
				run_id,
				snapshot_id = ?snapshot.id,
				bytes_uploaded = ?snapshot.bytes_uploaded,
				"backup completed"
			);
			emit(
				progress,
				BackupEvent::Done {
					snapshot_id: snapshot.id.clone(),
					bytes_uploaded: snapshot.bytes_uploaded,
				},
			)
		}
		Err(err) => emit(progress, BackupEvent::Failed { error: trim_error(err) }),
	}

	outcome.map(|_| ())
}

/// Connect kopia to the repo, snapshot the prepared source, parse the result.
///
/// Wraps the method's `prepare`/`cleanup` in the def's `pre`/`post` hooks, and
/// always runs cleanup + post even when the snapshot fails.
async fn run_kopia_backup(
	def: &BackupDef,
	target: &bestool_canopy::schema::BackupTarget,
	conn: &RepoConn,
	server_id: &str,
	device_id: Option<&str>,
	run_id: &str,
	progress: &Option<BackupProgress>,
) -> Result<SnapshotResult> {
	run_hooks(&def.pre, true).await?;

	emit(progress, BackupEvent::Phase("prepare"));
	let prepared = def.method.prepare(&def.r#type).await?;
	let source_path = prepared.path.clone();
	let tags = assemble_tags(&def.tags, &prepared.extra_tags, device_id, run_id, &def.r#type);

	emit(progress, BackupEvent::Phase("snapshot"));
	info!(
		backup_type = %def.r#type,
		source = %source_path.display(),
		"uploading snapshot to kopia repository"
	);
	let result = snapshot(
		target,
		conn,
		&source_path,
		server_id,
		&tags,
		&prepared.ignore,
		progress,
	)
	.await;

	// Cleanup and post-hooks run regardless of the snapshot outcome.
	let cleanup = def.method.cleanup(prepared).await;
	run_hooks(&def.post, false).await.ok();

	let snapshot = result?;
	cleanup?;
	Ok(snapshot)
}

/// Connect to the repo and create the snapshot.
async fn snapshot(
	target: &bestool_canopy::schema::BackupTarget,
	conn: &RepoConn,
	source_path: &Path,
	server_id: &str,
	tags: &BTreeMap<String, String>,
	ignore: &[String],
	progress: &Option<BackupProgress>,
) -> Result<SnapshotResult> {
	let kopia = find_kopia_binary(None).ok_or_else(|| miette!("could not find the kopia binary"))?;

	// A transient kopia config so the bucket/password never persist on the device.
	let config_dir = transient_config_dir()?;
	let config_path = config_dir.path().join("repository.config");

	// When kopia runs as the kopia user (not us), it writes and reads this config
	// itself, so hand the dir to that user. Root-owned 0700 tempdir would deny it.
	#[cfg(target_os = "linux")]
	if let Some(user) = bestool_kopia::kopia_run_as_user() {
		let status = tokio::process::Command::new("chown")
			.arg("-R")
			.arg(format!("{user}:{user}"))
			.arg(config_dir.path())
			.status()
			.await
			.into_diagnostic()
			.wrap_err("handing the transient kopia config dir to the kopia user")?;
		if !status.success() {
			bail!("chown of transient kopia config dir to {user} failed ({status})");
		}
	}
	let s3env = S3KopiaEnv {
		password: &conn.password,
		config_path: &config_path,
	};

	connect_repo(&kopia, &s3env, target, &conn.endpoint, server_id, RunAs::KopiaUser).await?;

	if !ignore.is_empty() {
		let mut policy =
			build_kopia_command_with_s3(&kopia, &s3env, RunAs::KopiaUser).map_err(|e| miette!("{e}"))?;
		args_policy_set_ignores(&mut policy, source_path, ignore);
		run_kopia(policy, "policy set").await?;
	}

	let mut create =
		build_kopia_command_with_s3(&kopia, &s3env, RunAs::KopiaUser).map_err(|e| miette!("{e}"))?;
	// Force kopia's progress output (it stays silent on a non-TTY otherwise) and
	// stream it, but only when someone's watching — a local run discards stderr.
	let stdout = if progress.is_some() {
		create.arg("--progress");
		args_snapshot_create(&mut create, source_path, tags);
		run_kopia_streaming(create, "snapshot create", progress).await?
	} else {
		args_snapshot_create(&mut create, source_path, tags);
		run_kopia(create, "snapshot create").await?
	};
	Ok(parse_snapshot_output(&stdout))
}

/// Create a transient directory for a throwaway kopia config (so the bucket and
/// password never persist on the device), ensuring the base temp dir exists
/// first. On Windows `%TEMP%` is often a per-session subdirectory
/// (`…\Temp\<session>`) that may not exist yet, and `tempfile` doesn't create the
/// parent — so create it here, or the tempdir fails with "path not found".
pub(super) fn transient_config_dir() -> Result<tempfile::TempDir> {
	let base = std::env::temp_dir();
	std::fs::create_dir_all(&base)
		.into_diagnostic()
		.wrap_err_with(|| format!("creating temp dir {}", base.display()))?;
	tempfile::tempdir_in(&base)
		.into_diagnostic()
		.wrap_err("creating transient kopia config dir")
}

/// Run a kopia command with its stdout/stderr inherited, so kopia's own
/// progress display reaches the terminal (it stays silent when its output is a
/// pipe, as [`run_kopia`] leaves it). stdin is detached so kopia can't consume
/// the caller's prompts. Errors on a non-zero exit; the output the user already
/// saw stands in for a captured error message.
pub(super) async fn run_kopia_visible(cmd: std::process::Command, what: &str) -> Result<()> {
	let status = tokio::process::Command::from(cmd)
		.stdin(std::process::Stdio::null())
		.status()
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("spawning kopia {what}"))?;
	if !status.success() {
		bail!("kopia {what} failed ({status})");
	}
	Ok(())
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

/// Run a kopia command, streaming its progress lines to the sink as they arrive
/// (kopia writes progress to stderr, `\r`-separated), and returning its stdout.
async fn run_kopia_streaming(
	cmd: std::process::Command,
	what: &str,
	progress: &Option<BackupProgress>,
) -> Result<String> {
	use std::process::Stdio;

	use tokio::io::AsyncReadExt as _;

	let mut child = tokio::process::Command::from(cmd)
		.stdout(Stdio::piped())
		.stderr(Stdio::piped())
		.spawn()
		.into_diagnostic()
		.wrap_err_with(|| format!("spawning kopia {what}"))?;
	let mut stdout = child.stdout.take().expect("piped stdout");
	let mut stderr = child.stderr.take().expect("piped stderr");

	// Read stderr concurrently, splitting on `\r`/`\n` (kopia updates its progress
	// line with carriage returns), forwarding progress lines and keeping the lot
	// for an error message.
	let progress = progress.clone();
	let stderr_task = tokio::spawn(async move {
		let mut captured = String::new();
		let mut buf = [0u8; 4096];
		let mut segment = Vec::new();
		// Throttle journal logging: kopia rewrites its progress line many times a
		// second, but the journal only wants an occasional heartbeat.
		let mut last_log: Option<Instant> = None;
		while let Ok(n) = stderr.read(&mut buf).await {
			if n == 0 {
				break;
			}
			for &byte in &buf[..n] {
				if byte == b'\r' || byte == b'\n' {
					flush_progress_segment(&mut segment, &mut captured, &progress, &mut last_log);
				} else {
					segment.push(byte);
				}
			}
		}
		flush_progress_segment(&mut segment, &mut captured, &progress, &mut last_log);
		captured
	});

	let mut out = String::new();
	stdout
		.read_to_string(&mut out)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("reading kopia {what} stdout"))?;
	let status = child
		.wait()
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("waiting for kopia {what}"))?;
	let captured = stderr_task.await.unwrap_or_default();

	if !status.success() {
		bail!("kopia {what} failed: {}", captured.trim());
	}
	Ok(out)
}

/// How often a kopia progress line is logged to the journal. Progress also goes
/// to the status sink unthrottled; this only paces the journal heartbeat.
const PROGRESS_LOG_INTERVAL: Duration = Duration::from_secs(30);

fn flush_progress_segment(
	segment: &mut Vec<u8>,
	captured: &mut String,
	progress: &Option<BackupProgress>,
	last_log: &mut Option<Instant>,
) {
	if segment.is_empty() {
		return;
	}
	let line = String::from_utf8_lossy(segment).trim().to_string();
	segment.clear();
	if line.is_empty() {
		return;
	}
	captured.push_str(&line);
	captured.push('\n');
	// kopia's progress line carries these counters; other stderr lines (e.g.
	// maintenance) aren't progress and aren't forwarded.
	if line.contains("hashing") || line.contains("hashed") || line.contains("uploaded") {
		let now = Instant::now();
		if last_log.is_none_or(|prev| now.duration_since(prev) >= PROGRESS_LOG_INTERVAL) {
			info!(progress = %line, "kopia upload progress");
			*last_log = Some(now);
		}
		emit(progress, BackupEvent::Progress(line));
	}
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
	device_id: Option<&str>,
	run_id: &str,
	backup_type: &str,
) -> BTreeMap<String, String> {
	let mut tags = def_tags.clone();
	tags.extend(extra_tags.iter().map(|(k, v)| (k.clone(), v.clone())));
	if let Some(device_id) = device_id {
		tags.insert("canopy-device".to_owned(), device_id.to_owned());
	}
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
	// `snapshot create --json` strips the whole `stats` object (kopia only emits it
	// under `--json-verbose`), so the size lives in the root directory summary
	// (`rootEntry.summ.size`), which is always present. It equals `stats.totalSize`
	// for a complete snapshot (both sum every file once); fall back to it for the
	// verbose form.
	let bytes_uploaded = value
		.get("rootEntry")
		.and_then(|r| r.get("summ"))
		.and_then(|s| s.get("size"))
		.and_then(serde_json::Value::as_i64)
		.or_else(|| {
			value
				.get("stats")
				.and_then(|s| s.get("totalSize"))
				.and_then(serde_json::Value::as_i64)
		});
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
pub(super) async fn build_client(base_url: Url, device_key: &str) -> Result<Arc<CanopyClient>> {
	let tailscale_url = bestool_canopy::TAILSCALE_URL
		.parse()
		.into_diagnostic()
		.wrap_err("parsing default canopy tailscale URL")?;
	let client =
		CanopyClient::with_urls(base_url, tailscale_url, Some(device_key), crate::http::client_builder)
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

/// Cap a kopia error to a length that fits a log line and a canopy report field.
const MAX_ERROR_LEN: usize = 1500;

pub(super) fn trim_error(err: &miette::Report) -> String {
	// Render the whole cause chain: `Display` on a report shows only the outermost
	// context ("spawning pg_basebackup"), and the cause is the part worth reporting.
	let msg = err
		.chain()
		.map(|e| e.to_string())
		.collect::<Vec<_>>()
		.join(": ");
	let len = msg.chars().count();
	if len <= MAX_ERROR_LEN {
		return msg;
	}
	// kopia leads with warnings ("too many index blobs …") and progress lines and
	// prints the operative error last, so keep the tail rather than the head.
	let tail: String = msg.chars().skip(len - MAX_ERROR_LEN).collect();
	format!("…{tail}")
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
	fn trim_error_renders_the_cause_chain() {
		let err = Err::<(), _>(std::io::Error::new(
			std::io::ErrorKind::NotFound,
			"No such file or directory",
		))
		.into_diagnostic()
		.wrap_err("spawning pg_basebackup")
		.unwrap_err();
		assert_eq!(
			trim_error(&err),
			"spawning pg_basebackup: No such file or directory"
		);
	}

	#[test]
	fn trim_error_keeps_the_operative_tail() {
		let head = "Found too many index blobs (1256), …\n".repeat(100);
		let msg = format!("{head}not authorized to perform: s3:PutObjectRetention");
		let trimmed = trim_error(&miette::miette!("{msg}"));
		assert!(trimmed.chars().count() <= MAX_ERROR_LEN + 1);
		assert!(trimmed.contains("s3:PutObjectRetention"));
		assert!(trimmed.starts_with('…'));
	}

	#[test]
	fn upstream_host_formats() {
		assert_eq!(
			dualstack_host("ap-southeast-2"),
			"s3.dualstack.ap-southeast-2.amazonaws.com"
		);
		assert_eq!(plain_host("ap-southeast-2"), "s3.ap-southeast-2.amazonaws.com");
	}

	#[test]
	fn flush_forwards_only_progress_lines() {
		let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
		let progress = Some(tx);
		let mut captured = String::new();
		let mut last_log = None;

		// A kopia progress update is forwarded…
		let mut seg = b" * 8 hashing, 8 hashed (800 MB), uploaded 800 MB, 100%".to_vec();
		flush_progress_segment(&mut seg, &mut captured, &progress, &mut last_log);
		// …an ordinary stderr line (e.g. maintenance) is not.
		let mut seg = b"Finished full maintenance.".to_vec();
		flush_progress_segment(&mut seg, &mut captured, &progress, &mut last_log);

		drop(progress);
		let mut events = Vec::new();
		while let Ok(event) = rx.try_recv() {
			events.push(event);
		}
		assert_eq!(events.len(), 1);
		assert!(matches!(&events[0], BackupEvent::Progress(line) if line.contains("uploaded")));
		// Both lines are retained for error context.
		assert!(captured.contains("Finished full maintenance."));
	}

	#[test]
	fn assemble_tags_merges_and_canopy_tags_win() {
		let mut def_tags = BTreeMap::new();
		def_tags.insert("app".to_owned(), "tamanu".to_owned());
		// A def must not be able to override the canopy-* tags.
		def_tags.insert("canopy-type".to_owned(), "spoofed".to_owned());
		let mut extra = BTreeMap::new();
		extra.insert("pg-version".to_owned(), "16".to_owned());

		let tags = assemble_tags(
			&def_tags,
			&extra,
			Some("device-uuid"),
			"run-uuid",
			"tamanu-postgres",
		);

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
	fn assemble_tags_omits_device_when_absent() {
		// A legacy-migrated host has no device id; the snapshot is still tagged
		// with run and type, just without canopy-device.
		let tags = assemble_tags(
			&BTreeMap::new(),
			&BTreeMap::new(),
			None,
			"run-uuid",
			"tamanu-postgres",
		);

		assert!(!tags.contains_key("canopy-device"));
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
	fn parse_snapshot_output_prefers_root_summary_size() {
		// The real `--json` output has no `stats` (kopia strips it); the size comes
		// from the root summary. When both are present, the root summary wins.
		let out = r#"{"id":"x","stats":{"totalSize":10},"rootEntry":{"summ":{"size":987654}}}"#;
		assert_eq!(
			parse_snapshot_output(out),
			SnapshotResult {
				id: Some("x".to_owned()),
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
