//! Apply PostgreSQL performance tuning on a Windows Tamanu host.
//!
//! On Linux this tuning is applied by configuration management; this command is
//! the Windows equivalent. It finds the PostgreSQL install, computes the tuning
//! from the host's resources (via the shared pgtune module), writes it as a
//! managed block at the end of `postgresql.conf`. Every computed value is
//! checked against the server's own accepted range before anything is written,
//! so a value PostgreSQL would reject never reaches the file — otherwise a
//! later restart, ours or an unrelated reboot, would fail to start the server.
//! It also disables bottom-up ASLR for the PostgreSQL executables, the
//! mitigation for the Windows "could not reattach to shared memory" startup
//! crash. It then reloads PostgreSQL so the reloadable settings take effect
//! immediately, and only when some settings still need a restart does it offer
//! to restart PostgreSQL and then the Tamanu workloads.
//!
//! Windows-only: the whole module is compile-gated at its declaration.

use std::{
	path::{Path, PathBuf},
	time::Duration,
};

use clap::Parser;
use miette::{IntoDiagnostic as _, Result, WrapErr as _, bail};
use sysinfo::{Disks, MemoryRefreshKind, RefreshKind, System};
use tracing::{info, warn};

use bestool_postgres::pgtune::{self, HostResources, Platform, Setting, TuneInputs, conf_block};

use crate::actions::{
	Context,
	tamanu::{TamanuArgs, find_tamanu, restart},
};

/// Tune PostgreSQL for this host, reloading it and restarting it if needed
/// (Windows only).
///
/// Computes pgtune-equivalent settings for an OLTP workload on SSD storage,
/// sized to a memory budget that leaves headroom for the co-located Tamanu
/// workloads, and writes them as a managed block at the end of postgresql.conf.
/// A reload applies the settings that don't need a restart immediately; if any
/// settings still require a restart, it offers to restart PostgreSQL and the
/// Tamanu workloads.
///
/// Alias: pgtune
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct PgTuneArgs {
	/// PostgreSQL data directory to tune.
	///
	/// Defaults to the newest install found under the standard Windows location.
	#[arg(long)]
	pub data_dir: Option<PathBuf>,

	/// PostgreSQL major version to tune.
	///
	/// Selects the install at `<ProgramFiles>\PostgreSQL\<version>\data`.
	#[arg(long)]
	pub version: Option<String>,

	/// The `max_connections` value to size the tuning around.
	#[arg(long, default_value_t = 100)]
	pub max_connections: u32,

	/// Percentage of the data volume's size to allow as `temp_file_limit`.
	#[arg(long, default_value_t = 25)]
	pub temp_file_percent: u8,

	/// Compute and show the tuning without writing it, reloading, or restarting
	/// anything.
	#[arg(long)]
	pub dry_run: bool,

	/// Assume yes to the restart prompts.
	#[arg(long)]
	pub yes: bool,

	/// Write and reload the tuning but don't restart PostgreSQL or the Tamanu
	/// workloads.
	#[arg(long)]
	pub no_restart: bool,
}

