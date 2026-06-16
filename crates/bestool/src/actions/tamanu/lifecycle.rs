//! Shared primitives for the `tamanu` lifecycle subcommands (`start`,
//! `stop`, `restart`, `status`).
//!
//! Discovery, matching, and supervisor (systemd/pm2) dispatch all live
//! here so the four subcommand entry points stay thin.

use std::{
	process::Command,
	time::{Duration, Instant},
};

use miette::{IntoDiagnostic, Result, bail};
use tracing::{debug, info, warn};

use bestool_tamanu::{
	ApiServerKind,
	config::load_config,
	pm2,
	server_info::query_patient_portal_enabled,
	services::{self, Expectation, Supervisor, parse_systemd_unit, systemd_patient_portal_instanced},
	systemd,
};

use super::{TamanuArgs, find_tamanu};

/// How long `config_and_expectations` should wait for the DB to come up
/// before falling back to `features.patientPortal = false`.
///
/// `No` matches the historical behaviour: try once, warn-and-default on
/// failure. Right for interactive/inspection commands (`status`, `logs`)
/// where reporting current state is the goal — flagging "DB unreachable"
/// is more useful than blocking on it.
///
/// `Yes` polls until the DB accepts a connection or [`DB_WAIT_TIMEOUT`]
/// elapses, warning on each retry so operators running manually see
/// what's going on. Right for boot-time `tamanu start`: without it, a
/// host where postgres hasn't finished starting yet sees the portal as
/// disabled and silently skips it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WaitForDb {
	No,
	Yes,
}

/// How long [`WaitForDb::Yes`] keeps retrying the DB connection before
/// giving up. Sized for systemd boot: postgres usually accepts
/// connections within a few seconds, but a slow-starting host (rebuilt
/// index, fsck, large WAL replay) can take longer.
const DB_WAIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Interval between DB connection retries when [`WaitForDb::Yes`].
const DB_WAIT_INTERVAL: Duration = Duration::from_secs(2);

/// Resolve the supervisor + expectation set for the current host.
///
/// Picks systemd on Linux, pm2 on Windows; bails on other platforms.
/// Loads the tamanu config from the discovered root and asks
/// `services::expected` for the canonical expectation list.
///
/// On Linux/central, opens a short-lived DB connection to read
/// `features.patientPortal` so the patient-portal expectation reflects what
/// Tamanu itself thinks is enabled — without this lookup the expectation is
/// always `Down`, which silently no-ops `bestool tamanu start|restart
/// tamanu-patientportal` on deployments where the flag is on. `wait_for_db`
/// controls what happens when the DB doesn't answer: [`WaitForDb::No`]
/// warns and defaults to false (right for inspection commands), while
/// [`WaitForDb::Yes`] polls for up to [`DB_WAIT_TIMEOUT`] so a boot-time
/// `start` doesn't silently skip DB-gated services.
pub async fn config_and_expectations(
	tamanu: &TamanuArgs,
	wait_for_db: WaitForDb,
) -> Result<(Supervisor, Vec<Expectation>)> {
	let supervisor = if cfg!(target_os = "linux") {
		Supervisor::Systemd
	} else if cfg!(target_os = "windows") {
		Supervisor::Pm2
	} else {
		bail!("tamanu lifecycle commands are only supported on Linux (systemd) and Windows (pm2)");
	};

	let (_, root) = find_tamanu(tamanu).await?;
	let config = load_config(&root, None)?;
	let kind = if config.is_facility() {
		ApiServerKind::Facility
	} else {
		ApiServerKind::Central
	};

	// The patient-portal expectation is only emitted on Linux/central; skip
	// the DB round-trip in any other shape. On other deployment shapes
	// there's no portal flag to read, so `Some(false)` is correct — not
	// Unknown, which would falsely imply "we couldn't tell".
	let patient_portal_enabled =
		if matches!(supervisor, Supervisor::Systemd) && matches!(kind, ApiServerKind::Central) {
			query_patient_portal_enabled_with_wait(&config.database_url(), wait_for_db).await
		} else {
			Some(false)
		};

	let patient_portal_instanced = matches!(supervisor, Supervisor::Systemd)
		&& matches!(kind, ApiServerKind::Central)
		&& systemd_patient_portal_instanced().await;

	let expectations = services::expected(
		supervisor,
		kind,
		&config,
		patient_portal_enabled,
		patient_portal_instanced,
	);
	Ok((supervisor, expectations))
}

