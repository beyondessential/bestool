//! Shared helpers for interacting with the kopia CLI.
//!
//! Used by `bestool` (for the `bestool kopia` subcommand suite and the canopy
//! backup/restore flows). Has nothing tamanu-specific in it.
//!
//! Highlights:
//! - [`find_kopia_binary`] / [`find_windows_kopia_binary`]: locate kopia from
//!   KopiaUI's standard install locations.
//! - [`linux_elevation`]: decide how to run kopia as the `kopia` system user on
//!   Linux — [`Elevation::SetPriv`] (drop from root) or [`Elevation::Sudo`]
//!   (escalate as a mortal) when the system install is present,
//!   [`Elevation::Direct`] when we already have access, [`Elevation::Skip`]
//!   otherwise.
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

#[cfg(feature = "proxy")]
pub mod proxy;

/// System user that owns the Linux kopia install.
pub const LINUX_KOPIA_USER: &str = "kopia";

/// Home directory of the [`LINUX_KOPIA_USER`]. Kopia derives its config
/// (`$HOME/.config/kopia`) and cache/logs (`$HOME/.cache/kopia`) from it, so we
/// set `HOME` to this when running kopia as that user.
pub const LINUX_KOPIA_HOME: &str = "/var/lib/kopia";

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

/// Current process's username (via `whoami`). `None` if `whoami` can't
/// determine it (rare).
pub fn current_username() -> Option<String> {
	whoami::username().ok()
}

/// How to run kopia as the [`LINUX_KOPIA_USER`] on Linux.
#[derive(Debug, PartialEq, Eq)]
pub enum Elevation {
	/// Run as the current user — either we're already the kopia user, or
	/// there's no system kopia install (the operator's running their own).
	Direct,
	/// We're root: drop to the kopia user with `setpriv`. A privilege *drop*,
	/// so it works under the daemon's `NoNewPrivileges` (where `sudo`, which
	/// must be able to gain privileges, refuses to run).
	SetPriv,
	/// We're a non-root user: elevate to the kopia user with `sudo -u kopia`.
	/// If `sudo` isn't allowed (no NOPASSWD rule, no TTY), the kopia invocation
	/// fails and the caller surfaces that as a Skip.
	Sudo,
	/// We can't elevate. The caller should bail with a reason.
	Skip(String),
}

/// Whether the current process's effective uid is 0. No `libc` in the tree, so
/// ask `id -u` (the backup code resolves uids the same way).
#[cfg(target_os = "linux")]
fn is_root() -> bool {
	std::process::Command::new("id")
		.arg("-u")
		.output()
		.ok()
		.filter(|o| o.status.success())
		.and_then(|o| String::from_utf8(o.stdout).ok())
		.map(|s| s.trim() == "0")
		.unwrap_or(false)
}

/// Decide how to invoke kopia on Linux. We always run kopia *as the kopia user*,
/// never as whoever we happen to be, so the repo config/cache stay owned by that
/// user and the snapshot's idmapped files are readable.
///
/// - If we're already the kopia user, run directly.
/// - Else probe the system kopia config:
///   - Not found (ENOENT): no system install. Run directly as the current user
///     (they're running their own kopia under their own config).
///   - Exists (readable as root, or EACCES as a mortal): run as the kopia user —
///     `setpriv` when we're root, `sudo` otherwise.
#[cfg(target_os = "linux")]
pub fn linux_elevation() -> Elevation {
	let Some(user) = current_username() else {
		return Elevation::Skip("could not determine current Unix username".into());
	};

	if user == LINUX_KOPIA_USER {
		return Elevation::Direct;
	}

	let exists = match std::fs::metadata(LINUX_KOPIA_CONFIG) {
		Ok(_) => true,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
		Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => true,
		Err(err) => return Elevation::Skip(format!("checking {LINUX_KOPIA_CONFIG}: {err}")),
	};
	if !exists {
		return Elevation::Direct;
	}
	if is_root() {
		Elevation::SetPriv
	} else {
		Elevation::Sudo
	}
}

#[cfg(not(target_os = "linux"))]
pub fn linux_elevation() -> Elevation {
	Elevation::Direct
}

/// Whether the kopia system user exists (via `id -u kopia`).
#[cfg(target_os = "linux")]
fn kopia_user_exists() -> bool {
	std::process::Command::new("id")
		.arg("-u")
		.arg(LINUX_KOPIA_USER)
		.output()
		.map(|o| o.status.success())
		.unwrap_or(false)
}