pub async fn run(args: PgTuneArgs, ctx: Context) -> Result<()> {
	let tamanu = ctx.require::<TamanuArgs>().clone();
	let (_, root) = find_tamanu(&tamanu).await?;
	let config = bestool_tamanu::config::load_config(&root, None)?;

	let data_dir = match (&args.data_dir, &args.version) {
		(Some(dir), _) => dir.clone(),
		(None, Some(version)) => postgres_base().join(version).join("data"),
		(None, None) => pick_data_dir(&postgres_base())?,
	};
	let conf_path = data_dir.join("postgresql.conf");
	if !conf_path.is_file() {
		bail!("no postgresql.conf at {}", conf_path.display());
	}
	let pg_major = read_major(&data_dir)?;

	let sys = System::new_with_specifics(
		RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
	);
	let total_ram_kib = sys.total_memory() / 1024;
	if total_ram_kib == 0 {
		bail!("could not read host memory");
	}
	let cpus = std::thread::available_parallelism()
		.map(|n| n.get() as u32)
		.unwrap_or(1);

	let temp_file_limit_kib =
		data_volume_bytes(&data_dir).map(|bytes| temp_file_limit_kib(bytes, args.temp_file_percent));
	if temp_file_limit_kib.is_none() {
		warn!("could not determine the data volume size; leaving temp_file_limit unset");
	}

	let lz4_wal_supported = detect_lz4(&config.database_url()).await;

	let settings = pgtune::compute(&TuneInputs {
		platform: Platform::Windows,
		resources: HostResources { total_ram_kib, cpus },
		pg_major,
		max_connections: args.max_connections,
		lz4_wal_supported,
		temp_file_limit_kib,
	});
	let block = conf_block::render(&settings);

	let existing = std::fs::read_to_string(&conf_path)
		.into_diagnostic()
		.wrap_err_with(|| format!("reading {}", conf_path.display()))?;
	let updated = conf_block::splice(&existing, &block);

	info!("computed tuning for PostgreSQL {pg_major} at {}", data_dir.display());
	print!("{block}");

	if args.dry_run {
		info!("dry run: not writing {}", conf_path.display());
		return Ok(());
	}

	ensure_bottom_up_aslr_disabled().await;

	if updated == existing {
		info!("already tuned; postgresql.conf is unchanged");
		return Ok(());
	}

	let db_url = config.database_url();

	// Validate every computed value against the server's own accepted domain
	// before writing anything: a value PostgreSQL would reject must never reach
	// postgresql.conf, or a later restart — ours or an unrelated reboot — would
	// stop the server from starting. If we can't check (server unreachable),
	// don't write: better left untuned than a config we can't vouch for.
	match validate_settings(&db_url, &settings).await {
		Ok(problems) if problems.is_empty() => {
			info!("validated the tuning against PostgreSQL's accepted ranges");
		}
		Ok(problems) => bail!(
			"PostgreSQL would reject the computed tuning, so nothing was written: {}",
			problems.join("; ")
		),
		Err(err) => {
			return Err(err)
				.wrap_err("could not validate the tuning against PostgreSQL, so nothing was written");
		}
	}

	write_atomically(&conf_path, &updated)?;
	info!("wrote tuning to {}", conf_path.display());

	// Apply everything we can without downtime first: a reload picks up every
	// setting whose context allows it, leaving only the postmaster-context
	// settings still needing a restart.
	let reloaded = match reload_pg(&data_dir).await {
		Ok(()) => {
			info!("reloaded PostgreSQL; the reloadable settings are now in effect");
			true
		}
		Err(err) => {
			warn!(%err, "could not reload PostgreSQL; a restart is needed to apply the tuning");
			false
		}
	};

	if args.no_restart {
		warn!("skipping restarts (--no-restart); restart PostgreSQL to apply any settings that need it");
		return Ok(());
	}

	// After a reload, PostgreSQL knows exactly which settings still need a
	// restart; when none do, there's no reason to restart at all. If the reload
	// didn't happen we can't tell, so fall through to the restart prompt.
	if reloaded {
		// pg_ctl reload only signals the postmaster; give it a moment to re-read
		// the files before asking which settings are still pending a restart.
		tokio::time::sleep(Duration::from_secs(1)).await;
		let pending = pending_restart_settings(&db_url).await;
		if pending.is_empty() {
			info!("no restart needed; every changed setting is now in effect");
			return Ok(());
		}
		warn!(
			"these settings need a full PostgreSQL restart to take effect: {}",
			pending.join(", ")
		);
	}

	let production = is_production().await;
	let default_restart = !production;

	if args.yes || confirm("Restart PostgreSQL now?", default_restart) {
		restart_pg_service(pg_major).await?;
	} else {
		warn!("PostgreSQL not restarted; schedule a restart to apply the settings that need it");
	}

	if args.yes || confirm("Restart Tamanu workloads now?", default_restart) {
		let restart_args = restart::RestartArgs {
			names: Vec::new(),
			ignore_unmatched: true,
			cooldown: Duration::from_secs(30),
			no_probe_http: true,
			check_url: None,
		};
		restart::run(restart_args, ctx).await?;
	}

	Ok(())
}