/// Connect to the DB and read `features.patientPortal`. Returns
/// `Some(value)` when the query succeeds (including the missing-row case,
/// where Tamanu's default of `false` applies), `None` when the DB is
/// unreachable — callers map `None` to the Unknown expectation state so
/// lifecycle commands leave the portal alone rather than guessing.
///
/// With [`WaitForDb::No`], one attempt; on failure, warn and return
/// `None`. With [`WaitForDb::Yes`], retry every [`DB_WAIT_INTERVAL`] for
/// up to [`DB_WAIT_TIMEOUT`], warning on each failure so an operator
/// running the command manually while the DB is down sees what's
/// happening.
async fn query_patient_portal_enabled_with_wait(
	database_url: &str,
	wait_for_db: WaitForDb,
) -> Option<bool> {
	query_patient_portal_enabled_with_wait_inner(
		database_url,
		wait_for_db,
		DB_WAIT_TIMEOUT,
		DB_WAIT_INTERVAL,
	)
	.await
}

/// Test-shimmed core of [`query_patient_portal_enabled_with_wait`] — the
/// retry loop itself, with timing parameters injected so tests can run it
/// with sub-second values instead of the 2-minute production default.
async fn query_patient_portal_enabled_with_wait_inner(
	database_url: &str,
	wait_for_db: WaitForDb,
	timeout: Duration,
	interval: Duration,
) -> Option<bool> {
	let deadline = match wait_for_db {
		WaitForDb::No => None,
		WaitForDb::Yes => Some(Instant::now() + timeout),
	};

	loop {
		match bestool_postgres::pool::connect_one(database_url, "bestool-tamanu-lifecycle").await {
			Ok(client) => return query_patient_portal_enabled(&client).await,
			Err(err) => match deadline {
				None => {
					warn!(%err, "could not query features.patientPortal; expectation will be Unknown");
					return None;
				}
				Some(deadline) if Instant::now() >= deadline => {
					warn!(
						%err,
						"timed out after {}s waiting for the database; features.patientPortal expectation will be Unknown",
						timeout.as_secs(),
					);
					return None;
				}
				Some(_) => {
					warn!(
						%err,
						"database not ready; retrying in {}s (will give up after {}s total)",
						interval.as_secs(),
						timeout.as_secs(),
					);
					tokio::time::sleep(interval).await;
				}
			},
		}
	}
}

/// A live service instance discovered from the supervisor.
///
/// One `Instance` per supervisor entry — for a systemd template unit with
/// instances `@1`, `@2` you get two entries with the same `name` and
/// different `instance` suffixes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
	/// Base service name (no `.service`, no `@`).
	pub name: String,
	/// systemd `@instance` suffix, or `None` on pm2 / singleton units.
	pub instance: Option<String>,
	/// pm2 process id, when discovered via pm2.
	pub pm_id: Option<i64>,
	/// Currently active/online.
	pub running: bool,
}

impl Instance {
	/// systemd unit name (`tamanu-frontend@a.service` /
	/// `tamanu-tasks.service`). Not meaningful for pm2 instances.
	pub fn unit(&self) -> String {
		match &self.instance {
			Some(i) => format!("{}@{}.service", self.name, i),
			None => format!("{}.service", self.name),
		}
	}

	/// Short label for log output.
	pub fn display(&self) -> String {
		match (&self.instance, self.pm_id) {
			(Some(i), _) => format!("{}@{}", self.name, i),
			(None, Some(id)) => format!("{}#{id}", self.name),
			(None, None) => self.name.clone(),
		}
	}
}

/// Enumerate the supervisor's view of currently-known tamanu services.
///
/// Includes both running and non-running entries — discovery doesn't
/// itself filter against expectations; that's `match_instances`.
pub async fn discover(supervisor: Supervisor) -> Result<Vec<Instance>> {
	match supervisor {
		Supervisor::Systemd => discover_systemd().await,
		Supervisor::Pm2 => discover_pm2().map(|(v, _)| v),
	}
}

async fn discover_systemd() -> Result<Vec<Instance>> {
	let units = systemd::list_units(&["tamanu-*.service"]).await?;
	let mut out = Vec::new();
	for u in units {
		let Some((base, instance)) = parse_systemd_unit(&u.name) else {
			continue;
		};
		out.push(Instance {
			name: base.to_string(),
			instance: instance.map(str::to_string),
			pm_id: None,
			running: u.running(),
		});
	}
	Ok(out)
}