/// Elevation for the canopy-managed repo. Unlike [`linux_elevation`], it keys on
/// whether the kopia *user* exists, not on the system repo config: canopy
/// backups reach the repo through a transient config and the loopback proxy, so
/// the system config is typically absent even though we still want to run kopia
/// as the kopia user (for cache ownership and idmapped-snapshot reads).
#[cfg(target_os = "linux")]
fn canopy_elevation() -> Elevation {
	let Some(user) = current_username() else {
		return Elevation::Skip("could not determine current Unix username".into());
	};
	if user == LINUX_KOPIA_USER {
		return Elevation::Direct;
	}
	if !kopia_user_exists() {
		// No kopia user to drop to; run as ourselves (the command still pins
		// kopia's home to /var/lib/kopia so the cache stays writable).
		return Elevation::Direct;
	}
	if is_root() {
		Elevation::SetPriv
	} else {
		Elevation::Sudo
	}
}

/// The user kopia will actually run as for a canopy-managed run, when that
/// differs from the current user — so a caller can hand it ownership of
/// transient files (e.g. the per-run config). `None` when kopia runs as the
/// current user, or off Linux.
pub fn kopia_run_as_user() -> Option<&'static str> {
	#[cfg(target_os = "linux")]
	{
		match canopy_elevation() {
			Elevation::SetPriv | Elevation::Sudo => Some(LINUX_KOPIA_USER),
			Elevation::Direct | Elevation::Skip(_) => None,
		}
	}
	#[cfg(not(target_os = "linux"))]
	{
		None
	}
}

/// `setpriv --reuid kopia --regid kopia --init-groups -- <kopia>`, with the
/// kopia user's environment. setpriv only swaps credentials and leaves the
/// environment untouched, so set `HOME` (kopia reads its config/cache relative
/// to it) and drop the inherited `XDG_CACHE_HOME` (so the cache lands in
/// `$HOME/.cache`, not the daemon's read-only `/var/cache`).
#[cfg(target_os = "linux")]
fn setpriv_as_kopia(kopia: &Path) -> Command {
	let mut c = Command::new("setpriv");
	c.args([
		"--reuid",
		LINUX_KOPIA_USER,
		"--regid",
		LINUX_KOPIA_USER,
		"--init-groups",
		"--",
	]);
	c.arg(kopia);
	c.env("HOME", LINUX_KOPIA_HOME);
	c.env_remove("XDG_CACHE_HOME");
	c
}

/// `sudo -H -u kopia [--preserve-env=…] -- <kopia>`. `-H` sets `HOME` to the
/// kopia user's home; `env_reset` (sudo's default) drops the rest, so any vars
/// kopia needs are forwarded via `--preserve-env`.
#[cfg(target_os = "linux")]
fn sudo_as_kopia(kopia: &Path, preserve_env: Option<&str>) -> Command {
	let mut c = Command::new("sudo");
	c.arg("-H");
	if let Some(keys) = preserve_env {
		c.arg(format!("--preserve-env={keys}"));
	}
	c.arg("-u").arg(LINUX_KOPIA_USER).arg("--").arg(kopia);
	c
}

/// Build the base kopia command for a given elevation (Linux).
#[cfg(target_os = "linux")]
fn command_for(kopia: &Path, elevation: Elevation) -> Result<Command, String> {
	Ok(match elevation {
		Elevation::Direct => Command::new(kopia),
		Elevation::SetPriv => setpriv_as_kopia(kopia),
		Elevation::Sudo => sudo_as_kopia(kopia, None),
		Elevation::Skip(reason) => return Err(reason),
	})
}

/// Build a `Command` that runs the kopia binary, elevated to the kopia user
/// if the current platform/user requires it (Linux only).
///
/// On non-Linux platforms this is just `Command::new(kopia)`. [`Elevation::Skip`]
/// is propagated as an `Err` whose message is the Skip reason.
#[cfg(target_os = "linux")]
pub fn build_kopia_command(kopia: &Path) -> Result<Command, String> {
	command_for(kopia, linux_elevation())
}

#[cfg(not(target_os = "linux"))]
pub fn build_kopia_command(kopia: &Path) -> Result<Command, String> {
	Ok(Command::new(kopia))
}