/// The standard base directory PostgreSQL installs live under on Windows.
fn postgres_base() -> PathBuf {
	std::env::var_os("ProgramFiles")
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from(r"C:\Program Files"))
		.join("PostgreSQL")
}

/// Whether a directory is a PostgreSQL data directory.
fn is_data_dir(path: &Path) -> bool {
	path.join("PG_VERSION").is_file()
}

/// The newest install's data directory under `base` (each `<version>\data` that
/// holds a `PG_VERSION`). Errors when none are found.
fn pick_data_dir(base: &Path) -> Result<PathBuf> {
	let mut found: Vec<(u32, PathBuf)> = std::fs::read_dir(base)
		.map_err(|e| miette::miette!("reading {}: {e}", base.display()))?
		.flatten()
		.map(|entry| entry.path().join("data"))
		.filter(|dir| is_data_dir(dir))
		.filter_map(|dir| {
			let major = dir.parent().and_then(major_from_name)?;
			Some((major, dir))
		})
		.collect();
	found.sort_by_key(|(major, _)| *major);
	match found.pop() {
		Some((_, dir)) => Ok(dir),
		None => bail!(
			"no PostgreSQL data directory found under {}; pass --data-dir or --version",
			base.display()
		),
	}
}

/// The major version from a `<version>` directory's name (e.g. `16` or `9.6`).
fn major_from_name(dir: &Path) -> Option<u32> {
	dir.file_name().and_then(|n| n.to_str()).and_then(parse_major)
}

/// Parse a PostgreSQL version string's major number (`"16"` → 16, `"9.6"` → 9).
fn parse_major(raw: &str) -> Option<u32> {
	raw.trim().split('.').next()?.parse().ok()
}

/// The PostgreSQL major version from the data directory's `PG_VERSION`.
fn read_major(data_dir: &Path) -> Result<u32> {
	let raw = std::fs::read_to_string(data_dir.join("PG_VERSION"))
		.into_diagnostic()
		.wrap_err_with(|| format!("reading PG_VERSION in {}", data_dir.display()))?;
	parse_major(&raw)
		.ok_or_else(|| miette::miette!("could not parse PostgreSQL version from {raw:?}"))
}

/// The Windows service name for a PostgreSQL major version (the EDB installer
/// default).
fn service_name(major: u32) -> String {
	format!("postgresql-x64-{major}")
}

/// The `temp_file_limit`, in kibibytes, for a data volume of `total_bytes` at
/// `percent` of its size.
fn temp_file_limit_kib(total_bytes: u64, percent: u8) -> u64 {
	(total_bytes / 1024) * u64::from(percent) / 100
}

/// The total size, in bytes, of the volume backing `data_dir` (the mounted disk
/// with the longest matching mount point).
fn data_volume_bytes(data_dir: &Path) -> Option<u64> {
	let disks = Disks::new_with_refreshed_list();
	disks
		.iter()
		.filter(|disk| data_dir.starts_with(disk.mount_point()))
		.max_by_key(|disk| disk.mount_point().as_os_str().len())
		.map(|disk| disk.total_space())
		.filter(|bytes| *bytes > 0)
}

/// Whether the running server reports lz4 as a supported WAL compression method.
/// Best-effort: `false` when the server can't be reached or doesn't list it.
async fn detect_lz4(database_url: &str) -> bool {
	let client = match bestool_postgres::pool::connect_one(database_url, "bestool-pg-tune").await {
		Ok(client) => client,
		Err(err) => {
			warn!(%err, "could not connect to PostgreSQL to detect lz4 support; using wal_compression=on");
			return false;
		}
	};
	let row = client
		.query_opt(
			"SELECT enumvals FROM pg_settings WHERE name = 'wal_compression'",
			&[],
		)
		.await;
	match row {
		Ok(Some(row)) => row
			.try_get::<_, Option<Vec<String>>>("enumvals")
			.ok()
			.flatten()
			.is_some_and(|vals| vals.iter().any(|v| v == "lz4")),
		_ => false,
	}
}