fn discover_pm2() -> Result<(Vec<Instance>, pm2::Source)> {
	let (procs, source) = pm2::list().map_err(|e| miette::miette!("pm2: {e}"))?;
	let mut out = Vec::new();
	for p in procs {
		if !p.name.starts_with("tamanu-") {
			continue;
		}
		out.push(Instance {
			name: p.name,
			instance: None,
			pm_id: p.pm_id,
			running: p.running,
		});
	}
	Ok((out, source))
}

/// Warn-log every expectation whose state is Unknown — lifecycle commands
/// silently skip Unknown services (we don't know what they should be doing,
/// so we leave them alone), but operators running interactively should see
/// what's been left out and why.
pub fn warn_unknown_expectations(expectations: &[&Expectation]) {
	for exp in expectations {
		if matches!(exp.state, services::ExpectedState::Unknown) {
			warn!(
				name = exp.name,
				reason = %exp.reason,
				"leaving service alone: expected state is Unknown"
			);
		}
	}
}

/// Group discovered instances under the expectation each belongs to.
///
/// Instances whose `name` and `instance` suffix match an expectation's
/// `name` + `Instances` shape land under that expectation. Unmatched
/// instances are dropped (they're not "expected", so lifecycle commands
/// don't touch them).
pub fn group_by_expectation<'a>(
	supervisor: Supervisor,
	expectations: &'a [&'a Expectation],
	instances: &[Instance],
) -> Vec<(&'a Expectation, Vec<Instance>)> {
	expectations
		.iter()
		.map(|exp| {
			let matches: Vec<Instance> = instances
				.iter()
				.filter(|d| {
					d.name == exp.name
						&& exp.instances.admits_instance(supervisor, d.instance.as_deref())
				})
				.cloned()
				.collect();
			(*exp, matches)
		})
		.collect()
}

/// Discovered running instances that belong (by name) to a templated,
/// expected-Up expectation but whose shape that expectation doesn't admit —
/// i.e. a leftover bare singleton on a host that's since migrated to the
/// `@a`/`@b` template. Grouped under the expectation they're stale for so
/// callers can find the instanced replacements.
///
/// `restart` retires these (stop + disable) once the instanced units are up
/// and serving, so a singleton→instanced migration takes no downtime.
/// systemd-only: pm2 has no `@instance` notion, so a `None` instance there is
/// a legitimate cluster member, not a stale singleton.
pub fn stale_shape_groups<'a>(
	supervisor: Supervisor,
	expectations: &'a [&'a Expectation],
	instances: &[Instance],
) -> Vec<(&'a Expectation, Vec<Instance>)> {
	if !matches!(supervisor, Supervisor::Systemd) {
		return Vec::new();
	}
	expectations
		.iter()
		.filter(|exp| {
			matches!(exp.state, services::ExpectedState::Up) && exp.instances.min_count() >= 2
		})
		.filter_map(|exp| {
			let stale: Vec<Instance> = instances
				.iter()
				.filter(|d| {
					d.running
						&& d.name == exp.name
						&& !exp.instances.admits_instance(supervisor, d.instance.as_deref())
				})
				.cloned()
				.collect();
			(!stale.is_empty()).then_some((*exp, stale))
		})
		.collect()
}

/// Stop then disable a batch of systemd units, best-effort. Used to retire
/// leftover singleton units after a host has migrated to an instanced layout:
/// failures are logged but never abort the caller, since the units are
/// already redundant and the migration's success doesn't hinge on them.
///
/// `stop` works even on a masked unit (systemd only refuses *activation* of
/// masked units, not deactivation); `disable` clears any enablement so the
/// unit can't return on the next boot.
pub async fn retire_systemd_units(units: &[String]) {
	if units.is_empty() {
		return;
	}
	if let Err(e) = systemd::stop(units).await {
		warn!(?units, "could not stop leftover units: {e}");
	}
	if let Err(e) = systemd::disable(units).await {
		warn!(?units, "could not disable leftover units: {e}");
	}
}

/// On Linux/systemd, re-exec the current process under sudo if not
/// already running as root. On pm2 / Windows this is a no-op — pm2
/// manages permissions itself.
///
/// Re-exec is via `Command::status`: the parent waits for sudo to
/// finish, then exits with sudo's status. We don't use `exec` because
/// it requires unsafe and the indirection cost is negligible for a
/// one-shot lifecycle command.
pub fn ensure_root_or_reexec(supervisor: Supervisor) -> Result<()> {
	if !matches!(supervisor, Supervisor::Systemd) {
		return Ok(());
	}
	if privilege::user::privileged() {
		return Ok(());
	}

	info!("not running as root; re-execing under sudo");
	let args: Vec<String> = std::env::args().collect();
	let status = Command::new("sudo").args(args).status().into_diagnostic()?;
	std::process::exit(status.code().unwrap_or(1));
}