/// Ambient AWS credential environment variables, scrubbed from kopia's
/// environment so the host's own credentials can't shadow the dummy keys kopia
/// carries for the loopback re-signing proxy.
pub const S3_SHADOWING_ENV_VARS: [&str; 7] = [
	"AWS_ACCESS_KEY_ID",
	"AWS_SECRET_ACCESS_KEY",
	"AWS_SESSION_TOKEN",
	"AWS_WEB_IDENTITY_TOKEN_FILE",
	"AWS_ROLE_ARN",
	"AWS_CONTAINER_CREDENTIALS_RELATIVE_URI",
	"AWS_CONTAINER_AUTHORIZATION_TOKEN_FILE",
];

/// Environment for a kopia run against the canopy-managed repo.
///
/// `password` is a secret — passed via the environment (never argv) so it
/// doesn't show up in the process list. `config_path` points kopia at a
/// transient per-run config so the bucket/password never persist to the
/// device's kopia config.
pub struct S3KopiaEnv<'a> {
	/// `KOPIA_PASSWORD` (the repo passphrase).
	pub password: &'a str,
	/// `KOPIA_CONFIG_PATH` (a transient per-run config file).
	pub config_path: &'a Path,
}

impl S3KopiaEnv<'_> {
	/// The (key, value) pairs this env sets on a kopia process.
	fn vars(&self) -> [(&'static str, std::ffi::OsString); 2] {
		[
			("KOPIA_PASSWORD", self.password.into()),
			("KOPIA_CONFIG_PATH", self.config_path.as_os_str().to_owned()),
		]
	}

	/// The keys to forward across a `sudo` env reset (`--preserve-env=…`).
	#[cfg(target_os = "linux")]
	fn preserve_env_keys(&self) -> String {
		self.vars()
			.iter()
			.map(|(k, _)| *k)
			.collect::<Vec<_>>()
			.join(",")
	}
}

/// Apply the canopy S3 repo environment to a kopia command: scrub the ambient
/// AWS vars (so they can't shadow the dummy proxy keys) and set the password /
/// transient-config vars. `setpriv` leaves the environment intact and sudo's
/// `env_reset` drops the AWS vars already — removing them here covers both and
/// the direct (inherited-environment) path.
fn apply_s3_env(cmd: &mut Command, env: &S3KopiaEnv<'_>) {
	for key in S3_SHADOWING_ENV_VARS {
		cmd.env_remove(key);
	}
	for (key, value) in env.vars() {
		cmd.env(key, value);
	}
}

/// Build a kopia [`Command`] for the canopy-managed S3 repo, which is reached
/// through the loopback re-signing proxy.
///
/// On Linux, kopia runs as the [`LINUX_KOPIA_USER`] (`setpriv` when we're root,
/// `sudo` otherwise). Under sudo the password/config vars are forwarded across
/// `env_reset` via `--preserve-env`; under setpriv the environment is intact.
#[cfg(target_os = "linux")]
fn command_for_s3(
	kopia: &Path,
	env: &S3KopiaEnv<'_>,
	elevation: Elevation,
) -> Result<Command, String> {
	let mut cmd = match elevation {
		Elevation::Direct => {
			// Even unelevated, pin kopia's home to the kopia user's so its cache
			// and logs land in the writable /var/lib/kopia rather than the
			// daemon's read-only /var/cache (where the inherited XDG_CACHE_HOME
			// would otherwise put them).
			let mut c = Command::new(kopia);
			c.env("HOME", LINUX_KOPIA_HOME);
			c.env_remove("XDG_CACHE_HOME");
			c
		}
		Elevation::SetPriv => setpriv_as_kopia(kopia),
		Elevation::Sudo => sudo_as_kopia(kopia, Some(&env.preserve_env_keys())),
		Elevation::Skip(reason) => return Err(reason),
	};
	apply_s3_env(&mut cmd, env);
	Ok(cmd)
}

/// Which user a canopy-managed kopia command should run as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAs {
	/// The kopia user (backup): keeps the repo cache owned by that user and lets
	/// it read the snapshot's idmapped, postgres-owned files. Elevates per
	/// `canopy_elevation` (keyed on the kopia user existing).
	KopiaUser,
	/// The current user (restore): the command writes destinations the kopia
	/// user can't reach (e.g. a staging dir beside a postgres data directory),
	/// and the caller already runs with enough privilege (root).
	CurrentUser,
}