/// Signal a running PostgreSQL to re-read its configuration files, so every
/// reloadable setting in the block we just wrote takes effect without a restart.
///
/// Uses `pg_ctl reload`, which signals the postmaster by PID and needs no
/// database superuser — unlike `SELECT pg_reload_conf()`, which the Tamanu
/// application role generally isn't allowed to run. `pg_ctl.exe` lives in the
/// install's `bin` directory, alongside the `data` directory being tuned.
async fn reload_pg(data_dir: &Path) -> Result<()> {
	let pg_ctl = data_dir
		.parent()
		.map(|install| install.join("bin").join("pg_ctl.exe"))
		.filter(|path| path.is_file())
		.ok_or_else(|| miette::miette!("could not find pg_ctl.exe next to {}", data_dir.display()))?;

	let output = tokio::process::Command::new(&pg_ctl)
		.arg("reload")
		.arg("-D")
		.arg(data_dir)
		.output()
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("running {}", pg_ctl.display()))?;

	if !output.status.success() {
		bail!(
			"pg_ctl reload failed: {}",
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}
	Ok(())
}

/// Disable bottom-up ASLR for the PostgreSQL executables — the documented
/// mitigation for the Windows "could not reattach to shared memory" startup
/// crash, where a forked child can't map the shared memory segment at the
/// address the postmaster used because ASLR relocated something into it.
/// Idempotent and best-effort: a failure warns rather than aborting, since the
/// tuning itself has already been applied.
async fn ensure_bottom_up_aslr_disabled() {
	// -ErrorAction Stop turns a cmdlet error into a terminating one, so a
	// failure shows up as a non-zero exit rather than a note on stdout.
	let script = "$ErrorActionPreference = 'Stop'; \
		Set-ProcessMitigation -Name postgres.exe -Disable BottomUp; \
		Set-ProcessMitigation -Name pg_ctl.exe -Disable BottomUp";
	match run_powershell(script).await {
		Ok(output) if output.status.success() => {
			info!("disabled bottom-up ASLR for postgres.exe and pg_ctl.exe");
		}
		Ok(output) => warn!(
			"could not disable bottom-up ASLR for PostgreSQL (see the runbook): {}",
			String::from_utf8_lossy(&output.stderr).trim()
		),
		Err(err) => {
			warn!(%err, "could not run Set-ProcessMitigation to disable bottom-up ASLR (see the runbook)");
		}
	}
}

/// Run a PowerShell one-liner and capture its output.
async fn run_powershell(script: &str) -> Result<std::process::Output> {
	tokio::process::Command::new("powershell")
		.args(["-NoProfile", "-NonInteractive", "-Command", script])
		.output()
		.await
		.into_diagnostic()
		.wrap_err("running powershell")
}

/// The names of settings that changed but need a full restart to take effect,
/// as PostgreSQL itself reports them via `pg_settings.pending_restart` after a
/// reload. Best-effort: an empty list when the server can't be reached, which
/// reads as "nothing pending" — reasonable, since a server that's down applies
/// everything on its next start anyway.
async fn pending_restart_settings(database_url: &str) -> Vec<String> {
	let client = match bestool_postgres::pool::connect_one(database_url, "bestool-pg-tune").await {
		Ok(client) => client,
		Err(err) => {
			warn!(%err, "could not connect to PostgreSQL to check which settings need a restart");
			return Vec::new();
		}
	};
	match client
		.query(
			"SELECT name FROM pg_settings WHERE pending_restart ORDER BY name",
			&[],
		)
		.await
	{
		Ok(rows) => rows
			.iter()
			.filter_map(|row| row.try_get::<_, String>("name").ok())
			.collect(),
		Err(err) => {
			warn!(%err, "could not read pending_restart from pg_settings");
			Vec::new()
		}
	}
}

