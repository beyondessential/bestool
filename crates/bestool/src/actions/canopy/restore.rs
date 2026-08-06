//! `bestool canopy restore`: restore a backup, method-aware.
//!
//! Reads the def for `--type`, fetches restore-purpose creds, kopia-restores the
//! selected snapshot into a staging dir, and dispatches to the method's restore
//! (the `postgresql` method does the full stop/swap/start). Defs that follow the
//! restored type (`after`) are restored with it, each from the earliest snapshot
//! taken at or after the one being restored, so a database-and-store cycle comes
//! back as a consistent pair. Refuses to overwrite existing data unless
//! `--clobber-existing-data-yes-i-am-sure` or an interactive confirmation is
//! given.

use std::{
	io::{IsTerminal as _, Write as _},
	path::{Path, PathBuf},
	sync::Arc,
};

use bestool_canopy::{
	CanopyClient, TargetOutcome,
	schema::{BackupPurpose, BackupTarget, ReportArgs, RunOutcome},
};
use bestool_kopia::{
	RunAs, S3KopiaEnv, Snapshot, args_snapshot_list, args_snapshot_restore,
	build_kopia_command_with_s3, find_kopia_binary, proxy::TrafficStats,
};
use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result, bail, miette};
use tracing::{info, warn};
use uuid::Uuid;

use super::backup::{
	base_url_of, build_client, config, connect_repo, hold, load_registration, method::RestoreOpts,
	postgresql::space, progress::ProgressReporter, run_kopia, run_kopia_visible, spawn_proxy,
	transient_config_dir,
	trim_error,
};
use crate::actions::Context;

/// Restore a backup from Canopy's repository.
#[derive(Debug, Clone, Parser)]
pub struct RestoreArgs {
	/// The backup type to restore (must have a def in the backups directory).
	#[arg(value_name = "TYPE")]
	pub backup_type: String,

	/// The snapshot id to restore (a prefix is accepted).
	#[arg(value_name = "ID", required_unless_present = "from_hold")]
	pub id: Option<String>,

	/// Restore from a capture held on this device instead of from the repository.
	///
	/// Reads only local data, so it needs no credentials and downloads nothing.
	/// The held capture is left in place, so a restore that fails partway can be
	/// attempted again from the same rollback point.
	///
	/// Takes a hold id, as shown by `bestool canopy hold list`.
	#[arg(long, value_name = "HOLD", conflicts_with = "id")]
	pub from_hold: Option<String>,

	/// Override the destination (the simple method's path); postgresql always
	/// targets its configured cluster.
	#[arg(long, value_name = "PATH")]
	pub target: Option<PathBuf>,

	/// Proceed even if the destination already contains data (non-interactive).
	#[arg(long = "clobber-existing-data-yes-i-am-sure")]
	pub clobber: bool,

	/// Restore only the named type, skipping the defs that follow it.
	///
	/// By default, restoring a type also restores each def that declares
	/// `after` on it, from the earliest snapshot of that def's type taken at or
	/// after the one being restored, never an earlier one, which could lack
	/// content the restored data references.
	#[arg(long)]
	pub no_followers: bool,

	/// Override the registration directory.
	#[arg(long, value_name = "DIR")]
	pub config: Option<PathBuf>,

	/// Override the backups definition directory.
	#[arg(long, value_name = "DIR")]
	pub backups_dir: Option<PathBuf>,
}