/// Build a kopia [`Command`] for the canopy-managed S3 repo, reached through the
/// loopback re-signing proxy, running as `run_as` (see [`RunAs`]).
#[cfg(target_os = "linux")]
pub fn build_kopia_command_with_s3(
	kopia: &Path,
	env: &S3KopiaEnv<'_>,
	run_as: RunAs,
) -> Result<Command, String> {
	let elevation = match run_as {
		RunAs::KopiaUser => canopy_elevation(),
		RunAs::CurrentUser => Elevation::Direct,
	};
	command_for_s3(kopia, env, elevation)
}

#[cfg(not(target_os = "linux"))]
pub fn build_kopia_command_with_s3(
	kopia: &Path,
	env: &S3KopiaEnv<'_>,
	_run_as: RunAs,
) -> Result<Command, String> {
	let mut cmd = Command::new(kopia);
	apply_s3_env(&mut cmd, env);
	Ok(cmd)
}

/// Dummy S3 credentials kopia carries. Meaningless on their own — the loopback
/// re-signing proxy discards them and re-signs every request with live
/// credentials — but kopia's minio-go backend requires non-empty keys at parse
/// time.
pub const PROXY_DUMMY_ACCESS_KEY: &str = "bestool-proxy-dummy-access-key";
pub const PROXY_DUMMY_SECRET_KEY: &str = "bestool-proxy-dummy-secret-key";