/// Confirm PostgreSQL will accept every computed setting, by checking each value
/// against the server's own declared type, range, and allowed values in
/// `pg_settings` — the same domain the config-file loader enforces at startup.
/// Returns a human-readable reason for each value that fails; an empty list
/// means all are in bounds.
///
/// Reads only `pg_settings`, so it needs no superuser and works as the Tamanu
/// application role. Errors (rather than returning problems) when the server
/// can't be reached, so the caller can decline to write a config it couldn't
/// vouch for.
async fn validate_settings(database_url: &str, settings: &[Setting]) -> Result<Vec<String>> {
	let client = bestool_postgres::pool::connect_one(database_url, "bestool-pg-tune")
		.await
		.wrap_err("connecting to PostgreSQL to validate the tuning")?;

	let mut problems = Vec::new();
	for setting in settings {
		let row = client
			.query_opt(
				"SELECT vartype, unit, min_val, max_val, enumvals FROM pg_settings WHERE name = $1",
				&[&setting.key],
			)
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("querying pg_settings for {}", setting.key))?;

		let Some(row) = row else {
			problems.push(format!("{}: unrecognised parameter", setting.key));
			continue;
		};

		let vartype: String = row.get("vartype");
		let unit: Option<String> = row.get("unit");
		let min_val: Option<String> = row.get("min_val");
		let max_val: Option<String> = row.get("max_val");
		let enumvals: Option<Vec<String>> = row.get("enumvals");

		if let Err(why) = check_value(
			&vartype,
			unit.as_deref(),
			min_val.as_deref(),
			max_val.as_deref(),
			enumvals.as_deref(),
			&setting.value,
		) {
			problems.push(format!("{} = {}: {why}", setting.key, setting.value));
		}
	}
	Ok(problems)
}

/// Whether `value` is acceptable for a parameter described by this `pg_settings`
/// metadata, mirroring the checks PostgreSQL runs when it loads a value from
/// postgresql.conf. Returns the reason on failure.
fn check_value(
	vartype: &str,
	unit: Option<&str>,
	min_val: Option<&str>,
	max_val: Option<&str>,
	enumvals: Option<&[String]>,
	value: &str,
) -> Result<(), String> {
	match vartype {
		"bool" => parse_bool(value).map(|_| ()).ok_or_else(|| "not a boolean".into()),
		"enum" => {
			let allowed = enumvals.unwrap_or_default();
			if allowed.iter().any(|v| v.eq_ignore_ascii_case(value.trim())) {
				Ok(())
			} else {
				Err(format!("not one of: {}", allowed.join(", ")))
			}
		}
		"integer" => match normalise_number(value, unit) {
			Some(n) => check_range(n, min_val, max_val),
			None => Err("not a valid amount".into()),
		},
		"real" => match value.trim().parse::<f64>() {
			Ok(n) => check_range(n, min_val, max_val),
			Err(_) => Err("not a number".into()),
		},
		// The tuner emits no string parameters; leave anything else to PostgreSQL.
		_ => Ok(()),
	}
}

/// A numeric value expressed in `unit`, so it can be compared with the
/// `pg_settings` min_val/max_val (which are in that unit). Handles unitless
/// numbers and the memory units the tuner emits (`kB`/`MB`/`GB`); returns `None`
/// for anything it can't confidently convert.
fn normalise_number(value: &str, unit: Option<&str>) -> Option<f64> {
	match unit.map(str::trim).filter(|u| !u.is_empty()) {
		None => value.trim().parse().ok(),
		Some(unit) => {
			let per_unit = memory_unit_bytes(unit)?;
			let bytes = parse_memory_bytes(value)?;
			Some(bytes as f64 / per_unit as f64)
		}
	}
}

