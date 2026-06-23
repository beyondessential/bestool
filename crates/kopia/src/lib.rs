//! Shared helpers for interacting with the kopia CLI.
//!
//! Used by `bestool-tamanu` (for the `kopia_backup` doctor check) and by
//! `bestool` (for the `bestool kopia` subcommand suite). Has nothing
//! tamanu-specific in it.
//!
//! Highlights:
//! - [`find_kopia_binary`] / [`find_windows_kopia_binary`] /
//!   [`find_windows_kopia_config`]: locate kopia and (on Windows) the per-user
//!   repository config from KopiaUI's standard install locations.
//! - [`linux_elevation`]: decide whether/how to elevate to the `kopia` system
//!   user on Linux. Returns [`Elevation::Sudo`] when we're not the kopia user
//!   and the system kopia install is present, [`Elevation::Direct`] when we
//!   already have access, [`Elevation::Skip`] otherwise.
//! - [`Snapshot`] and [`fetch_snapshots`]: deserialise `kopia snapshot list
//!   --json` output into a typed shape.
//! - [`SnapshotFilter`] / [`build_filter`]: in-process filtering of a snapshot
//!   list by host, tag, path substring, and time window.
//! - With the `cli` feature: [`SnapshotSelectorArgs`] (a `clap::Args`-derived
//!   struct that consumer commands flatten into their own args) and
//!   [`select_snapshot`] (a `dialoguer`-backed interactive picker).

use std::{
	collections::BTreeMap,
	path::{Path, PathBuf},
	process::Command,
};

use jiff::{Span, Timestamp};
use miette::{Context as _, IntoDiagnostic as _, Result, miette};
use serde::{Deserialize, Serialize};

/// System user that owns the Linux kopia install.
pub const LINUX_KOPIA_USER: &str = "kopia";

/// Standard location of the system kopia repository config on Linux. Owned
/// by the [`LINUX_KOPIA_USER`].
pub const LINUX_KOPIA_CONFIG: &str = "/var/lib/kopia/.config/kopia/repository.config";

/// Locate the kopia binary.
///
/// Order of preference:
///   1. An explicit override path (`None` to skip)
///   2. `kopia` (or `kopia.exe`) in `PATH`
///   3. On Windows, well-known KopiaUI bundled binary locations
pub fn find_kopia_binary(override_path: Option<&Path>) -> Option<PathBuf> {
	if let Some(p) = override_path {
		return Some(p.to_path_buf());
	}
	if let Some(p) = find_in_path("kopia") {
		return Some(p);
	}
	if cfg!(windows) {
		return find_windows_kopia_binary();
	}
	None
}

fn find_in_path(name: &str) -> Option<PathBuf> {
	let exe = if cfg!(windows) {
		format!("{name}.exe")
	} else {
		name.to_string()
	};
	let path = std::env::var_os("PATH")?;
	for dir in std::env::split_paths(&path) {
		let candidate = dir.join(&exe);
		if candidate.is_file() {
			return Some(candidate);
		}
	}
	None
}

/// Look for the kopia binary bundled with KopiaUI on Windows.
pub fn find_windows_kopia_binary() -> Option<PathBuf> {
	let mut candidates: Vec<PathBuf> = Vec::new();
	if let Ok(local) = std::env::var("LOCALAPPDATA") {
		candidates.push(
			Path::new(&local)
				.join("Programs")
				.join("KopiaUI")
				.join("resources")
				.join("server")
				.join("kopia.exe"),
		);
	}
	if let Ok(pf) = std::env::var("ProgramFiles") {
		candidates.push(
			Path::new(&pf)
				.join("KopiaUI")
				.join("resources")
				.join("server")
				.join("kopia.exe"),
		);
	}
	if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
		candidates.push(
			Path::new(&pf86)
				.join("KopiaUI")
				.join("resources")
				.join("server")
				.join("kopia.exe"),
		);
	}
	candidates.into_iter().find(|p| p.exists())
}

/// Standard per-user kopia repository config on Windows, used by KopiaUI.
pub fn find_windows_kopia_config() -> Option<PathBuf> {
	let appdata = std::env::var("APPDATA").ok()?;
	let config = Path::new(&appdata).join("kopia").join("repository.config");
	config.exists().then_some(config)
}

/// Current process's username (via `whoami`). `None` if `whoami` can't
/// determine it (rare).
pub fn current_username() -> Option<String> {
	whoami::username().ok()
}