/// Poll the supervisor until every target is running, or the timeout
/// elapses. Targets are unit names (systemd) or process names (pm2),
/// matching the input given to `systemctl start` / `pm2 start`.
pub async fn wait_running(supervisor: Supervisor, targets: &[String]) -> Result<()> {
	wait_for(supervisor, targets, true, "active").await
}

/// Mirror of `wait_running`: poll until every target is stopped.
pub async fn wait_stopped(supervisor: Supervisor, targets: &[String]) -> Result<()> {
	wait_for(supervisor, targets, false, "inactive").await
}

/// Issue a stop call to the right supervisor for every target.
///
/// `targets` are supervisor-native identifiers — systemd unit names
/// (`tamanu-foo.service`, `tamanu-foo@1.service`) or pm2 process names.
/// No-op for an empty slice. Bails on supervisor failure; doesn't itself
/// wait for the stop to complete (use `wait_stopped` afterwards).
pub async fn stop_targets(supervisor: Supervisor, targets: &[String]) -> Result<()> {
	if targets.is_empty() {
		return Ok(());
	}
	match supervisor {
		Supervisor::Systemd => systemd::stop(targets).await,
		Supervisor::Pm2 => {
			let status = Command::new("pm2")
				.arg("stop")
				.args(targets)
				.status()
				.into_diagnostic()?;
			if !status.success() {
				bail!("pm2 stop failed: exit {status}");
			}
			Ok(())
		}
	}
}

/// Issue a start call to the right supervisor for every target.
///
/// `targets` are supervisor-native identifiers — systemd unit names or pm2
/// process names (which must already be registered with pm2; this can't
/// create new entries). No-op for an empty slice. Bails on supervisor
/// failure; doesn't itself wait for the start to complete (use
/// `wait_running` afterwards).
pub async fn start_targets(supervisor: Supervisor, targets: &[String]) -> Result<()> {
	if targets.is_empty() {
		return Ok(());
	}
	match supervisor {
		Supervisor::Systemd => systemd::start(targets).await,
		Supervisor::Pm2 => {
			let status = Command::new("pm2")
				.arg("start")
				.args(targets)
				.status()
				.into_diagnostic()?;
			if !status.success() {
				bail!("pm2 start failed: exit {status}");
			}
			Ok(())
		}
	}
}

/// Restart a batch of pm2 targets by stop-then-start with a short pause
/// between, rather than `pm2 restart`. `targets` are pm2-acceptable
/// identifiers (process names, or pm_ids stringified).
///
/// `pm2 restart` has been observed to occasionally leak the previous
/// node process — it shows up as no longer owned by pm2 (and no longer in
/// `pm2 list`) but still holding the TCP port, causing the freshly
/// started replacement to either fail to bind or sit alongside the
/// zombie. Splitting into explicit stop and start with ~1s in between
/// gives the old process time to exit and release its handles before pm2
/// hands them to the new one. No-op for an empty slice.
pub fn pm2_restart_targets(targets: &[String]) -> Result<()> {
	if targets.is_empty() {
		return Ok(());
	}
	let stop = Command::new("pm2")
		.arg("stop")
		.args(targets)
		.status()
		.into_diagnostic()?;
	if !stop.success() {
		bail!("pm2 stop failed: exit {stop}");
	}
	std::thread::sleep(Duration::from_secs(1));
	let start = Command::new("pm2")
		.arg("start")
		.args(targets)
		.status()
		.into_diagnostic()?;
	if !start.success() {
		bail!("pm2 start failed: exit {start}");
	}
	Ok(())
}

/// Delete (i.e. unregister) a batch of pm2 processes. pm2's analogue of
/// `systemctl disable`: removes the process entry from pm2's list entirely,
/// so it won't be picked up by `pm2 resurrect` after the next pm2 restart
/// and won't show up in `pm2 list`. Implies stopping the process if it was
/// running. No-op for an empty slice.
///
/// More aggressive than systemd's `disable`: there's no plain "don't
/// auto-start" toggle on pm2, so re-bringing-up requires the ops setup
/// playbook to re-register the process via the ecosystem file.
pub fn delete_pm2(names: &[String]) -> Result<()> {
	if names.is_empty() {
		return Ok(());
	}
	let status = Command::new("pm2")
		.arg("delete")
		.args(names)
		.status()
		.into_diagnostic()?;
	if !status.success() {
		bail!("pm2 delete failed: exit {status}");
	}
	Ok(())
}