/// Whether `n` sits within the optional bounds parsed from `pg_settings`.
fn check_range(n: f64, min_val: Option<&str>, max_val: Option<&str>) -> Result<(), String> {
	if let Some(min) = min_val.and_then(|m| m.trim().parse::<f64>().ok())
		&& n < min
	{
		return Err(format!("below the minimum of {min}"));
	}
	if let Some(max) = max_val.and_then(|m| m.trim().parse::<f64>().ok())
		&& n > max
	{
		return Err(format!("above the maximum of {max}"));
	}
	Ok(())
}

/// Parse a PostgreSQL boolean literal.
fn parse_bool(value: &str) -> Option<bool> {
	match value.trim().to_ascii_lowercase().as_str() {
		"on" | "true" | "yes" | "1" => Some(true),
		"off" | "false" | "no" | "0" => Some(false),
		_ => None,
	}
}

/// Parse a memory quantity like `4GB`, `64MB`, or `512kB` into bytes, matching
/// PostgreSQL's binary (1024-based) memory units.
fn parse_memory_bytes(value: &str) -> Option<u64> {
	let value = value.trim();
	let (digits, factor) = if let Some(n) = value.strip_suffix("TB") {
		(n, 1024u64.pow(4))
	} else if let Some(n) = value.strip_suffix("GB") {
		(n, 1024u64.pow(3))
	} else if let Some(n) = value.strip_suffix("MB") {
		(n, 1024u64.pow(2))
	} else if let Some(n) = value.strip_suffix("kB") {
		(n, 1024)
	} else if let Some(n) = value.strip_suffix('B') {
		(n, 1)
	} else {
		return None;
	};
	digits.trim().parse::<u64>().ok().map(|n| n * factor)
}

/// Bytes per unit for a `pg_settings` memory `unit` such as `8kB`, `kB`, or `MB`
/// (an optional leading multiplier and a memory suffix). `None` for non-memory
/// units, which the tuner never emits.
fn memory_unit_bytes(unit: &str) -> Option<u64> {
	let unit = unit.trim();
	let split = unit.find(|c: char| !c.is_ascii_digit()).unwrap_or(unit.len());
	let (multiplier, suffix) = unit.split_at(split);
	let multiplier: u64 = if multiplier.is_empty() {
		1
	} else {
		multiplier.parse().ok()?
	};
	let base = match suffix {
		"B" => 1,
		"kB" => 1024,
		"MB" => 1024u64.pow(2),
		"GB" => 1024u64.pow(3),
		"TB" => 1024u64.pow(4),
		_ => return None,
	};
	Some(multiplier * base)
}

/// Whether this host is production, from its Tailscale device name containing
/// `prod`. When it can't be determined, the host is treated as production so the
/// restart prompts default to no.
async fn is_production() -> bool {
	match self_name().await {
		Some(name) => name.to_ascii_lowercase().contains("prod"),
		None => {
			warn!("could not read the Tailscale device name; treating host as production");
			true
		}
	}
}

async fn self_name() -> Option<String> {
	let output = tokio::process::Command::new("tailscale")
		.args(["status", "--json"])
		.output()
		.await
		.ok()?;
	if !output.status.success() {
		return None;
	}
	parse_self_name(&output.stdout)
}

/// The local device's Tailscale name, from `tailscale status --json`.
fn parse_self_name(bytes: &[u8]) -> Option<String> {
	#[derive(serde::Deserialize)]
	struct Status {
		#[serde(rename = "Self")]
		self_node: Option<SelfNode>,
	}
	#[derive(serde::Deserialize)]
	struct SelfNode {
		#[serde(rename = "HostName")]
		host_name: Option<String>,
		#[serde(rename = "DNSName")]
		dns_name: Option<String>,
	}

	let status: Status = serde_json::from_slice(bytes).ok()?;
	let node = status.self_node?;
	node.host_name.filter(|n| !n.is_empty()).or_else(|| {
		node.dns_name
			.and_then(|d| d.split('.').next().map(str::to_owned))
			.filter(|n| !n.is_empty())
	})
}