/// What to do about elevation on Linux when we want to run kopia.
#[derive(Debug)]
pub enum Elevation {
	/// Run as the current user — either we're already the kopia user, or
	/// there's no system kopia install (the operator's running their own).
	Direct,
	/// Wrap the kopia invocation in `sudo -u kopia --`. Used whenever we're
	/// running as a different user and the system kopia install exists; if
	/// `sudo` isn't allowed (no NOPASSWD rule, no TTY), the resulting kopia
	/// invocation will fail and the caller surfaces that as a Skip.
	Sudo,
	/// We can't elevate. The caller should bail with a reason.
	Skip(String),
}

/// Decide how to invoke kopia on Linux given the current user and whether
/// the system kopia install is present (and accessible).
///
/// Logic:
/// - If we're the kopia user, run directly.
/// - Else, probe the system kopia config:
///   - Not found (ENOENT): no system install. Run directly as current user;
///     they're presumably running their own kopia under their own config.
///   - Permission denied (EACCES): exists, owned by kopia user. Elevate via
///     `sudo -u kopia`.
///   - Readable: exists and we can read it (unusual mode). Run directly.
#[cfg(target_os = "linux")]
pub fn linux_elevation() -> Elevation {
	let Some(user) = current_username() else {
		return Elevation::Skip("could not determine current Unix username".into());
	};

	if user == LINUX_KOPIA_USER {
		return Elevation::Direct;
	}

	match std::fs::metadata(LINUX_KOPIA_CONFIG) {
		Ok(_) => Elevation::Direct,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => Elevation::Direct,
		Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => Elevation::Sudo,
		Err(err) => Elevation::Skip(format!("checking {LINUX_KOPIA_CONFIG}: {err}")),
	}
}

#[cfg(not(target_os = "linux"))]
pub fn linux_elevation() -> Elevation {
	Elevation::Direct
}

/// Build a `Command` that runs the kopia binary, elevated to the kopia user
/// if the current platform/user requires it (Linux only).
///
/// On non-Linux platforms or when no elevation is needed, this is just
/// `Command::new(kopia)`. On Linux with [`Elevation::Sudo`], it returns
/// `sudo -u kopia -- <kopia>`. [`Elevation::Skip`] is propagated as an
/// `Err` whose message is the Skip reason.
pub fn build_kopia_command(kopia: &Path) -> Result<Command, String> {
	if cfg!(target_os = "linux") {
		match linux_elevation() {
			Elevation::Direct => Ok(Command::new(kopia)),
			Elevation::Sudo => {
				let mut c = Command::new("sudo");
				c.arg("-u").arg(LINUX_KOPIA_USER).arg("--").arg(kopia);
				Ok(c)
			}
			Elevation::Skip(reason) => Err(reason),
		}
	} else {
		Ok(Command::new(kopia))
	}
}

/// AWS environment variables that precede the container-credentials provider in
/// minio-go's resolution chain.
///
/// They're scrubbed from kopia's environment so they can't shadow the loopback
/// container-credentials endpoint the device serves. (`FULL_URI` and
/// `AUTHORIZATION_TOKEN` are *not* here — those are the endpoint we point kopia
/// at.)
pub const S3_SHADOWING_ENV_VARS: [&str; 7] = [
	"AWS_ACCESS_KEY_ID",
	"AWS_SECRET_ACCESS_KEY",
	"AWS_SESSION_TOKEN",
	"AWS_WEB_IDENTITY_TOKEN_FILE",
	"AWS_ROLE_ARN",
	"AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
	"AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
];

/// Environment for a kopia run that gets its S3 creds from a loopback
/// container-credentials endpoint.
///
/// `token` and `password` are secrets — passed via the environment (never argv)
/// so they don't show up in the process list. `config_path` points kopia at a
/// transient per-run config so the bucket/password never persist to the device's
/// kopia config.
pub struct S3KopiaEnv<'a> {
	/// `AWS_CONTAINER_CREDENTIALS_FULL_URI` (the loopback creds endpoint).
	pub full_uri: &'a str,
	/// `AWS_CONTAINER_AUTHORIZATION_TOKEN` (the leased bearer token).
	pub token: &'a str,
	/// `KOPIA_PASSWORD` (the repo passphrase).
	pub password: &'a str,
	/// `KOPIA_CONFIG_PATH` (a transient per-run config file).
	pub config_path: &'a Path,
}

impl S3KopiaEnv<'_> {
	/// The (key, value) pairs this env sets on a kopia process.
	fn vars(&self) -> [(&'static str, std::ffi::OsString); 4] {
		[
			("AWS_CONTAINER_CREDENTIALS_FULL_URI", self.full_uri.into()),
			("AWS_CONTAINER_AUTHORIZATION_TOKEN", self.token.into()),
			("KOPIA_PASSWORD", self.password.into()),
			("KOPIA_CONFIG_PATH", self.config_path.as_os_str().to_owned()),
		]
	}

	/// The keys to forward across a `sudo` env reset (`--preserve-env=…`).
	fn preserve_env_keys(&self) -> String {
		self.vars()
			.iter()
			.map(|(k, _)| *k)
			.collect::<Vec<_>>()
			.join(",")
	}
}