/// Disable a batch of systemd units. No-op for an empty slice. Errors
/// bubble up — callers that want best-effort behaviour should filter the
/// list with `systemd::collect_enabled` first (the typical pattern).
pub async fn disable_systemd_units(units: &[String]) -> Result<()> {
	systemd::disable(units).await
}

async fn wait_for(
	supervisor: Supervisor,
	targets: &[String],
	want_running: bool,
	state_label: &str,
) -> Result<()> {
	let deadline = Instant::now() + Duration::from_secs(60);
	let interval = Duration::from_millis(500);
	loop {
		let mut all_match = true;
		for t in targets {
			if is_running(supervisor, t).await != want_running {
				all_match = false;
				break;
			}
		}
		if all_match {
			return Ok(());
		}
		if Instant::now() >= deadline {
			let mut still_wrong: Vec<&str> = Vec::new();
			for t in targets {
				if is_running(supervisor, t).await != want_running {
					still_wrong.push(t.as_str());
				}
			}
			bail!(
				"timed out after 60s waiting for {} to become {state_label}",
				still_wrong.join(", ")
			);
		}
		tokio::time::sleep(interval).await;
	}
}

/// Restart a single instance, identified by its supervisor-native key
/// (systemd unit name, or pm2 pm_id).
pub async fn restart_one(supervisor: Supervisor, instance: &Instance) -> Result<()> {
	match supervisor {
		Supervisor::Systemd => systemd::restart(&instance.unit()).await,
		Supervisor::Pm2 => {
			let id = instance
				.pm_id
				.ok_or_else(|| miette::miette!("pm2 instance {} has no pm_id", instance.name))?;
			pm2_restart_targets(&[id.to_string()])
		}
	}
}

/// Wait for one specific instance to be running again after a restart.
///
/// For systemd, polls `systemctl is-active`. For pm2, polls jlist and
/// matches by `pm_id` (so we can distinguish individual processes that
/// share a name).
pub async fn wait_running_one(
	supervisor: Supervisor,
	instance: &Instance,
	timeout: Duration,
) -> Result<()> {
	let deadline = Instant::now() + timeout;
	let interval = Duration::from_millis(500);
	loop {
		let up = match supervisor {
			Supervisor::Systemd => is_running(supervisor, &instance.unit()).await,
			Supervisor::Pm2 => is_pm2_pm_id_online(instance.pm_id),
		};
		if up {
			return Ok(());
		}
		if Instant::now() >= deadline {
			bail!(
				"timed out after {}s waiting for {} to become active",
				timeout.as_secs(),
				instance.display(),
			);
		}
		tokio::time::sleep(interval).await;
	}
}

fn is_pm2_pm_id_online(pm_id: Option<i64>) -> bool {
	let Some(id) = pm_id else { return false };
	match pm2::list() {
		Ok((procs, _)) => procs.iter().any(|p| p.pm_id == Some(id) && p.running),
		Err(_) => false,
	}
}

/// Reload caddy (Linux: + flush systemd-resolved). Needed after
/// restarting a containerised tamanu service: caddy's upstream list is by
/// hostname, resolved caches IPs, and the restarted container has a new
/// IP. All calls are best-effort: failures are logged but don't bail.
///
/// Uses the platform-native path that reads the on-disk Caddyfile:
///
/// - Linux: `systemctl reload caddy`. We deliberately don't POST the
///   Caddyfile to the admin API here even though it'd work for a
///   single-file config — production deployments split their Caddyfile
///   across `/etc/caddy/conf.d/*` includes, and the safest way to pick
///   up every fragment is to let caddy re-read from disk the same way
///   it did at startup.
/// - Windows: tries the admin API first (`POST localhost:2019/load`
///   with the Caddyfile content) since it's the most reliable when
///   available; falls back to `caddy reload --config <path> --adapter
///   caddyfile` if the admin endpoint is unreachable.
///
/// On Linux, also runs `resolvectl flush-caches` regardless of which
/// reload path actually fired — systemd-resolved caches DNS independently
/// of caddy's upstream list.
///
/// Mirror of the ansible "Reload caddy" handler from #313.
#[cfg(target_os = "linux")]
pub async fn reload_caddy() {
	match systemd::reload("caddy.service").await {
		Ok(()) => debug!("caddy reloaded"),
		Err(e) => warn!("could not reload caddy: {e}"),
	}
	let status = Command::new("resolvectl").arg("flush-caches").status();
	match status {
		Ok(s) if s.success() => debug!("resolvectl flush-caches OK"),
		Ok(s) => warn!("resolvectl flush-caches exited with {s}"),
		Err(e) => debug!("resolvectl not available: {e}"),
	}
}