/// Ask a yes/no question with a default taken when the answer is empty or input
/// isn't available.
fn confirm(question: &str, default_yes: bool) -> bool {
	use std::io::{BufRead as _, Write as _};

	let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
	eprint!("{question} {hint} ");
	std::io::stderr().flush().ok();

	let mut line = String::new();
	if std::io::stdin().lock().read_line(&mut line).unwrap_or(0) == 0 {
		return default_yes;
	}
	match line.trim().to_ascii_lowercase().as_str() {
		"y" | "yes" => true,
		"n" | "no" => false,
		_ => default_yes,
	}
}

fn write_atomically(path: &Path, contents: &str) -> Result<()> {
	let tmp = path.with_extension("conf.bestool-tmp");
	std::fs::write(&tmp, contents)
		.into_diagnostic()
		.wrap_err_with(|| format!("writing {}", tmp.display()))?;
	std::fs::rename(&tmp, path)
		.into_diagnostic()
		.wrap_err_with(|| format!("replacing {}", path.display()))
}

/// Restart the PostgreSQL service for `major`, waiting for each transition.
async fn restart_pg_service(major: u32) -> Result<()> {
	let name = service_name(major);
	scm::transition(&name, scm::Desired::Stopped).await?;
	scm::transition(&name, scm::Desired::Running).await?;
	Ok(())
}

mod scm {
	use std::time::Duration;

	use miette::{IntoDiagnostic as _, Result, WrapErr as _, bail};
	use tracing::info;
	use windows_service::{
		service::{ServiceAccess, ServiceState},
		service_manager::{ServiceManager, ServiceManagerAccess},
	};

	#[derive(Clone, Copy)]
	pub enum Desired {
		Stopped,
		Running,
	}

	pub async fn transition(name: &str, desired: Desired) -> Result<()> {
		let name = name.to_owned();
		tokio::task::spawn_blocking(move || transition_blocking(&name, desired))
			.await
			.into_diagnostic()
			.wrap_err("joining service-control task")?
	}

	fn transition_blocking(name: &str, desired: Desired) -> Result<()> {
		let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
			.into_diagnostic()
			.wrap_err("connecting to the Service Control Manager")?;
		let service = manager
			.open_service(
				name,
				ServiceAccess::QUERY_STATUS | ServiceAccess::START | ServiceAccess::STOP,
			)
			.into_diagnostic()
			.wrap_err_with(|| format!("opening the postgres service {name:?}"))?;

		let current = service.query_status().into_diagnostic()?.current_state;
		match desired {
			Desired::Stopped => {
				if current == ServiceState::Stopped {
					return Ok(());
				}
				service
					.stop()
					.into_diagnostic()
					.wrap_err_with(|| format!("stopping the postgres service {name}"))?;
				wait_for(&service, ServiceState::Stopped, name)?;
				info!("stopped the postgres service {name}");
			}
			Desired::Running => {
				if current == ServiceState::Running {
					return Ok(());
				}
				service
					.start::<&str>(&[])
					.into_diagnostic()
					.wrap_err_with(|| format!("starting the postgres service {name}"))?;
				wait_for(&service, ServiceState::Running, name)?;
				info!("started the postgres service {name}");
			}
		}
		Ok(())
	}