/// Build a kopia [`Command`] that gets its S3 creds from a loopback
/// container-credentials endpoint.
///
/// On Linux, kopia runs as the [`LINUX_KOPIA_USER`] via `sudo` when needed;
/// `sudo`'s `env_reset` drops the parent environment (so the
/// [`S3_SHADOWING_ENV_VARS`] never reach kopia) and the creds/password vars are
/// forwarded explicitly via `--preserve-env`. Run directly, the child inherits
/// our environment, so the shadowing vars are removed explicitly instead.
pub fn build_kopia_command_with_s3(kopia: &Path, env: &S3KopiaEnv<'_>) -> Result<Command, String> {
	let mut cmd = if cfg!(target_os = "linux") {
		match linux_elevation() {
			Elevation::Direct => Command::new(kopia),
			Elevation::Sudo => {
				let mut c = Command::new("sudo");
				c.arg(format!("--preserve-env={}", env.preserve_env_keys()));
				c.arg("-u").arg(LINUX_KOPIA_USER).arg("--").arg(kopia);
				c
			}
			Elevation::Skip(reason) => return Err(reason),
		}
	} else {
		Command::new(kopia)
	};

	// Under sudo, env_reset already dropped these; removing them here covers the
	// direct (inherited-environment) path.
	for key in S3_SHADOWING_ENV_VARS {
		cmd.env_remove(key);
	}
	for (key, value) in env.vars() {
		cmd.env(key, value);
	}
	Ok(cmd)
}

/// Push `repository connect s3` args (the canopy-managed repo connection).
pub fn args_repository_connect_s3(
	cmd: &mut Command,
	bucket: &str,
	prefix: &str,
	region: &str,
	username: &str,
	hostname: &str,
) {
	cmd.args(["repository", "connect", "s3"])
		.arg("--bucket")
		.arg(bucket)
		.arg("--prefix")
		.arg(prefix)
		.arg("--region")
		.arg(region)
		.arg("--override-username")
		.arg(username)
		.arg("--override-hostname")
		.arg(hostname);
}

/// Push `snapshot create --json` args, with each tag as `key:value`.
pub fn args_snapshot_create(cmd: &mut Command, path: &Path, tags: &BTreeMap<String, String>) {
	cmd.args(["snapshot", "create", "--json"]);
	for (key, value) in tags {
		cmd.arg("--tags").arg(format!("{key}:{value}"));
	}
	cmd.arg(path);
}

/// Push `snapshot list --json --all` args (every snapshot, all sources).
pub fn args_snapshot_list(cmd: &mut Command) {
	cmd.args(["snapshot", "list", "--json", "--all"]);
}

/// Push `snapshot restore` args (restore a snapshot id into `dest`).
pub fn args_snapshot_restore(cmd: &mut Command, snapshot_id: &str, dest: &Path) {
	cmd.args(["snapshot", "restore", snapshot_id]).arg(dest);
}

/// Push `policy set --add-ignore=… <path>` args (ignore transient files at a source).
pub fn args_policy_set_ignores(cmd: &mut Command, path: &Path, ignores: &[String]) {
	cmd.args(["policy", "set"]);
	for glob in ignores {
		cmd.arg(format!("--add-ignore={glob}"));
	}
	cmd.arg(path);
}