#[cfg(target_os = "windows")]
pub async fn reload_caddy() {
	let path = r"C:\Caddy\Caddyfile";
	match reload_caddy_via_admin_api(path).await {
		Ok(()) => {
			debug!("caddy reloaded via admin API");
			return;
		}
		Err(err) => debug!(%err, "caddy admin API unreachable, falling back to CLI"),
	}
	// `caddy reload` reads the file, adapts it, and POSTs to the admin
	// API on the caddy process's behalf. Same end-state as the admin-API
	// path above; this fallback handles the case where caddy is running
	// but the admin endpoint is locked down or relocated.
	let status = Command::new(bestool_tamanu::caddy::program())
		.args(["reload", "--config", path, "--adapter", "caddyfile"])
		.status();
	match status {
		Ok(s) if s.success() => debug!("caddy reloaded via CLI"),
		Ok(s) => warn!("caddy reload exited with {s}"),
		Err(e) => warn!("could not run caddy reload: {e}"),
	}
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub async fn reload_caddy() {
	debug!("caddy reload is unsupported on this OS");
}

/// POST the Caddyfile content to Caddy's admin API at the default
/// `localhost:2019/load` endpoint with `Content-Type: text/caddyfile`,
/// telling Caddy to adapt + load it in-process. Returns an error string
/// (with enough context to debug) if the file can't be read, the API
/// doesn't respond, or it returns a non-2xx — the caller logs at debug
/// and falls back to the platform CLI.
#[cfg(target_os = "windows")]
async fn reload_caddy_via_admin_api(path: &str) -> std::result::Result<(), String> {
	let content = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
	let client = crate::http::client_builder()
		.timeout(Duration::from_secs(5))
		.build()
		.map_err(|e| format!("build client: {e}"))?;
	let resp = client
		.post("http://localhost:2019/load")
		.header("Content-Type", "text/caddyfile")
		.body(content)
		.send()
		.await
		.map_err(|e| format!("POST localhost:2019/load: {e}"))?;
	if !resp.status().is_success() {
		let status = resp.status();
		let body = resp.text().await.unwrap_or_default();
		return Err(format!("admin API returned {status}: {body}"));
	}
	Ok(())
}

/// Look up the netavark IP of the podman container backing a systemd
/// unit. Returns None if there's no matching container or no IP yet
/// (e.g. container not finished starting). Mirror of #313's helper.
pub fn container_ip_for_unit(unit: &str) -> Result<Option<std::net::IpAddr>> {
	let ps = Command::new("podman")
		.args([
			"ps",
			"--filter",
			&format!("label=PODMAN_SYSTEMD_UNIT={unit}"),
			"--format",
			"json",
		])
		.output();
	let ps = match ps {
		Ok(o) if o.status.success() => o,
		Ok(o) => {
			warn!(
				"podman ps failed: {}",
				String::from_utf8_lossy(&o.stderr).trim()
			);
			return Ok(None);
		}
		Err(e) => {
			debug!("podman not available: {e}");
			return Ok(None);
		}
	};

	let entries: Vec<serde_json::Value> = serde_json::from_slice(&ps.stdout).into_diagnostic()?;
	let Some(id) = entries.first().and_then(|c| c["Id"].as_str()) else {
		return Ok(None);
	};

	let inspect = Command::new("podman")
		.args(["inspect", id])
		.output()
		.into_diagnostic()?;
	if !inspect.status.success() {
		bail!(
			"podman inspect failed: {}",
			String::from_utf8_lossy(&inspect.stderr).trim()
		);
	}
	let inspects: Vec<serde_json::Value> =
		serde_json::from_slice(&inspect.stdout).into_diagnostic()?;
	let ip = inspects
		.first()
		.and_then(|c| c["NetworkSettings"]["Networks"].as_object())
		.and_then(|nets| nets.values().find_map(|n| n["IPAddress"].as_str()))
		.filter(|s| !s.is_empty())
		.map(|s| s.parse::<std::net::IpAddr>())
		.transpose()
		.into_diagnostic()?;
	Ok(ip)
}

/// Read pm2's view of an instance to find its listening port from the
/// `PORT` env var. Returns None if pm2 doesn't expose one — e.g.
/// workers that don't open a socket.
pub fn pm2_port_for(pm_id: i64) -> Result<Option<u16>> {
	let output = Command::new("pm2").arg("jlist").output().into_diagnostic()?;
	if !output.status.success() {
		bail!(
			"pm2 jlist failed: {}",
			String::from_utf8_lossy(&output.stderr).trim()
		);
	}
	let entries: Vec<serde_json::Value> =
		serde_json::from_slice(&output.stdout).into_diagnostic()?;
	let port = entries
		.iter()
		.find(|p| p["pm_id"].as_i64() == Some(pm_id))
		.and_then(|p| p["pm2_env"].get("env"))
		.and_then(|env| env.get("PORT"))
		.and_then(|v| {
			v.as_str()
				.and_then(|s| s.parse().ok())
				.or_else(|| v.as_u64().and_then(|n| u16::try_from(n).ok()))
		});
	Ok(port)
}

async fn is_running(supervisor: Supervisor, target: &str) -> bool {
	match supervisor {
		Supervisor::Systemd => systemd::is_active(target).await.unwrap_or(false),
		Supervisor::Pm2 => match pm2::list() {
			Ok((procs, _)) => procs.iter().any(|p| p.name == target && p.running),
			Err(_) => false,
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use bestool_tamanu::services::{ExpectedState, Instances};

	fn exp(name: &'static str) -> Expectation {
		Expectation {
			name,
			instances: Instances::Single,
			state: ExpectedState::Up,
			reason: "test".into(),
			legacy: false,
			behind_caddy: false,
		}
	}

	fn templated_exp(name: &'static str) -> Expectation {
		Expectation {
			name,
			instances: Instances::NumericAtLeast(2),
			state: ExpectedState::Up,
			reason: "test".into(),
			legacy: false,
			behind_caddy: false,
		}
	}

	fn inst(name: &str, instance: Option<&str>, running: bool) -> Instance {
		Instance {
			name: name.to_string(),
			instance: instance.map(str::to_string),
			pm_id: None,
			running,
		}
	}

	#[test]
	fn group_by_expectation_collects_matching_instances() {
		let api = templated_exp("tamanu-central-api");
		let tasks = exp("tamanu-central-tasks");
		let expectations = [&api, &tasks];
		let instances = vec![
			inst("tamanu-central-api", Some("1"), true),
			inst("tamanu-central-api", Some("2"), true),
			inst("tamanu-central-tasks", None, true),
			inst("tamanu-orphan", None, true), // unrelated, dropped
		];
		let groups = group_by_expectation(Supervisor::Systemd, &expectations, &instances);
		assert_eq!(groups.len(), 2);
		assert_eq!(groups[0].0.name, "tamanu-central-api");
		assert_eq!(groups[0].1.len(), 2);
		assert_eq!(groups[1].0.name, "tamanu-central-tasks");
		assert_eq!(groups[1].1.len(), 1);
	}

	fn named_exp(name: &'static str, names: &'static [&'static str]) -> Expectation {
		Expectation {
			name,
			instances: Instances::Named(names),
			state: ExpectedState::Up,
			reason: "test".into(),
			legacy: false,
			behind_caddy: true,
		}
	}

	#[test]
	fn group_by_expectation_excludes_leftover_singleton_on_systemd() {
		// Host mid-migration: the @a/@b template is installed (so the portal
		// expectation is instanced) but an old singleton `tamanu-patientportal`
		// is still running. On systemd that singleton must NOT be grouped under
		// the instanced expectation — otherwise restart would try to roll the
		// bare (and possibly masked) `.service` unit.
		let portal = named_exp("tamanu-patientportal", &["a", "b"]);
		let expectations = [&portal];
		let instances = vec![
			inst("tamanu-patientportal", None, true),
			inst("tamanu-patientportal", Some("a"), true),
			inst("tamanu-patientportal", Some("b"), true),
		];
		let groups = group_by_expectation(Supervisor::Systemd, &expectations, &instances);
		assert_eq!(groups.len(), 1);
		let matched: Vec<Option<&str>> =
			groups[0].1.iter().map(|i| i.instance.as_deref()).collect();
		assert_eq!(matched, vec![Some("a"), Some("b")]);
	}

	#[test]
	fn stale_shape_groups_finds_leftover_singleton() {
		let portal = named_exp("tamanu-patientportal", &["a", "b"]);
		let expectations = [&portal];
		let instances = vec![
			inst("tamanu-patientportal", None, true),
			inst("tamanu-patientportal", Some("a"), true),
			inst("tamanu-patientportal", Some("b"), true),
		];
		let stale = stale_shape_groups(Supervisor::Systemd, &expectations, &instances);
		assert_eq!(stale.len(), 1);
		assert_eq!(stale[0].0.name, "tamanu-patientportal");
		assert_eq!(stale[0].1.len(), 1);
		assert_eq!(stale[0].1[0].instance, None);
	}

	#[test]
	fn stale_shape_groups_ignores_stopped_singleton() {
		// A leftover singleton that isn't running needs no retiring — it's
		// already down. (It'd surface as a Down-expectation concern elsewhere,
		// not as something restart should stop.)
		let portal = named_exp("tamanu-patientportal", &["a", "b"]);
		let expectations = [&portal];
		let instances = vec![inst("tamanu-patientportal", None, false)];
		let stale = stale_shape_groups(Supervisor::Systemd, &expectations, &instances);
		assert!(stale.is_empty());
	}

	#[test]
	fn stale_shape_groups_ignores_singleton_expectation() {
		// On an un-migrated host the expectation is itself a singleton, so the
		// running `None` unit is the real thing, not a leftover.
		let portal = exp("tamanu-patientportal");
		let expectations = [&portal];
		let instances = vec![inst("tamanu-patientportal", None, true)];
		let stale = stale_shape_groups(Supervisor::Systemd, &expectations, &instances);
		assert!(stale.is_empty());
	}

	#[test]
	fn stale_shape_groups_empty_on_pm2() {
		// pm2 clusters legitimately share one name with no @suffix, so a `None`
		// instance under a templated expectation is a member, not a leftover.
		let api = templated_exp("tamanu-api");
		let expectations = [&api];
		let instances = vec![
			inst("tamanu-api", None, true),
			inst("tamanu-api", None, true),
		];
		let stale = stale_shape_groups(Supervisor::Pm2, &expectations, &instances);
		assert!(stale.is_empty());
	}

	#[test]
	fn stale_shape_groups_skips_down_and_unknown() {
		let mut down = named_exp("tamanu-patientportal", &["a", "b"]);
		down.state = ExpectedState::Down;
		let mut unknown = named_exp("tamanu-patientportal", &["a", "b"]);
		unknown.state = ExpectedState::Unknown;
		let running = vec![inst("tamanu-patientportal", None, true)];
		for exp in [&down, &unknown] {
			let expectations = [exp];
			let stale = stale_shape_groups(Supervisor::Systemd, &expectations, &running);
			assert!(stale.is_empty(), "{:?} should not retire", exp.state);
		}
	}

	#[test]
	fn instance_unit_and_display() {
		let templated = inst("tamanu-frontend", Some("a"), true);
		assert_eq!(templated.unit(), "tamanu-frontend@a.service");
		assert_eq!(templated.display(), "tamanu-frontend@a");

		let singleton = inst("tamanu-tasks", None, true);
		assert_eq!(singleton.unit(), "tamanu-tasks.service");
		assert_eq!(singleton.display(), "tamanu-tasks");

		let pm2 = Instance {
			name: "tamanu-api".into(),
			instance: None,
			pm_id: Some(3),
			running: true,
		};
		assert_eq!(pm2.display(), "tamanu-api#3");
	}

	/// A postgres URL pointing at a port nothing should listen on, so
	/// `connect_one` returns an error fast and the retry loop runs the
	/// timing-out path without waiting on real I/O.
	const UNREACHABLE_DB_URL: &str = "postgres://localhost:1/x";

	#[tokio::test]
	async fn wait_for_db_no_returns_none_after_single_attempt() {
		let start = Instant::now();
		let result = query_patient_portal_enabled_with_wait_inner(
			UNREACHABLE_DB_URL,
			WaitForDb::No,
			Duration::from_secs(10),
			Duration::from_millis(50),
		)
		.await;
		assert_eq!(result, None, "DB unreachable must surface as Unknown, not a guess");
		assert!(
			start.elapsed() < Duration::from_secs(5),
			"WaitForDb::No should not loop; took {:?}",
			start.elapsed(),
		);
	}

	#[tokio::test]
	async fn wait_for_db_yes_retries_until_timeout() {
		// Short timeout + short interval keeps the test fast. The point is
		// that we make >1 attempt before giving up.
		let start = Instant::now();
		let result = query_patient_portal_enabled_with_wait_inner(
			UNREACHABLE_DB_URL,
			WaitForDb::Yes,
			Duration::from_millis(300),
			Duration::from_millis(50),
		)
		.await;
		assert_eq!(result, None, "should surface as Unknown after timeout");
		assert!(
			start.elapsed() >= Duration::from_millis(300),
			"should have waited at least the timeout; took {:?}",
			start.elapsed(),
		);
	}
}