pub async fn run(args: RestoreArgs, _ctx: Context) -> Result<()> {
	let dir = args
		.backups_dir
		.clone()
		.unwrap_or_else(config::backups_dir);
	let def = config::find_def(&dir, &args.backup_type)
		.await?
		.ok_or_else(|| {
			miette!(
				"no backup def for type '{}' in {}",
				args.backup_type,
				dir.display()
			)
		})?;

	// A restore from a hold reads only local data: no registration, no
	// credentials, no repository, and nothing to report to Canopy, which has no
	// part in a hold's lifecycle.
	if let Some(hold_id) = args.from_hold.clone() {
		return restore_from_hold(&hold_id, &def, &args).await;
	}

	let reg = load_registration(args.config.as_deref())
		.await?
		.ok_or_else(|| miette!("not registered with canopy; run `bestool canopy register` first"))?;
	let server_id = reg
		.server_id
		.clone()
		.ok_or_else(|| miette!("registration has no server id"))?;
	let client = build_client(base_url_of(&reg)?, reg.device_key.as_deref()).await?;

	let target = match TargetOutcome::from_result(client.backup_target().await)? {
		TargetOutcome::Ready(target) => target,
		TargetOutcome::Dormant => {
			bail!("device is not authorised for this backup repository (cannot restore)")
		}
	};

	// A fresh run id per restore (canopy rejects a repeated one), carried on every
	// credential issuance so canopy can correlate the whole session.
	let run_id = Uuid::new_v4();

	// Read-only creds + connection (the restore purpose downscopes server-side).
	// The proxy serves for the whole restore; held in scope to the end.
	let proxy = spawn_proxy(
		client.clone(),
		args.backup_type.clone(),
		BackupPurpose::Restore,
		&target.region,
		run_id,
	)
	.await?;
	let config_dir = transient_config_dir()?;
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

	// Select the snapshot to restore.
	let snapshots = list_snapshots(&kopia, &s3env).await?;
	// Guaranteed present: the argument is required unless `--from-hold` is given,
	// and that path returned above.
	let wanted = args
		.id
		.as_deref()
		.ok_or_else(|| miette!("no snapshot id given"))?;
	let snapshot = select_snapshot(&snapshots, wanted)?;
	info!(
		id = %snapshot.id,
		taken = ?snapshot.end_time.or(snapshot.start_time),
		"restoring snapshot",
	);

	// Plan the whole cycle before touching any data: every follower def must
	// have a pairable snapshot, or the restore refuses here.
	let followed = if args.no_followers {
		Vec::new()
	} else {
		plan_followers(&dir, &args.backup_type, snapshot, &snapshots).await?
	};
	if args.target.is_some() && !followed.is_empty() {
		bail!(
			"--target redirects only '{}', but its followers ({}) would still restore over \
			 their live paths; pass --no-followers and restore them separately if needed",
			args.backup_type,
			followed
				.iter()
				.map(|(follower_def, _)| follower_def.r#type.as_str())
				.collect::<Vec<_>>()
				.join(", "),
		);
	}
	for (follower_def, follower_snapshot) in &followed {
		info!(
			backup_type = %follower_def.r#type,
			id = %follower_snapshot.id,
			taken = ?follower_snapshot.end_time.or(follower_snapshot.start_time),
			"paired follower snapshot to restore after this one",
		);
	}

	// Sample the restore's S3 traffic to Canopy while it runs, so a long download
	// shows progress. A restore has no engine cell and no freeze moment: the bytes
	// received are the "is it moving" signal.
	let reporter = ProgressReporter::spawn(
		client.clone(),
		run_id,
		args.backup_type.clone(),
		BackupPurpose::Restore,
		proxy.traffic_handle(),
		None,
	);

	// Perform the restore, capturing the outcome so it can be reported to canopy
	// whether it succeeds or fails.
	let outcome = run_restore(&kopia, &s3env, snapshot, &def, args.target.as_deref(), args.clobber).await;

	// Stop sampling before the final report.
	reporter.stop().await;

	report_restore(
		&client,
		run_id,
		&args.backup_type,
		&outcome,
		&snapshot.id,
		proxy.traffic(),
	)
	.await;
	outcome?;

	// The followers, each a full restore session of its own, after the type they
	// follow: a store def resolving its target through `path_command` reads the
	// database restored just above.
	for (follower_def, follower_snapshot) in &followed {
		restore_follower(
			&client,
			&server_id,
			&target,
			follower_def,
			follower_snapshot,
			args.clobber,
		)
		.await?;
	}
	Ok(())
}

/// Restore one follower snapshot: its own credentials, run id, and report,
/// exactly as if `restore <type> <id>` had been invoked for it.
async fn restore_follower(
	client: &Arc<CanopyClient>,
	server_id: &str,
	target: &BackupTarget,
	def: &config::BackupDef,
	snapshot: &Snapshot,
	clobber: bool,
) -> Result<()> {
	info!(backup_type = %def.r#type, id = %snapshot.id, "restoring follower snapshot");
	let run_id = Uuid::new_v4();
	let proxy = spawn_proxy(
		client.clone(),
		def.r#type.clone(),
		BackupPurpose::Restore,
		&target.region,
		run_id,
	)
	.await?;
	let config_dir = transient_config_dir()?;
	let config_path = config_dir.path().join("repository.config");
	let s3env = S3KopiaEnv {
		password: &target.repo_password.0,
		config_path: &config_path,
	};
	let kopia = find_kopia_binary(None).ok_or_else(|| miette!("could not find the kopia binary"))?;
	connect_repo(
		&kopia,
		&s3env,
		target,
		&proxy.endpoint(),
		server_id,
		RunAs::CurrentUser,
	)
	.await?;

	let reporter = ProgressReporter::spawn(
		client.clone(),
		run_id,
		def.r#type.clone(),
		BackupPurpose::Restore,
		proxy.traffic_handle(),
		None,
	);
	let outcome = run_restore(&kopia, &s3env, snapshot, def, None, clobber).await;
	reporter.stop().await;
	report_restore(client, run_id, &def.r#type, &outcome, &snapshot.id, proxy.traffic()).await;
	outcome
}

/// Report a restore run to canopy so it shows up in the fleet table. The
/// restore's own outcome is what the command returns; a reporting failure is
/// only warned.
async fn report_restore(
	client: &CanopyClient,
	run_id: Uuid,
	backup_type: &str,
	outcome: &Result<()>,
	snapshot_id: &str,
	traffic: TrafficStats,
) {
	let to_i64 = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
	let report = ReportArgs::builder()
		.run_id(run_id)
		.type_(backup_type.to_owned())
		.purpose(BackupPurpose::Restore)
		.outcome(if outcome.is_ok() {
			RunOutcome::Success
		} else {
			RunOutcome::Failure
		})
		.maybe_error(outcome.as_ref().err().map(trim_error))
		.snapshot_id(snapshot_id.to_owned())
		.s3_sent_raw_bytes(to_i64(traffic.sent_raw))
		.s3_sent_payload_bytes(to_i64(traffic.sent_payload))
		.s3_received_raw_bytes(to_i64(traffic.received_raw))
		.s3_received_payload_bytes(to_i64(traffic.received_payload))
		.build();
	if let Err(err) = client.backup_report(&report).await {
		warn!("failed to report the restore to canopy: {err}");
	}
}

/// Run the kopia restore into a staging dir and lay it down via the def's method.
async fn run_restore(
	kopia: &Path,
	s3env: &S3KopiaEnv<'_>,
	snapshot: &Snapshot,
	def: &config::BackupDef,
	target_override: Option<&Path>,
	clobber: bool,
) -> Result<()> {
	// Restore into a staging dir colocated with the target's filesystem.
	let staging = def
		.method
		.staging_dir(target_override, std::process::id())
		.await?;
	if staging.exists() {
		tokio::fs::remove_dir_all(&staging).await.ok();
	}

	// The download needs room for the whole snapshot on the staging volume. Check
	// up front and, if short, let the operator free space and retry rather than
	// fail deep into the download.
	ensure_free_space(&staging, snapshot.total_size()).await?;

	let mut restore_cmd = build_kopia_command_with_s3(kopia, s3env, RunAs::CurrentUser)
		.map_err(|e| miette!("{e}"))?;
	// Force kopia's progress display (a large restore is otherwise a silent wait)
	// and run it against the inherited terminal so it's actually visible.
	restore_cmd.arg("--progress");
	args_snapshot_restore(&mut restore_cmd, &snapshot.id, &staging);
	run_kopia_visible(restore_cmd, "snapshot restore").await?;

	let clobber = clobber || confirm_clobber_interactively(&def.r#type)?;
	let opts = RestoreOpts {
		target: target_override.map(Path::to_path_buf),
		clobber,
	};
	def.method.restore(&staging, &opts).await
}

/// Restore from a capture held on this device.
///
/// The capture is copied into the same staging area a downloaded snapshot lands
/// in, and the method's restore then proceeds exactly as it does for one — so
/// there is one restore path, not two. Copying rather than moving the capture
/// into place is what lets the hold outlive the restore.
async fn restore_from_hold(
	hold_id: &str,
	def: &config::BackupDef,
	args: &RestoreArgs,
) -> Result<()> {
	let record = hold::load(hold_id).await?;
	if record.backup_type != args.backup_type {
		bail!(
			"hold {hold_id} holds a '{}' capture, not '{}'",
			record.backup_type,
			args.backup_type
		);
	}
	if !hold::capture_present(&record.capture).await {
		bail!(
			"the capture behind hold {hold_id} is gone, so it is not a rollback point; \
			 drop it with `bestool canopy hold drop {hold_id}` and restore from the repository"
		);
	}
	info!(
		hold = %hold_id,
		source = %record.source.display(),
		frozen = ?record.taken_at,
		"restoring from a held capture",
	);

	let staging = def
		.method
		.staging_dir(args.target.as_deref(), std::process::id())
		.await?;
	if staging.exists() {
		tokio::fs::remove_dir_all(&staging).await.ok();
	}

	// The staged copy is the only new allocation the restore makes — the tree it
	// displaces is renamed aside on the same filesystem, not copied — but it is a
	// whole second copy of the cluster, so check for it before starting rather
	// than failing partway through a restore an operator is depending on.
	let needed = i64::try_from(space::dir_size(&record.source).await).ok();
	ensure_free_space(&staging, needed).await?;

	copy_capture(&record.source, &staging).await?;

	let clobber = args.clobber || confirm_clobber_interactively(&args.backup_type)?;
	let opts = RestoreOpts {
		target: args.target.clone(),
		clobber,
	};
	def.method.restore(&staging, &opts).await
}

/// Copy the held capture's tree into `staging`, preserving what the restore
/// needs. The capture is read-only (a volume snapshot) and stays untouched.
#[cfg(unix)]
async fn copy_capture(source: &std::path::Path, staging: &std::path::Path) -> Result<()> {
	tokio::fs::create_dir_all(staging)
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("creating {}", staging.display()))?;
	let mut from = source.as_os_str().to_owned();
	from.push("/.");
	let status = tokio::process::Command::new("cp")
		.arg("-a")
		.arg(&from)
		.arg(staging)
		.stdin(std::process::Stdio::null())
		.status()
		.await
		.into_diagnostic()
		.wrap_err("spawning cp")?;
	if !status.success() {
		bail!("copying the held capture into {} failed ({status})", staging.display());
	}
	Ok(())
}

#[cfg(windows)]
async fn copy_capture(source: &std::path::Path, staging: &std::path::Path) -> Result<()> {
	let status = tokio::process::Command::new("robocopy")
		.arg(source)
		.arg(staging)
		// Mirror the tree with its ACLs, without retrying on a locked file: the
		// capture is a frozen read-only snapshot, so a file that can't be read
		// once won't become readable by waiting.
		.args(["/E", "/COPYALL", "/DCOPY:DAT", "/R:0", "/W:0", "/NFL", "/NDL", "/NP"])
		.stdin(std::process::Stdio::null())
		.status()
		.await
		.into_diagnostic()
		.wrap_err("spawning robocopy")?;
	// robocopy reports what it did in the exit code: below 8 is success (files
	// copied, extras present, and so on); 8 and above is a genuine failure.
	if status.code().is_none_or(|code| code >= 8) {
		bail!("copying the held capture into {} failed ({status})", staging.display());
	}
	Ok(())
}

/// Plan the follower restores: for each def that (transitively) follows the
/// restored type, the snapshot paired with its leader's.
///
/// spec: BAK#restore
async fn plan_followers(
	dir: &Path,
	backup_type: &str,
	primary: &Snapshot,
	snapshots: &[Snapshot],
) -> Result<Vec<(config::BackupDef, Snapshot)>> {
	let mut plan = Vec::new();
	let mut visited = std::collections::BTreeSet::from([backup_type.to_owned()]);
	let mut leaders =
		std::collections::VecDeque::from([(backup_type.to_owned(), primary.clone())]);
	while let Some((leader_type, leader_snapshot)) = leaders.pop_front() {
		for follower_def in config::followers_of(dir, &leader_type).await? {
			if !visited.insert(follower_def.r#type.clone()) {
				continue;
			}
			let follower_snapshot =
				select_paired(snapshots, &leader_snapshot, &follower_def.r#type)?.clone();
			leaders.push_back((follower_def.r#type.clone(), follower_snapshot.clone()));
			plan.push((follower_def, follower_snapshot));
		}
	}
	Ok(plan)
}

/// Select the snapshot paired with `leader` for a follower type: the earliest
/// snapshot of that type from the same source host taken at or after the
/// leader. A later one is a safe superset of what the leader's data
/// references; an earlier one is not, and is never selected.
///
/// spec: BAK#restore
fn select_paired<'a>(
	snapshots: &'a [Snapshot],
	leader: &Snapshot,
	follower_type: &str,
) -> Result<&'a Snapshot> {
	let Some(leader_start) = leader.start_time else {
		bail!(
			"snapshot {} has no start time, so a '{follower_type}' snapshot cannot be paired with it; \
			 restore '{follower_type}' explicitly by id, or skip it with --no-followers",
			leader.id
		);
	};
	let mut candidates: Vec<&Snapshot> = snapshots
		.iter()
		.filter(|s| s.id != leader.id && s.source.host == leader.source.host)
		.filter(|s| is_of_type(s, follower_type))
		.collect();
	candidates.sort_by_key(|s| s.start_time);
	candidates
		.into_iter()
		.find(|s| s.start_time.is_some_and(|t| t >= leader_start))
		.ok_or_else(|| {
			miette!(
				"no '{follower_type}' snapshot from host {} taken at or after {leader_start} \
				 (an earlier one could lack content the restored data references); \
				 take a new '{follower_type}' backup first, restore it explicitly by id, \
				 or skip it with --no-followers",
				leader.source.host
			)
		})
}

/// Whether a snapshot is a capture of `backup_type`: its description carries
/// the type (set at create since followers exist), or its source is the
/// type-keyed view path the simple method exposes on Linux. Tags would be the
/// natural signal but don't round-trip through `kopia snapshot list`.
fn is_of_type(snapshot: &Snapshot, backup_type: &str) -> bool {
	if snapshot.description == backup_type {
		return true;
	}
	let path = snapshot.source.path.replace('\\', "/");
	path.ends_with(&format!("backup-source/{backup_type}"))
}

/// Error unless the volume backing `staging` has room for `needed` bytes (plus a
/// little headroom), prompting the operator to free space and retry on an
/// interactive terminal. Skipped when the snapshot size is unknown.
async fn ensure_free_space(staging: &std::path::Path, needed: Option<i64>) -> Result<()> {
	let Some(needed) = needed.filter(|n| *n > 0).map(|n| n as u64) else {
		return Ok(()); // unknown size (no root summary): nothing to check against
	};
	// 5% headroom for filesystem overhead and rounding. The swap into place is a
	// rename (the old data is kept as `.old` in place), so only the staging copy
	// consumes new space.
	let required = needed.saturating_add(needed / 20);
	// Check the parent: `staging` itself doesn't exist yet.
	let volume = staging.parent().unwrap_or(staging).to_path_buf();
	crate::interactive::retry("ensuring enough free disk space", async || {
		let available = fs4::available_space(&volume)
			.into_diagnostic()
			.wrap_err_with(|| format!("checking free space on {}", volume.display()))?;
		if available >= required {
			return Ok(());
		}
		bail!(
			"restoring needs about {} free on {} but only {} is available; free up space and retry",
			human_bytes(required),
			volume.display(),
			human_bytes(available),
		)
	})
	.await
}

/// A rough human-readable byte size (binary units), for operator-facing messages.
fn human_bytes(bytes: u64) -> String {
	const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
	let mut value = bytes as f64;
	let mut unit = 0;
	while value >= 1024.0 && unit < UNITS.len() - 1 {
		value /= 1024.0;
		unit += 1;
	}
	if unit == 0 {
		format!("{bytes} B")
	} else {
		format!("{value:.1} {}", UNITS[unit])
	}
}

async fn list_snapshots(kopia: &std::path::Path, s3env: &S3KopiaEnv<'_>) -> Result<Vec<Snapshot>> {
	let mut cmd = build_kopia_command_with_s3(kopia, s3env, RunAs::CurrentUser)
		.map_err(|e| miette!("{e}"))?;
	args_snapshot_list(&mut cmd);
	let stdout = run_kopia(cmd, "snapshot list").await?;
	serde_json::from_str(stdout.trim())
		.into_diagnostic()
		.wrap_err("parsing kopia snapshot list")
}

/// Pick the snapshot to restore from the repo's list by id prefix.
///
/// Selection is by snapshot id alone. It is deliberately not scoped to the
/// local server — a restore is typically onto a different (rebuilt or
/// replacement) host, so the snapshot's source host is not this server's id —
/// nor gated on the `canopy-type` tag: kopia's `snapshot list` does not echo
/// the tags set at create time, so every listed snapshot deserialises with no
/// tags and gating on them would reject every snapshot. The backup type still
/// selects the def, method, and credentials in the caller.
fn select_snapshot<'a>(snapshots: &'a [Snapshot], id: &str) -> Result<&'a Snapshot> {
	let mut hits = snapshots.iter().filter(|s| s.id.starts_with(id));
	let Some(first) = hits.next() else {
		bail!(
			"no snapshot matching id '{id}' in the repository{}",
			available_snapshots_hint(snapshots)
		);
	};
	if hits.next().is_some() {
		bail!("snapshot id '{id}' is ambiguous; give more characters");
	}
	Ok(first)
}

/// Describe what the connected repository actually holds, appended to the
/// "no match" error so the operator can see the ids available rather than
/// guess. Newest first, capped so the message stays readable.
fn available_snapshots_hint(snapshots: &[Snapshot]) -> String {
	if snapshots.is_empty() {
		return "; the repository has no snapshots".to_owned();
	}
	const MAX: usize = 20;
	let mut sorted: Vec<&Snapshot> = snapshots.iter().collect();
	sorted.sort_by_key(|s| std::cmp::Reverse(s.end_time.or(s.start_time)));
	let mut out = format!("; {} snapshot(s) available (id, source, taken):", snapshots.len());
	for s in sorted.iter().take(MAX) {
		let taken = s
			.end_time
			.or(s.start_time)
			.map_or_else(|| "unknown".to_owned(), |t| t.to_string());
		out.push_str(&format!("\n  {} {} {taken}", s.id, s.source.host));
	}
	if snapshots.len() > MAX {
		out.push_str(&format!("\n  … and {} more", snapshots.len() - MAX));
	}
	out
}

/// Interactive double-confirmation for a destructive restore. Returns `true`
/// only when both prompts pass. With no TTY, returns `false` (the caller then
/// relies on the explicit flag / the clobber guard).
fn confirm_clobber_interactively(backup_type: &str) -> Result<bool> {
	if !std::io::stdin().is_terminal() {
		return Ok(false);
	}
	print!(
		"This will OVERWRITE existing data for '{backup_type}'. Continue? [y/N] "
	);
	std::io::stdout().flush().ok();
	if !read_line()?.trim().eq_ignore_ascii_case("y") {
		return Ok(false);
	}
	print!("Type the backup type '{backup_type}' to confirm: ");
	std::io::stdout().flush().ok();
	Ok(read_line()?.trim() == backup_type)
}

fn read_line() -> Result<String> {
	let mut buf = String::new();
	std::io::stdin()
		.read_line(&mut buf)
		.into_diagnostic()
		.wrap_err("reading confirmation")?;
	Ok(buf)
}

#[cfg(test)]
mod tests {
	use bestool_kopia::SnapshotSource;

	use super::*;

	/// A snapshot as kopia's `snapshot list --json` actually emits it: source
	/// host and id, but no tags (kopia does not echo the create-time tags).
	fn snap(id: &str, host: &str, end: Option<&str>) -> Snapshot {
		Snapshot {
			id: id.into(),
			source: SnapshotSource {
				host: host.into(),
				user_name: "canopy".into(),
				path: "/x".into(),
			},
			description: String::new(),
			start_time: None,
			end_time: end.map(|t| t.parse().unwrap()),
			tags: std::collections::BTreeMap::new(),
			root_entry: None,
		}
	}

	#[test]
	fn selects_by_id_prefix() {
		let snaps = vec![snap("abc123", "srv", None)];
		let chosen = select_snapshot(&snaps, "abc").unwrap();
		assert_eq!(chosen.id, "abc123");
	}

	#[test]
	fn selects_when_kopia_omits_tags() {
		// The regression: kopia's snapshot list returns no tags, so gating on the
		// canopy-type tag rejected every snapshot. Selection by id must still find
		// a valid, correct id.
		let snaps = vec![snap("99f1f3f6e25f483b5196d61d2f28a871", "srv", None)];
		let chosen = select_snapshot(&snaps, "99f1f3f6e25f483b5196d61d2f28a871").unwrap();
		assert_eq!(chosen.id, "99f1f3f6e25f483b5196d61d2f28a871");
	}

	#[test]
	fn selects_across_hosts() {
		// A restore onto a different host: the snapshot's source host is not this
		// server's id, but selection by id must still find it.
		let snaps = vec![snap("abc123", "other-host", None)];
		let chosen = select_snapshot(&snaps, "abc").unwrap();
		assert_eq!(chosen.id, "abc123");
	}

	#[test]
	fn errors_on_ambiguous_prefix() {
		let snaps = vec![snap("abc123", "srv", None), snap("abc456", "other", None)];
		assert!(select_snapshot(&snaps, "abc").is_err());
	}

	#[test]
	fn errors_when_no_match_lists_available() {
		let snaps = vec![snap("abc", "srv", Some("2026-01-01T00:00:00Z"))];
		let err = select_snapshot(&snaps, "zzz").unwrap_err().to_string();
		assert!(err.contains("no snapshot matching id 'zzz'"));
		assert!(err.contains("abc"));
	}

	#[test]
	fn errors_when_repository_empty() {
		let snaps: Vec<Snapshot> = vec![];
		let err = select_snapshot(&snaps, "abc").unwrap_err().to_string();
		assert!(err.contains("no snapshots"));
	}

	#[test]
	fn human_bytes_scales_units() {
		assert_eq!(human_bytes(512), "512 B");
		assert_eq!(human_bytes(1024), "1.0 KiB");
		assert_eq!(human_bytes(8 * 1024 * 1024 * 1024), "8.0 GiB");
	}

	#[tokio::test]
	async fn ensure_free_space_skips_unknown_size() {
		// No snapshot size: nothing to check, so it never touches the filesystem.
		ensure_free_space(std::path::Path::new("/nonexistent/staging"), None)
			.await
			.unwrap();
	}

	/// A snapshot with the fields follower pairing reads: description (the
	/// backup type), source host/path, and start time.
	fn typed_snap(id: &str, host: &str, description: &str, start: Option<&str>) -> Snapshot {
		Snapshot {
			id: id.into(),
			source: SnapshotSource {
				host: host.into(),
				user_name: "canopy".into(),
				path: "/x".into(),
			},
			description: description.into(),
			start_time: start.map(|t| t.parse().unwrap()),
			end_time: None,
			tags: std::collections::BTreeMap::new(),
			root_entry: None,
		}
	}

	#[test]
	fn pairs_earliest_follower_at_or_after_leader() {
		let leader = typed_snap("db1", "srv", "tamanu-postgres", Some("2026-08-01T03:00:00Z"));
		let snaps = vec![
			typed_snap("blob-early", "srv", "tamanu-blobs", Some("2026-08-01T02:00:00Z")),
			typed_snap("blob-next", "srv", "tamanu-blobs", Some("2026-08-01T03:10:00Z")),
			typed_snap("blob-later", "srv", "tamanu-blobs", Some("2026-08-02T03:10:00Z")),
			leader.clone(),
		];
		let paired = select_paired(&snaps, &leader, "tamanu-blobs").unwrap();
		assert_eq!(paired.id, "blob-next");
	}

	#[test]
	fn pairs_at_identical_start_time() {
		let leader = typed_snap("db1", "srv", "tamanu-postgres", Some("2026-08-01T03:00:00Z"));
		let snaps = vec![
			typed_snap("blob-same", "srv", "tamanu-blobs", Some("2026-08-01T03:00:00Z")),
			leader.clone(),
		];
		assert_eq!(
			select_paired(&snaps, &leader, "tamanu-blobs").unwrap().id,
			"blob-same"
		);
	}

	#[test]
	fn refuses_an_earlier_follower() {
		// Only an earlier store capture exists: it may lack content the restored
		// database references, so it is never selected.
		let leader = typed_snap("db1", "srv", "tamanu-postgres", Some("2026-08-01T03:00:00Z"));
		let snaps = vec![
			typed_snap("blob-early", "srv", "tamanu-blobs", Some("2026-08-01T02:00:00Z")),
			leader.clone(),
		];
		let err = select_paired(&snaps, &leader, "tamanu-blobs")
			.unwrap_err()
			.to_string();
		assert!(err.contains("at or after"));
		assert!(err.contains("--no-followers"));
	}

	#[test]
	fn pairing_ignores_other_hosts_and_types() {
		// The repository is shared by the whole group: another server's store
		// snapshots and other types on this server must not pair.
		let leader = typed_snap("db1", "srv", "tamanu-postgres", Some("2026-08-01T03:00:00Z"));
		let snaps = vec![
			typed_snap("blob-elsewhere", "other-srv", "tamanu-blobs", Some("2026-08-01T03:10:00Z")),
			typed_snap("db2", "srv", "tamanu-postgres", Some("2026-08-01T03:10:00Z")),
			leader.clone(),
		];
		assert!(select_paired(&snaps, &leader, "tamanu-blobs").is_err());
	}

	#[test]
	fn refuses_pairing_without_a_leader_start_time() {
		let leader = typed_snap("db1", "srv", "tamanu-postgres", None);
		let snaps = vec![
			typed_snap("blob", "srv", "tamanu-blobs", Some("2026-08-01T03:10:00Z")),
			leader.clone(),
		];
		assert!(select_paired(&snaps, &leader, "tamanu-blobs").is_err());
	}

	#[test]
	fn classifies_by_view_path_when_description_is_absent() {
		let mut snapshot = typed_snap("blob", "srv", "", Some("2026-08-01T03:10:00Z"));
		snapshot.source.path = "/var/cache/bestool/backup-source/tamanu-blobs".into();
		assert!(is_of_type(&snapshot, "tamanu-blobs"));
		assert!(!is_of_type(&snapshot, "tamanu-postgres"));

		let mut windows = typed_snap("blob", "srv", "", Some("2026-08-01T03:10:00Z"));
		windows.source.path = r"C:\ProgramData\bestool\backup-source\tamanu-blobs".into();
		assert!(is_of_type(&windows, "tamanu-blobs"));

		let in_place = typed_snap("blob", "srv", "", Some("2026-08-01T03:10:00Z"));
		assert!(!is_of_type(&in_place, "tamanu-blobs"));
	}
}