/// A single kopia snapshot, as emitted by `kopia snapshot list --json`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
	pub id: String,
	pub source: SnapshotSource,
	#[serde(default)]
	pub description: String,
	#[serde(default)]
	pub start_time: Option<Timestamp>,
	#[serde(default)]
	pub end_time: Option<Timestamp>,
	#[serde(default)]
	pub tags: BTreeMap<String, String>,
	#[serde(default)]
	pub root_entry: Option<RootEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SnapshotSource {
	#[serde(default)]
	pub host: String,
	#[serde(default, rename = "userName")]
	pub user_name: String,
	#[serde(default)]
	pub path: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RootEntry {
	#[serde(default, rename = "summ")]
	pub summary: Option<DirSummary>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DirSummary {
	#[serde(default, rename = "size")]
	pub total_size: i64,
	#[serde(default, rename = "files")]
	pub total_files: i64,
	#[serde(default, rename = "dirs")]
	pub total_dirs: i64,
}

impl Snapshot {
	/// Time at which the snapshot finished (or started, if `endTime` is missing).
	pub fn taken_at(&self) -> Option<Timestamp> {
		self.end_time.or(self.start_time)
	}

	/// Best-effort total size of the snapshot's contents.
	pub fn total_size(&self) -> Option<i64> {
		self.root_entry
			.as_ref()
			.and_then(|r| r.summary.as_ref())
			.map(|s| s.total_size)
	}
}

/// Filter criteria used by listing/restore/mount.
#[derive(Debug, Default, Clone)]
pub struct SnapshotFilter {
	/// `None` means "any host". `Some(name)` filters source.host == name.
	pub source_host: Option<String>,
	/// All entries must match.
	pub tags: BTreeMap<String, String>,
	/// Source path must contain this substring (case-insensitive).
	pub path_substr: Option<String>,
	/// Snapshot's taken_at must be within this Span from now.
	pub since: Option<Span>,
	/// Cap to the N most recent snapshots after the other filters apply.
	pub limit: Option<usize>,
}

impl SnapshotFilter {
	/// Apply the filter to a list of snapshots, returning matches sorted
	/// newest-first.
	pub fn apply(&self, snapshots: &[Snapshot], now: Timestamp) -> Vec<Snapshot> {
		let cutoff: Option<Timestamp> = self.since.and_then(|span| now.checked_sub(span).ok());

		let path_substr_lc = self.path_substr.as_ref().map(|s| s.to_lowercase());

		let mut matches: Vec<Snapshot> = snapshots
			.iter()
			.filter(|s| {
				if let Some(host) = &self.source_host
					&& s.source.host != *host
				{
					return false;
				}
				for (k, v) in &self.tags {
					if s.tags.get(k) != Some(v) {
						return false;
					}
				}
				if let Some(needle) = &path_substr_lc
					&& !s.source.path.to_lowercase().contains(needle)
				{
					return false;
				}
				if let Some(cutoff) = cutoff
					&& s.taken_at().is_none_or(|t| t < cutoff)
				{
					return false;
				}
				true
			})
			.cloned()
			.collect();

		matches.sort_by_key(|s| std::cmp::Reverse(s.taken_at()));

		if let Some(n) = self.limit {
			matches.truncate(n);
		}
		matches
	}
}

/// Parse a `key:value` tag spec from a `--tag` flag.
pub fn parse_tag_kv(s: &str) -> Result<(String, String), String> {
	s.split_once(':')
		.map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
		.filter(|(k, v)| !k.is_empty() && !v.is_empty())
		.ok_or_else(|| format!("expected KEY:VALUE, got `{s}`"))
}

/// Build a [`SnapshotFilter`] from CLI-shaped inputs. `all = true` drops any
/// source-host filter.
pub fn build_filter(
	all: bool,
	source_host: Option<String>,
	default_host: Option<String>,
	tags: &[String],
	path: Option<String>,
	since: Option<&str>,
	limit: Option<usize>,
) -> Result<SnapshotFilter> {
	let source_host = if all {
		None
	} else {
		source_host.or(default_host)
	};

	let mut tag_map = BTreeMap::new();
	for raw in tags {
		let (k, v) = parse_tag_kv(raw).map_err(|e| miette!("invalid --tag: {e}"))?;
		tag_map.insert(k, v);
	}

	let since = since
		.map(|s| {
			s.parse::<Span>()
				.map_err(|e| miette!("invalid --since duration `{s}`: {e}"))
		})
		.transpose()?;

	Ok(SnapshotFilter {
		source_host,
		tags: tag_map,
		path_substr: path,
		since,
		limit,
	})
}

/// Run `kopia snapshot list --json --all` and parse the result.
///
/// `bin` is expected to already be wrapped by [`build_kopia_command`] if
/// elevation was needed — callers typically run that first and pass the
/// resulting binary path here.
pub fn fetch_snapshots(bin: &Path) -> Result<Vec<Snapshot>> {
	let output = Command::new(bin)
		.args(["snapshot", "list", "--json", "--all"])
		.env("KOPIA_CHECK_FOR_UPDATES", "false")
		.output()
		.into_diagnostic()
		.wrap_err_with(|| format!("invoking {}", bin.display()))?;
	if !output.status.success() {
		let stderr = String::from_utf8_lossy(&output.stderr);
		return Err(miette!(
			"kopia snapshot list exited {}: {}",
			output.status,
			stderr.trim()
		));
	}
	serde_json::from_slice(&output.stdout)
		.into_diagnostic()
		.wrap_err("decoding kopia snapshot list JSON")
}

/// Kopia's manifest IDs are long hex. The short prefix is enough to identify
/// a snapshot in a list; kopia restore/mount accept short prefixes too.
pub fn short_id(id: &str) -> String {
	const SHORT: usize = 16;
	if id.len() <= SHORT {
		id.to_string()
	} else {
		id.chars().take(SHORT).collect()
	}
}

/// Format a snapshot timestamp for human display.
pub fn format_taken(ts: Timestamp) -> String {
	ts.strftime("%Y-%m-%d %H:%M").to_string()
}

/// Render a snapshot's tag map as a `k=v, k2=v2` string (sorted by key).
pub fn format_tags(tags: &BTreeMap<String, String>) -> String {
	tags.iter()
		.map(|(k, v)| format!("{k}={v}"))
		.collect::<Vec<_>>()
		.join(", ")
}

/// One-line summary of a snapshot, suitable for an interactive picker.
pub fn format_snapshot_line(snap: &Snapshot) -> String {
	let taken = snap
		.taken_at()
		.map(format_taken)
		.unwrap_or_else(|| "—".into());
	let source = format!(
		"{}@{}:{}",
		snap.source.user_name, snap.source.host, snap.source.path
	);
	let size = snap
		.total_size()
		.map(human_bytes)
		.unwrap_or_else(|| "—".into());
	let tags = format_tags(&snap.tags);
	if tags.is_empty() {
		format!("{}  {taken}  {source}  {size}", short_id(&snap.id))
	} else {
		format!(
			"{}  {taken}  {source}  {size}  [{tags}]",
			short_id(&snap.id)
		)
	}
}

/// Human-readable size formatter.
pub fn human_bytes(b: i64) -> String {
	if b < 0 {
		return "?".into();
	}
	const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
	let mut value = b as f64;
	let mut unit = 0;
	while value >= 1024.0 && unit < UNITS.len() - 1 {
		value /= 1024.0;
		unit += 1;
	}
	if unit == 0 {
		format!("{}{}", b, UNITS[0])
	} else {
		format!("{value:.1}{}", UNITS[unit])
	}
}

#[cfg(feature = "cli")]
pub use cli::*;

#[cfg(feature = "cli")]
mod cli {
	use clap::Args;
	use dialoguer::Select;
	use miette::{Context as _, IntoDiagnostic as _, Result, bail};

	use super::*;

	/// Shared snapshot-selection flags for commands that operate on a single
	/// snapshot (`restore`, `mount`, …). Flatten into each command's args
	/// struct via `#[command(flatten)]`.
	#[derive(Debug, Clone, Args)]
	pub struct SnapshotSelectorArgs {
		/// Snapshot ID (full or short prefix). Without this or `--latest`,
		/// the command opens an interactive picker.
		#[arg(long, value_name = "ID")]
		pub snapshot: Option<String>,

		/// Use the newest matching snapshot without prompting.
		///
		/// Requires at least one of `--tag` or `--path` so the "newest" is
		/// unambiguous — a kopia repo holds many kinds of snapshots and "the
		/// latest one for this host" would otherwise pick whichever ran most
		/// recently, regardless of what it was backing up.
		#[arg(long, conflicts_with = "snapshot")]
		pub latest: bool,

		/// Filter: source host. Defaults to this host.
		#[arg(long, value_name = "HOST", conflicts_with = "all")]
		pub source_host: Option<String>,

		/// Filter: list snapshots from every host.
		#[arg(long, conflicts_with = "source_host")]
		pub all: bool,

		/// Filter: tag. Repeatable. Format: `key:value`.
		#[arg(long = "tag", value_name = "KEY:VALUE", value_parser = parse_tag_arg)]
		pub tags: Vec<String>,

		/// Filter: source path substring (case-insensitive).
		#[arg(long, value_name = "SUBSTR")]
		pub path: Option<String>,

		/// Filter: only snapshots within this duration (e.g. `24h`, `7d`).
		#[arg(long, value_name = "DURATION")]
		pub since: Option<String>,
	}

	fn parse_tag_arg(s: &str) -> Result<String, String> {
		parse_tag_kv(s).map(|_| s.to_string())
	}

	impl SnapshotSelectorArgs {
		/// Resolve to a single snapshot: explicit ID, `--latest` over
		/// filters, or interactive picker over filters. `default_host` is
		/// the host to filter on when neither `--source-host` nor `--all` is
		/// given (typically the current hostname).
		pub fn resolve(
			&self,
			bin: &std::path::Path,
			default_host: Option<String>,
			picker_prompt: &str,
		) -> Result<Snapshot> {
			use std::io::IsTerminal as _;

			if let Some(id) = &self.snapshot {
				return resolve_by_id(bin, id);
			}

			if self.latest && self.tags.is_empty() && self.path.is_none() {
				bail!(
					"--latest requires --tag or --path: a kopia repo has many kinds of snapshots, and the newest unfiltered one would pick an arbitrary type. Narrow with --tag (e.g. --tag area:postgres) or --path."
				);
			}

			let snapshots = fetch_snapshots(bin)?;
			let filter = build_filter(
				self.all,
				self.source_host.clone(),
				default_host,
				&self.tags,
				self.path.clone(),
				self.since.as_deref(),
				None,
			)?;
			let matches = filter.apply(&snapshots, Timestamp::now());

			if matches.is_empty() {
				bail!("no snapshots match the given filters");
			}

			if self.latest {
				return Ok(matches.into_iter().next().expect("non-empty"));
			}

			let interactive = std::io::stdout().is_terminal() && std::io::stdin().is_terminal();
			if !interactive {
				bail!(
					"no snapshot specified and stdin/stdout isn't a TTY — pass --snapshot, or --latest (with --tag/--path) to pick the newest match"
				);
			}

			select_snapshot(&matches, picker_prompt)
		}
	}

	fn resolve_by_id(bin: &std::path::Path, id_query: &str) -> Result<Snapshot> {
		let snapshots = fetch_snapshots(bin)?;
		let matches: Vec<&Snapshot> = snapshots
			.iter()
			.filter(|s| s.id.starts_with(id_query))
			.collect();
		match matches.len() {
			0 => bail!("no snapshot found with id starting `{id_query}`"),
			1 => Ok(matches[0].clone()),
			n => bail!("snapshot id `{id_query}` is ambiguous ({n} matches); use a longer prefix"),
		}
	}

	/// Open an interactive picker over a list of snapshots, returning the
	/// chosen one. Defaults to the first (newest) entry.
	pub fn select_snapshot(snapshots: &[Snapshot], prompt: &str) -> Result<Snapshot> {
		let items: Vec<String> = snapshots.iter().map(format_snapshot_line).collect();
		let selection = Select::new()
			.with_prompt(prompt)
			.items(&items)
			.default(0)
			.interact()
			.into_diagnostic()
			.wrap_err("interactive picker failed")?;
		Ok(snapshots[selection].clone())
	}
}

#[cfg(test)]
mod tests {
	use jiff::ToSpan;

	use super::*;

	fn args_of(cmd: &Command) -> Vec<String> {
		cmd.get_args()
			.map(|a| a.to_string_lossy().into_owned())
			.collect()
	}

	#[test]
	fn repository_connect_s3_args_are_in_order() {
		let mut cmd = Command::new("kopia");
		args_repository_connect_s3(
			&mut cmd,
			"my-bucket",
			"",
			"ap-southeast-2",
			"canopy",
			"server-id-123",
		);
		assert_eq!(
			args_of(&cmd),
			vec![
				"repository",
				"connect",
				"s3",
				"--bucket",
				"my-bucket",
				"--prefix",
				"",
				"--region",
				"ap-southeast-2",
				"--override-username",
				"canopy",
				"--override-hostname",
				"server-id-123",
			]
		);
	}

	#[test]
	fn snapshot_create_args_emit_sorted_colon_tags() {
		let mut cmd = Command::new("kopia");
		let mut tags = BTreeMap::new();
		tags.insert("canopy-run".to_owned(), "run-uuid".to_owned());
		tags.insert("canopy-device".to_owned(), "device-uuid".to_owned());
		args_snapshot_create(&mut cmd, Path::new("/data/pg"), &tags);
		assert_eq!(
			args_of(&cmd),
			vec![
				"snapshot",
				"create",
				"--json",
				// BTreeMap iterates sorted: canopy-device before canopy-run.
				"--tags",
				"canopy-device:device-uuid",
				"--tags",
				"canopy-run:run-uuid",
				"/data/pg",
			]
		);
	}

	#[test]
	fn snapshot_restore_args() {
		let mut cmd = Command::new("kopia");
		args_snapshot_restore(&mut cmd, "abc123", Path::new("/restore/here"));
		assert_eq!(
			args_of(&cmd),
			vec!["snapshot", "restore", "abc123", "/restore/here"]
		);
	}

	#[test]
	fn policy_set_ignores_args() {
		let mut cmd = Command::new("kopia");
		args_policy_set_ignores(
			&mut cmd,
			Path::new("/data/pg"),
			&["postmaster.pid".to_owned(), "*.log".to_owned()],
		);
		assert_eq!(
			args_of(&cmd),
			vec![
				"policy",
				"set",
				"--add-ignore=postmaster.pid",
				"--add-ignore=*.log",
				"/data/pg",
			]
		);
	}

	#[test]
	fn s3_env_sets_creds_vars_and_scrubs_shadowing_ones() {
		let env = S3KopiaEnv {
			full_uri: "http://127.0.0.1:5000/creds",
			token: "bearer-token",
			password: "repo-pass",
			config_path: Path::new("/run/bestool/kopia.config"),
		};
		let cmd = build_kopia_command_with_s3(Path::new("/usr/bin/kopia"), &env).unwrap();
		let envs: std::collections::HashMap<String, Option<String>> = cmd
			.get_envs()
			.map(|(k, v)| {
				(
					k.to_string_lossy().into_owned(),
					v.map(|v| v.to_string_lossy().into_owned()),
				)
			})
			.collect();

		assert_eq!(
			envs.get("AWS_CONTAINER_CREDENTIALS_FULL_URI"),
			Some(&Some("http://127.0.0.1:5000/creds".to_owned()))
		);
		assert_eq!(
			envs.get("AWS_CONTAINER_AUTHORIZATION_TOKEN"),
			Some(&Some("bearer-token".to_owned()))
		);
		assert_eq!(
			envs.get("KOPIA_PASSWORD"),
			Some(&Some("repo-pass".to_owned()))
		);
		assert_eq!(
			envs.get("KOPIA_CONFIG_PATH"),
			Some(&Some("/run/bestool/kopia.config".to_owned()))
		);
		// Shadowing vars are explicitly removed (None == env_remove).
		for key in S3_SHADOWING_ENV_VARS {
			assert_eq!(envs.get(key), Some(&None), "{key} must be scrubbed");
		}
	}

	#[test]
	fn preserve_env_keys_lists_all_creds_vars() {
		let env = S3KopiaEnv {
			full_uri: "http://127.0.0.1:0/creds",
			token: "t",
			password: "p",
			config_path: Path::new("/tmp/c"),
		};
		assert_eq!(
			env.preserve_env_keys(),
			"AWS_CONTAINER_CREDENTIALS_FULL_URI,AWS_CONTAINER_AUTHORIZATION_TOKEN,KOPIA_PASSWORD,KOPIA_CONFIG_PATH"
		);
	}

	fn snapshot(id: &str, host: &str, path: &str, taken: Timestamp) -> Snapshot {
		Snapshot {
			id: id.into(),
			source: SnapshotSource {
				host: host.into(),
				user_name: "kopia".into(),
				path: path.into(),
			},
			description: String::new(),
			start_time: Some(taken),
			end_time: Some(taken),
			tags: BTreeMap::new(),
			root_entry: None,
		}
	}

	#[test]
	fn filter_by_host() {
		let now = Timestamp::from_second(10_000_000).unwrap();
		let snaps = vec![
			snapshot("a", "host-1", "/data", now),
			snapshot("b", "host-2", "/data", now),
		];
		let filter = SnapshotFilter {
			source_host: Some("host-1".into()),
			..Default::default()
		};
		let got = filter.apply(&snaps, now);
		assert_eq!(got.len(), 1);
		assert_eq!(got[0].id, "a");
	}

	#[test]
	fn filter_by_tags_requires_all_to_match() {
		let now = Timestamp::from_second(10_000_000).unwrap();
		let mut tagged = snapshot("t", "h", "/data", now);
		tagged.tags.insert("area".into(), "postgres".into());
		tagged.tags.insert("type".into(), "ext4".into());
		let snaps = vec![tagged, snapshot("u", "h", "/data", now)];

		let mut tags = BTreeMap::new();
		tags.insert("area".into(), "postgres".into());
		tags.insert("type".into(), "ext4".into());
		let filter = SnapshotFilter {
			tags,
			..Default::default()
		};
		let got = filter.apply(&snaps, now);
		assert_eq!(got.len(), 1);
		assert_eq!(got[0].id, "t");
	}

	#[test]
	fn filter_path_substr_case_insensitive() {
		let now = Timestamp::from_second(10_000_000).unwrap();
		let snaps = vec![
			snapshot("a", "h", r"C:\Program Files\PostgreSQL\15", now),
			snapshot("b", "h", "/var/log/something", now),
		];
		let filter = SnapshotFilter {
			path_substr: Some("postgresql".into()),
			..Default::default()
		};
		let got = filter.apply(&snaps, now);
		assert_eq!(got.len(), 1);
		assert_eq!(got[0].id, "a");
	}

	#[test]
	fn filter_since_drops_old_snapshots() {
		let now = Timestamp::from_second(10_000_000).unwrap();
		let snaps = vec![
			snapshot("recent", "h", "/data", now - 1.hour()),
			snapshot("old", "h", "/data", now - 30.hours()),
		];
		let filter = SnapshotFilter {
			since: Some(24.hours()),
			..Default::default()
		};
		let got = filter.apply(&snaps, now);
		assert_eq!(got.len(), 1);
		assert_eq!(got[0].id, "recent");
	}

	#[test]
	fn filter_sorts_newest_first() {
		let now = Timestamp::from_second(10_000_000).unwrap();
		let snaps = vec![
			snapshot("older", "h", "/data", now - 2.hours()),
			snapshot("newer", "h", "/data", now - 1.hour()),
		];
		let filter = SnapshotFilter::default();
		let got = filter.apply(&snaps, now);
		assert_eq!(got[0].id, "newer");
		assert_eq!(got[1].id, "older");
	}

	#[test]
	fn filter_limit_truncates_after_sort() {
		let now = Timestamp::from_second(10_000_000).unwrap();
		let snaps = vec![
			snapshot("a", "h", "/data", now - 3.hours()),
			snapshot("b", "h", "/data", now - 1.hour()),
			snapshot("c", "h", "/data", now - 2.hours()),
		];
		let filter = SnapshotFilter {
			limit: Some(2),
			..Default::default()
		};
		let got = filter.apply(&snaps, now);
		assert_eq!(got.len(), 2);
		assert_eq!(got[0].id, "b");
		assert_eq!(got[1].id, "c");
	}

	#[test]
	fn parse_tag_kv_accepts_simple() {
		assert_eq!(
			parse_tag_kv("area:postgres").unwrap(),
			("area".into(), "postgres".into())
		);
	}

	#[test]
	fn parse_tag_kv_rejects_no_colon() {
		assert!(parse_tag_kv("area-postgres").is_err());
	}

	#[test]
	fn parse_tag_kv_rejects_empty_sides() {
		assert!(parse_tag_kv(":value").is_err());
		assert!(parse_tag_kv("key:").is_err());
	}

	#[test]
	fn build_filter_all_drops_host() {
		let filter =
			build_filter(true, None, Some("ignored".into()), &[], None, None, None).unwrap();
		assert!(filter.source_host.is_none());
	}

	#[test]
	fn build_filter_default_host_used_when_not_overridden() {
		let filter = build_filter(
			false,
			None,
			Some("default-host".into()),
			&[],
			None,
			None,
			None,
		)
		.unwrap();
		assert_eq!(filter.source_host.as_deref(), Some("default-host"));
	}

	#[test]
	fn build_filter_explicit_host_beats_default() {
		let filter = build_filter(
			false,
			Some("explicit".into()),
			Some("default".into()),
			&[],
			None,
			None,
			None,
		)
		.unwrap();
		assert_eq!(filter.source_host.as_deref(), Some("explicit"));
	}

	#[test]
	fn build_filter_parses_since() {
		let filter = build_filter(false, None, None, &[], None, Some("24h"), None).unwrap();
		assert!(filter.since.is_some());
	}

	#[test]
	fn build_filter_rejects_bad_since() {
		let err =
			build_filter(false, None, None, &[], None, Some("not-a-duration"), None).unwrap_err();
		assert!(format!("{err}").contains("--since"));
	}

	#[test]
	fn human_bytes_formats_units() {
		assert_eq!(human_bytes(500), "500B");
		assert_eq!(human_bytes(2 * 1024), "2.0KB");
		assert_eq!(human_bytes(3 * 1024 * 1024 + 512 * 1024), "3.5MB");
		assert_eq!(human_bytes(-1), "?");
	}

	#[test]
	fn snapshot_taken_at_falls_back_to_start() {
		let now = Timestamp::from_second(10_000_000).unwrap();
		let mut snap = snapshot("a", "h", "/data", now);
		snap.end_time = None;
		assert_eq!(snap.taken_at(), Some(now));
	}

	#[test]
	fn short_id_truncates_long_ids() {
		assert_eq!(
			short_id("kabcdef0123456789aaaaaaaaaaaaaaaa"),
			"kabcdef012345678"
		);
	}

	#[test]
	fn short_id_passes_short_through() {
		assert_eq!(short_id("k0000"), "k0000");
	}

	#[test]
	fn format_tags_renders_sorted_kv_pairs() {
		let mut tags = BTreeMap::new();
		tags.insert("z".into(), "last".into());
		tags.insert("a".into(), "first".into());
		assert_eq!(format_tags(&tags), "a=first, z=last");
	}

	#[test]
	fn format_tags_empty() {
		let tags = BTreeMap::new();
		assert_eq!(format_tags(&tags), "");
	}

	#[test]
	fn format_snapshot_line_includes_id_source_and_tags() {
		let now = Timestamp::from_second(10_000_000).unwrap();
		let mut s = snapshot("kabc", "host-1", "/data", now);
		s.tags.insert("area".into(), "postgres".into());
		let line = format_snapshot_line(&s);
		assert!(line.contains("kabc"));
		assert!(line.contains("host-1"));
		assert!(line.contains("/data"));
		assert!(line.contains("area=postgres"));
	}

	#[test]
	fn format_snapshot_line_omits_brackets_when_no_tags() {
		let now = Timestamp::from_second(10_000_000).unwrap();
		let s = snapshot("kabc", "host-1", "/data", now);
		let line = format_snapshot_line(&s);
		assert!(!line.contains("[]"));
	}
}