	fn wait_for(
		service: &windows_service::service::Service,
		want: ServiceState,
		name: &str,
	) -> Result<()> {
		for _ in 0..120 {
			if service.query_status().into_diagnostic()?.current_state == want {
				return Ok(());
			}
			std::thread::sleep(Duration::from_millis(500));
		}
		bail!("the postgres service {name} did not reach {want:?} within 60s");
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_major_versions() {
		assert_eq!(parse_major("16"), Some(16));
		assert_eq!(parse_major("9.6"), Some(9));
		assert_eq!(parse_major("18\n"), Some(18));
		assert_eq!(parse_major("nonsense"), None);
	}

	#[test]
	fn service_name_uses_edb_default() {
		assert_eq!(service_name(16), "postgresql-x64-16");
	}

	#[test]
	fn temp_file_limit_is_a_fraction_of_the_volume() {
		// 100 GiB volume at 25% -> 25 GiB expressed in KiB.
		let total = 100 * 1024 * 1024 * 1024;
		assert_eq!(temp_file_limit_kib(total, 25), 25 * 1024 * 1024);
		assert_eq!(temp_file_limit_kib(total, 0), 0);
	}

	#[test]
	fn picks_the_newest_install() {
		let tmp = tempfile::tempdir().unwrap();
		for version in ["15", "16", "9.6"] {
			let dir = tmp.path().join(version).join("data");
			std::fs::create_dir_all(&dir).unwrap();
			std::fs::write(dir.join("PG_VERSION"), version).unwrap();
		}
		// A version dir without a data/PG_VERSION is ignored.
		std::fs::create_dir_all(tmp.path().join("junk")).unwrap();
		let picked = pick_data_dir(tmp.path()).unwrap();
		assert_eq!(picked, tmp.path().join("16").join("data"));
	}

	#[test]
	fn no_install_errors() {
		let tmp = tempfile::tempdir().unwrap();
		assert!(pick_data_dir(tmp.path()).is_err());
	}

	#[test]
	fn parses_memory_quantities_and_units() {
		assert_eq!(parse_memory_bytes("4GB"), Some(4 * 1024 * 1024 * 1024));
		assert_eq!(parse_memory_bytes("64MB"), Some(64 * 1024 * 1024));
		assert_eq!(parse_memory_bytes("512kB"), Some(512 * 1024));
		assert_eq!(parse_memory_bytes("nonsense"), None);

		assert_eq!(memory_unit_bytes("8kB"), Some(8 * 1024));
		assert_eq!(memory_unit_bytes("kB"), Some(1024));
		assert_eq!(memory_unit_bytes("MB"), Some(1024 * 1024));
		assert_eq!(memory_unit_bytes("ms"), None);
	}

	#[test]
	fn checks_values_against_metadata() {
		// A memory value inside shared_buffers' range (unit is 8kB blocks).
		assert!(check_value("integer", Some("8kB"), Some("16"), Some("1073741823"), None, "4GB").is_ok());
		// Below the minimum.
		assert!(check_value("integer", Some("8kB"), Some("16"), Some("1073741823"), None, "64kB").is_err());
		// A unitless integer above its maximum.
		assert!(check_value("integer", Some(""), Some("1"), Some("262143"), None, "999999").is_err());
		// Enum membership is case-insensitive.
		let huge = ["off".to_string(), "on".to_string(), "try".to_string()];
		assert!(check_value("enum", None, None, None, Some(&huge), "try").is_ok());
		assert!(check_value("enum", None, None, None, Some(&huge), "maybe").is_err());
		// Reals are range-checked too.
		assert!(check_value("real", Some(""), Some("0"), Some("1"), None, "0.9").is_ok());
		assert!(check_value("real", Some(""), Some("0"), Some("1"), None, "1.5").is_err());
	}

	#[test]
	fn parses_self_name_from_hostname() {
		let json = br#"{"Self": {"HostName": "tamanu-prod-1", "DNSName": "tamanu-prod-1.tail.ts.net."}}"#;
		assert_eq!(parse_self_name(json).as_deref(), Some("tamanu-prod-1"));
	}

	#[test]
	fn falls_back_to_dns_leaf() {
		let json = br#"{"Self": {"HostName": "", "DNSName": "dev-box.tail.ts.net."}}"#;
		assert_eq!(parse_self_name(json).as_deref(), Some("dev-box"));
	}

	#[test]
	fn missing_self_is_none() {
		assert_eq!(parse_self_name(br#"{"Peer": {}}"#), None);
	}
}