/// Push `repository connect s3` args for the canopy-managed repo, reached
/// through the loopback re-signing proxy at `endpoint` (TLS disabled on that
/// leg) with dummy credentials.
pub fn args_repository_connect_s3(
	cmd: &mut Command,
	bucket: &str,
	prefix: &str,
	region: &str,
	endpoint: &str,
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
		.arg("--endpoint")
		.arg(endpoint)
		.arg("--disable-tls")
		.arg("--access-key")
		.arg(PROXY_DUMMY_ACCESS_KEY)
		.arg("--secret-access-key")
		.arg(PROXY_DUMMY_SECRET_KEY)
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

/// In-process filter criteria for a snapshot list.
///
/// Tag filtering is not here: kopia's `snapshot list` does not echo the
/// create-time tags, so tags are applied at the source (see
/// [`fetch_snapshots`]), not against the parsed list.
#[derive(Debug, Default, Clone)]
pub struct SnapshotFilter {
	/// `None` means "any host". `Some(name)` filters source.host == name.
	pub source_host: Option<String>,
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

/// Parse repeated `--tag KEY:VALUE` flags into a map for [`fetch_snapshots`].
pub fn parse_tags(tags: &[String]) -> Result<BTreeMap<String, String>> {
	let mut map = BTreeMap::new();
	for raw in tags {
		let (k, v) = parse_tag_kv(raw).map_err(|e| miette!("invalid --tag: {e}"))?;
		map.insert(k, v);
	}
	Ok(map)
}

/// Build a [`SnapshotFilter`] from CLI-shaped inputs. `all = true` drops any
/// source-host filter. Tags are applied at fetch time, not here.
pub fn build_filter(
	all: bool,
	source_host: Option<String>,
	default_host: Option<String>,
	path: Option<String>,
	since: Option<&str>,
	limit: Option<usize>,
) -> Result<SnapshotFilter> {
	let source_host = if all {
		None
	} else {
		source_host.or(default_host)
	};

	let since = since
		.map(|s| {
			s.parse::<Span>()
				.map_err(|e| miette!("invalid --since duration `{s}`: {e}"))
		})
		.transpose()?;

	Ok(SnapshotFilter {
		source_host,
		path_substr: path,
		since,
		limit,
	})
}

/// Build the `kopia snapshot list --json --all` command, filtering by `tags`.
///
/// Tags are applied by kopia (`--tags KEY:VALUE`) against the snapshot's
/// manifest labels: `snapshot list` does not echo the create-time tags in its
/// output, so filtering on them in-process would match nothing.
fn snapshot_list_command(bin: &Path, tags: &BTreeMap<String, String>) -> Command {
	let mut cmd = Command::new(bin);
	cmd.args(["snapshot", "list", "--json", "--all"]);
	for (k, v) in tags {
		cmd.arg("--tags").arg(format!("{k}:{v}"));
	}
	cmd.env("KOPIA_CHECK_FOR_UPDATES", "false");
	cmd
}

/// Run `kopia snapshot list --json --all` (optionally tag-filtered) and parse
/// the result.
///
/// `bin` is expected to already be wrapped by [`build_kopia_command`] if
/// elevation was needed — callers typically run that first and pass the
/// resulting binary path here.
pub fn fetch_snapshots(bin: &Path, tags: &BTreeMap<String, String>) -> Result<Vec<Snapshot>> {
	let output = snapshot_list_command(bin, tags)
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

			let snapshots = fetch_snapshots(bin, &parse_tags(&self.tags)?)?;
			let filter = build_filter(
				self.all,
				self.source_host.clone(),
				default_host,
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
		let snapshots = fetch_snapshots(bin, &BTreeMap::new())?;
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

	#[cfg(target_os = "linux")]
	fn env_of(cmd: &Command) -> std::collections::HashMap<String, Option<String>> {
		cmd.get_envs()
			.map(|(k, v)| {
				(
					k.to_string_lossy().into_owned(),
					v.map(|v| v.to_string_lossy().into_owned()),
				)
			})
			.collect()
	}

	#[cfg(target_os = "linux")]
	#[test]
	fn setpriv_elevation_runs_kopia_as_the_kopia_user() {
		let cmd = command_for(Path::new("/usr/bin/kopia"), Elevation::SetPriv).unwrap();
		assert_eq!(cmd.get_program(), "setpriv");
		let args = args_of(&cmd);
		assert_eq!(
			args,
			vec![
				"--reuid",
				LINUX_KOPIA_USER,
				"--regid",
				LINUX_KOPIA_USER,
				"--init-groups",
				"--",
				"/usr/bin/kopia",
			]
		);
		let env = env_of(&cmd);
		assert_eq!(env.get("HOME"), Some(&Some(LINUX_KOPIA_HOME.to_owned())));
		// XDG_CACHE_HOME is explicitly cleared so the cache lands under $HOME.
		assert_eq!(env.get("XDG_CACHE_HOME"), Some(&None));
	}

	#[cfg(target_os = "linux")]
	#[test]
	fn sudo_elevation_sets_home_and_targets_the_kopia_user() {
		let cmd = command_for(Path::new("/usr/bin/kopia"), Elevation::Sudo).unwrap();
		assert_eq!(cmd.get_program(), "sudo");
		assert_eq!(
			args_of(&cmd),
			vec!["-H", "-u", LINUX_KOPIA_USER, "--", "/usr/bin/kopia"]
		);
	}

	#[cfg(target_os = "linux")]
	#[test]
	fn direct_elevation_runs_kopia_unwrapped() {
		let cmd = command_for(Path::new("/usr/bin/kopia"), Elevation::Direct).unwrap();
		assert_eq!(cmd.get_program(), "/usr/bin/kopia");
		assert!(args_of(&cmd).is_empty());
	}

	#[cfg(target_os = "linux")]
	#[test]
	fn s3_direct_still_pins_the_cache_home() {
		// Unelevated (no kopia user / we are it), the canopy command must still
		// move kopia's cache off the daemon's read-only /var/cache to its home.
		let env = S3KopiaEnv {
			password: "hunter2",
			config_path: Path::new("/tmp/x/repository.config"),
		};
		let cmd = command_for_s3(Path::new("/usr/bin/kopia"), &env, Elevation::Direct).unwrap();
		assert_eq!(cmd.get_program(), "/usr/bin/kopia");
		let envs = env_of(&cmd);
		assert_eq!(envs.get("HOME"), Some(&Some(LINUX_KOPIA_HOME.to_owned())));
		assert_eq!(envs.get("XDG_CACHE_HOME"), Some(&None));
	}

	#[cfg(target_os = "linux")]
	#[test]
	fn s3_setpriv_carries_secrets_and_scrubs_aws() {
		let env = S3KopiaEnv {
			password: "hunter2",
			config_path: Path::new("/tmp/x/repository.config"),
		};
		let cmd = command_for_s3(Path::new("/usr/bin/kopia"), &env, Elevation::SetPriv).unwrap();
		assert_eq!(cmd.get_program(), "setpriv");
		let envs = env_of(&cmd);
		assert_eq!(
			envs.get("KOPIA_PASSWORD"),
			Some(&Some("hunter2".to_owned()))
		);
		assert_eq!(
			envs.get("KOPIA_CONFIG_PATH"),
			Some(&Some("/tmp/x/repository.config".to_owned()))
		);
		// The ambient AWS vars are scrubbed so they can't shadow the proxy keys.
		for key in S3_SHADOWING_ENV_VARS {
			assert_eq!(envs.get(key), Some(&None), "{key} should be removed");
		}
	}

	#[cfg(target_os = "linux")]
	#[test]
	fn s3_sudo_preserves_the_secret_env_across_env_reset() {
		let env = S3KopiaEnv {
			password: "hunter2",
			config_path: Path::new("/tmp/x/repository.config"),
		};
		let cmd = command_for_s3(Path::new("/usr/bin/kopia"), &env, Elevation::Sudo).unwrap();
		assert_eq!(cmd.get_program(), "sudo");
		assert!(
			args_of(&cmd)
				.iter()
				.any(|a| a == "--preserve-env=KOPIA_PASSWORD,KOPIA_CONFIG_PATH")
		);
	}

	#[test]
	fn repository_connect_s3_args_are_in_order() {
		let mut cmd = Command::new("kopia");
		args_repository_connect_s3(
			&mut cmd,
			"my-bucket",
			"",
			"ap-southeast-2",
			"127.0.0.1:8333",
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
				"--endpoint",
				"127.0.0.1:8333",
				"--disable-tls",
				"--access-key",
				PROXY_DUMMY_ACCESS_KEY,
				"--secret-access-key",
				PROXY_DUMMY_SECRET_KEY,
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
	fn s3_env_sets_repo_vars_and_scrubs_shadowing_ones() {
		let env = S3KopiaEnv {
			password: "repo-pass",
			config_path: Path::new("/run/bestool/kopia.config"),
		};
		let cmd =
			build_kopia_command_with_s3(Path::new("/usr/bin/kopia"), &env, RunAs::CurrentUser)
				.unwrap();
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

	#[cfg(target_os = "linux")]
	#[test]
	fn preserve_env_keys_lists_repo_vars() {
		let env = S3KopiaEnv {
			password: "p",
			config_path: Path::new("/tmp/c"),
		};
		assert_eq!(env.preserve_env_keys(), "KOPIA_PASSWORD,KOPIA_CONFIG_PATH");
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
	fn snapshot_list_command_passes_tags_to_kopia() {
		// Tags must be filtered by kopia (`--tags KEY:VALUE`), not in-process:
		// `snapshot list` doesn't echo them, so a parsed snapshot has empty tags.
		let mut tags = BTreeMap::new();
		tags.insert("area".into(), "postgres".into());
		tags.insert("type".into(), "ext4".into());
		let cmd = snapshot_list_command(Path::new("kopia"), &tags);
		let args: Vec<String> = cmd
			.get_args()
			.map(|a| a.to_string_lossy().into_owned())
			.collect();
		assert!(args.contains(&"--all".to_string()));
		assert!(
			args.windows(2)
				.any(|w| w[0] == "--tags" && w[1] == "area:postgres")
		);
		assert!(
			args.windows(2)
				.any(|w| w[0] == "--tags" && w[1] == "type:ext4")
		);
	}

	#[test]
	fn snapshot_list_command_without_tags_omits_tag_flag() {
		let cmd = snapshot_list_command(Path::new("kopia"), &BTreeMap::new());
		assert!(!cmd.get_args().any(|a| a.to_string_lossy() == "--tags"));
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
		let filter = build_filter(true, None, Some("ignored".into()), None, None, None).unwrap();
		assert!(filter.source_host.is_none());
	}

	#[test]
	fn build_filter_default_host_used_when_not_overridden() {
		let filter =
			build_filter(false, None, Some("default-host".into()), None, None, None).unwrap();
		assert_eq!(filter.source_host.as_deref(), Some("default-host"));
	}

	#[test]
	fn build_filter_explicit_host_beats_default() {
		let filter = build_filter(
			false,
			Some("explicit".into()),
			Some("default".into()),
			None,
			None,
			None,
		)
		.unwrap();
		assert_eq!(filter.source_host.as_deref(), Some("explicit"));
	}

	#[test]
	fn build_filter_parses_since() {
		let filter = build_filter(false, None, None, None, Some("24h"), None).unwrap();
		assert!(filter.since.is_some());
	}

	#[test]
	fn build_filter_rejects_bad_since() {
		let err = build_filter(false, None, None, None, Some("not-a-duration"), None).unwrap_err();
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
