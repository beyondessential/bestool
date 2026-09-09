use std::{
	collections::{HashMap, HashSet},
	path::{Path, PathBuf},
	sync::Arc,
};

use bestool_canopy::{
	CanopyClient,
	schema::{
		ApplicationReport, CheckResult, CheckSeverity, HealthCheck, StatusPayload, StatusResponse,
		TargetReport,
	},
};
use futures::{
	future::BoxFuture,
	stream::{FuturesUnordered, StreamExt},
};
use miette::{IntoDiagnostic, Result, miette};
use node_semver::Version;
use serde_json::{Map, Value};
use tracing::{debug, warn};

use bestool_tamanu::{config::TamanuConfig, server_info::get_or_create_machine_id};

use crate::doctor::{
	check::{Check, CheckOutcome, OverallResult},
	checks::{self, CheckContext, SweepContext},
	heal,
	progress::{DoctorEvent, ProgressSender},
	server_info::{self, ServerFacts},
	subject::{ApplicationKind, ApplicationRef, CheckScope, Subject},
};

/// The name bestool's daemon reports under.
///
/// Canopy attributes a push without one to `alertd` today, but the field is
/// becoming mandatory, so it is always sent.
///
/// spec: SUBJ
pub const REPORTING_SOURCE: &str = "alertd";

/// Ceiling applied to a check that canopy's severity map doesn't mention.
///
/// A check absent from the map is one canopy doesn't yet know about (it's newer
/// than the deployment), which canopy's own contract classifies as `warn` until
/// it catches up. Matching that here keeps a local sweep's verdict in step with
/// what canopy would show.
const ABSENT_CHECK_SEVERITY: CheckSeverity = CheckSeverity::Warn;

/// The ceiling to apply to the check named `name`, honouring the absent-check
/// default ([`ABSENT_CHECK_SEVERITY`]).
pub fn severity_ceiling(severities: &HashMap<String, CheckSeverity>, name: &str) -> CheckSeverity {
	severities
		.get(name)
		.copied()
		.unwrap_or(ABSENT_CHECK_SEVERITY)
}

/// Environment variable holding a generic (non-Tamanu) `postgresql://` URL.
///
/// Consulted only when there is no Tamanu install and no
/// [`TAMANU_DATABASE_URL`] override: the sweep still gets a database to run
/// the generic postgres checks against, while Tamanu-specific checks skip.
///
/// [`TAMANU_DATABASE_URL`]: bestool_tamanu::config::DATABASE_URL_ENV
pub const GENERIC_DATABASE_URL_ENV: &str = "DATABASE_URL";

/// The [`GENERIC_DATABASE_URL_ENV`] fallback, if set to a non-empty value.
fn generic_database_url() -> Option<String> {
	std::env::var(GENERIC_DATABASE_URL_ENV)
		.ok()
		.filter(|s| !s.is_empty())
}

/// The database context a sweep runs against, when the host has one.
///
/// Usually a Tamanu deployment; with only a generic [`GENERIC_DATABASE_URL_ENV`]
/// it's a bare postgres and `is_tamanu` is false.
#[derive(Clone)]
pub struct SweepTamanu {
	pub version: Version,
	pub root: PathBuf,
	pub config: Arc<TamanuConfig>,
	pub database_url: String,
	/// `false` when this was synthesised from a database URL with no Tamanu
	/// install on the host: DB checks run, but install-dependent ones
	/// (the install metadata, local HTTP, caddy, services, kopia) skip.
	pub has_install: bool,
	/// `false` when the URL came from the generic [`GENERIC_DATABASE_URL_ENV`]
	/// fallback: it points at a postgres that isn't necessarily Tamanu's, so
	/// only the generic database checks run against it and Tamanu-specific
	/// ones (which query Tamanu tables) skip.
	pub is_tamanu: bool,
}

/// Discover the host's Tamanu install and resolve the sweep's database context
/// from it, honouring an explicit `root` override.
///
/// Cheap enough to redo before every sweep, which is what the daemon does: an
/// in-place upgrade changes the version, the install root, and the config, and
/// only re-running discovery picks that up.
pub async fn discover_sweep_tamanu(root: Option<&Path>) -> Result<Option<SweepTamanu>> {
	resolve_sweep_tamanu(bestool_tamanu::try_find_tamanu(root).await?)
}

/// Resolve the database context for a sweep from an optionally-discovered
/// install.
///
/// * `Some(install)` → a real install: its config is loaded and `has_install`
///   is true.
/// * no install but [`TAMANU_DATABASE_URL`] set → a DB-only context synthesised
///   from that URL (`has_install` false) so DB checks still run against it.
/// * no install but [`GENERIC_DATABASE_URL_ENV`] set → a generic (non-Tamanu)
///   database context (`is_tamanu` false): the generic DB checks run, all
///   Tamanu-specific ones skip.
/// * none of those → `None`: host-level checks only.
///
/// [`TAMANU_DATABASE_URL`]: bestool_tamanu::config::DATABASE_URL_ENV
pub fn resolve_sweep_tamanu(install: Option<(Version, PathBuf)>) -> Result<Option<SweepTamanu>> {
	resolve_sweep_tamanu_from(
		install,
		bestool_tamanu::config::database_url_override(),
		generic_database_url(),
	)
}

/// [`resolve_sweep_tamanu`] with the environment reads made explicit, so the
/// resolution order is testable without mutating process-global env vars.
fn resolve_sweep_tamanu_from(
	install: Option<(Version, PathBuf)>,
	tamanu_url: Option<String>,
	generic_url: Option<String>,
) -> Result<Option<SweepTamanu>> {
	use bestool_tamanu::config::{Database, TamanuConfig, load_config};

	match install {
		Some((version, root)) => {
			let config = load_config(&root, None)?;
			let database_url = config.database_url();
			Ok(Some(SweepTamanu {
				version,
				root,
				config: Arc::new(config),
				database_url,
				has_install: true,
				is_tamanu: true,
			}))
		}
		None => {
			let (url, is_tamanu) = match (tamanu_url, generic_url) {
				(Some(url), _) => (url, true),
				(None, Some(url)) => (url, false),
				(None, None) => return Ok(None),
			};
			let db = Database::from_url(&url)?;
			Ok(Some(SweepTamanu {
				version: Version::parse("0.0.0").into_diagnostic()?,
				root: PathBuf::new(),
				config: Arc::new(TamanuConfig::from_database(db)),
				database_url: url,
				has_install: false,
				is_tamanu,
			}))
		}
	}
}

/// Every check's severity ceiling, split by the subject it was filed against.
///
/// Canopy answers per target, keyed by bare check name, so a machine check and
/// an application check sharing a name are graded separately.
#[derive(Debug, Clone, Default)]
pub struct SplitSeverities {
	pub machine: HashMap<String, CheckSeverity>,
	/// Keyed by application key, as the push named them.
	pub applications: HashMap<String, HashMap<String, CheckSeverity>>,
	/// Applied to any application the map does not name, for the ungrouped
	/// endpoint that answers by bare check name alone.
	pub fallback: Option<HashMap<String, CheckSeverity>>,
}

impl SplitSeverities {
	/// One map governing every subject alike.
	///
	/// For the ungrouped `status_check_severities` endpoint, which predates the
	/// split and answers by bare check name alone: the same ceiling applies
	/// wherever that name is filed.
	pub fn flat(severities: HashMap<String, CheckSeverity>) -> Self {
		Self {
			machine: severities.clone(),
			applications: HashMap::new(),
			fallback: Some(severities),
		}
	}

	/// The ceilings governing `subject`, or `None` when canopy said nothing
	/// about it — in which case every check falls to the absent-check default.
	pub fn for_subject(&self, subject: &Subject) -> Option<&HashMap<String, CheckSeverity>> {
		match subject.key() {
			None => Some(&self.machine),
			Some(key) => self.applications.get(key).or(self.fallback.as_ref()),
		}
	}
}

/// Read canopy's per-target severity ceilings out of a status response.
///
/// A canopy that predates the split format answers with neither a `machine` nor
/// an `applications` target, only the ungrouped top-level map. Both grains fall
/// back to it in that case, because the alternative is every check dropping to
/// the absent-check ceiling and a genuine failure rendering as a warning.
///
/// When canopy *does* answer per target, a key it omits is an application it
/// does not yet hold — freshly reported — and its checks are genuinely unknown
/// to canopy, so the absent-check default is the right answer and no fallback
/// applies.
pub fn split_severities(response: &StatusResponse) -> SplitSeverities {
	SplitSeverities {
		machine: response
			.machine
			.as_ref()
			.map(|t| t.check_severities.clone())
			.unwrap_or_else(|| response.check_severities.clone()),
		applications: response
			.applications
			.iter()
			.flatten()
			.map(|(key, target)| (key.clone(), target.check_severities.clone()))
			.collect(),
		fallback: response
			.applications
			.is_none()
			.then(|| response.check_severities.clone()),
	}
}

#[derive(Clone)]
pub struct SweepResult {
	/// This machine's Canopy identity. Not the OS `/etc/machine-id`.
	pub machine_id: Option<String>,
	pub results: Vec<CheckOutcome>,
	pub overall: OverallResult,
	pub payload: StatusPayload,
	/// `SELECT version()` result observed during this sweep, available so
	/// callers (e.g. the daemon plugin) can cache it across ticks instead of
	/// re-querying every minute.
	pub pg_version: Option<String>,
}

impl SweepResult {
	/// Lower each check's status to canopy's effective-severity ceiling for the
	/// subject it was filed against, then re-derive the overall result and each
	/// target's wire `health[]` to match.
	///
	/// This is a display-time transform for local consumers (the `doctor` CLI):
	/// the payload posted to canopy is always the raw one, since canopy is the
	/// source of truth for severities and applies the mapping itself. See
	/// [`CheckStatus::cap_to`](crate::doctor::check::CheckStatus::cap_to) for the
	/// ceiling semantics and [`severity_ceiling`] for the absent-check default.
	pub fn apply_severities(&mut self, severities: &SplitSeverities) {
		let empty = HashMap::new();
		for outcome in &mut self.results {
			let map = severities.for_subject(&outcome.subject).unwrap_or(&empty);
			let ceiling = severity_ceiling(map, outcome.check.name);
			outcome.check.status = outcome.check.status.clone().cap_to(ceiling);
		}
		self.overall = OverallResult::from_statuses(self.results.iter().map(|o| &o.check.status));
		refresh_wire_health(&mut self.payload, &self.results);
	}
}

/// Rewrite each target's `health[]` from the current results, leaving every
/// `detail` block as it was.
fn refresh_wire_health(payload: &mut StatusPayload, results: &[CheckOutcome]) {
	if let Some(machine) = payload.machine.as_mut() {
		machine.health = Some(health_for(results, None));
	}
	if let Some(apps) = payload.applications.as_mut() {
		for (key, report) in apps.iter_mut() {
			report.health = Some(health_for(results, Some(key)));
		}
	}
}

/// A single check ready to run: its registry index, the subject it reports for
/// and its wire flag, the future that produces its result, and its heal action
/// paired with the context to run it against (when the sweep enables healing
/// and the check has one).
struct PreparedCheck {
	idx: usize,
	name: &'static str,
	subject: Subject,
	on_wire: bool,
	fut: BoxFuture<'static, Check>,
	heal: Option<(heal::HealAction, SweepContext)>,
}

/// The subject a check reports for on this sweep, or `None` when the sweep has
/// no such subject and the check is therefore not run at all.
///
/// A machine is always present. An application is present only when the host
/// has one, so on a host with no Tamanu every Tamanu check is simply absent
/// rather than reported as skipped, and likewise for Postgres.
///
/// spec: SUBJ
fn subjects_for(scope: CheckScope, applications: &[ApplicationRef]) -> Vec<Subject> {
	if scope.admits(&Subject::Machine) {
		return vec![Subject::Machine];
	}
	applications
		.iter()
		.cloned()
		.map(Subject::Application)
		.filter(|subject| scope.admits(subject))
		.collect()
}

/// The Postgres application a connection string reaches.
///
/// Identified by the port it answers on, never by its version: an in-place
/// major upgrade must not read as one application stopping and another
/// starting. The port is the one identifier every connection form carries — a
/// Unix socket is named `.s.PGSQL.<port>` — and libpq's 5432 default applies
/// when the string leaves it out.
///
/// A cluster reached at an address that is not this machine is keyed apart, so
/// the `host-` prefix never claims the machine hosts something it does not.
///
/// spec: SUBJ
fn postgres_ref(url: &str) -> ApplicationRef {
	use tokio_postgres::config::Host;

	// A string this cannot parse is one the sweep cannot connect with either, so
	// the cluster is reported at the default port and `connect` fails loudly.
	// Reporting nothing would drop the database's reachability check entirely,
	// and being unable to alert on the database is the one thing alertd must
	// never do.
	let Ok(config) = url.parse::<tokio_postgres::Config>() else {
		warn!("database connection string is unparseable; reporting it at the default port");
		return ApplicationRef::local_postgres(5432);
	};

	let hosts = config.get_hosts();

	// The first host that is not this machine's names the cluster. Checking for
	// one rather than requiring every host to be remote keeps this in step with
	// `database_is_local`, which treats a list as local only when all of it is.
	let remote = hosts
		.iter()
		.enumerate()
		.find_map(|(index, host)| match host {
			Host::Tcp(name) if !host_is_local(name) => Some((index, name.as_str())),
			_ => None,
		});

	match remote {
		Some((index, host)) => ApplicationRef::remote_postgres(host, port_at(&config, index)),
		None => ApplicationRef::local_postgres(port_at(&config, 0)),
	}
}

/// The port serving the host at `index`.
///
/// A connection string carries either one port for every host or one per host,
/// so the port must be taken at the chosen host's position rather than from the
/// front of the list. Absent, it is libpq's 5432.
fn port_at(config: &tokio_postgres::Config, index: usize) -> u16 {
	let ports = config.get_ports();
	ports
		.get(index)
		.or_else(|| ports.first())
		.copied()
		.unwrap_or(5432)
}

/// Whether a TCP host name refers to this machine.
fn host_is_local(name: &str) -> bool {
	name.is_empty()
		|| name.eq_ignore_ascii_case("localhost")
		|| name
			.parse::<std::net::IpAddr>()
			.is_ok_and(|ip| ip.is_loopback())
}

/// Whether a database connection string points at this machine.
///
/// A Postgres is only this machine's application when it actually runs here: a
/// deployment pointed at a database on another box hosts no Postgres, and
/// grading that server's tuning against this machine's memory would be wrong
/// rather than merely imprecise.
///
/// Parsed with the same connection-string parser used to open the connection,
/// so every form that connects — Unix sockets, host lists, percent-encoded
/// credentials — is judged the way it will actually be reached.
///
/// spec: SUBJ
pub fn database_is_local(url: &str) -> bool {
	use tokio_postgres::config::Host;

	let Ok(config) = url.parse::<tokio_postgres::Config>() else {
		// Unparseable means the sweep cannot connect either, so there is nothing
		// here to call a local application.
		return false;
	};

	let hosts = config.get_hosts();
	if hosts.is_empty() {
		// No host at all is libpq's default of a local socket.
		return true;
	}

	hosts.iter().all(|host| match host {
		// A socket is on the machine that serves it, by construction. The variant
		// only exists on platforms that have Unix sockets, so the arm is gated to
		// match — on Windows a host list is TCP or nothing.
		#[cfg(unix)]
		Host::Unix(_) => true,
		Host::Tcp(name) => host_is_local(name),
	})
}

/// Every `subject:name` the registry can produce, in registry order.
///
/// Answers from the registry rather than from what this host runs, so the same
/// invocation is an error for the same reason on every machine.
pub fn known_qualified_names(registry: &[checks::CheckEntry]) -> Vec<String> {
	registry
		.iter()
		.flat_map(|entry| {
			entry
				.scope
				.possible_slugs()
				.into_iter()
				.map(|slug| format!("{slug}:{}", entry.name))
		})
		.collect()
}

/// Reject a selection flag that names a check bestool cannot file.
///
/// A bare name is an error rather than a wildcard: a name identifies a check
/// only together with its subject. The error names the qualified forms that do
/// exist, so an operator who types a bare name is told what to write instead.
///
/// spec: DOC
pub fn validate_selection(
	registry: &[checks::CheckEntry],
	names: &[String],
	flag: &str,
) -> Result<()> {
	// The default path names nothing, so building the known-name list would be
	// pure waste — and this runs twice per sweep and twice more in the CLI.
	if names.is_empty() {
		return Ok(());
	}

	let known: HashSet<String> = known_qualified_names(registry).into_iter().collect();
	for name in names {
		if !name.contains(':') {
			let forms: Vec<String> = registry
				.iter()
				.filter(|entry| entry.name == name)
				.flat_map(|entry| {
					entry
						.scope
						.possible_slugs()
						.into_iter()
						.map(|slug| format!("{slug}:{}", entry.name))
				})
				.collect();
			return Err(if forms.is_empty() {
				miette!(
					"unknown check `{name}` in {flag}; checks are named by subject, e.g. `machine:disk_free`"
				)
			} else {
				miette!(
					"`{name}` in {flag} needs the subject it reports for: {}",
					forms.join(", ")
				)
			});
		}
		if !known.contains(name) {
			let mut sorted: Vec<&str> = known.iter().map(String::as_str).collect();
			sorted.sort_unstable();
			return Err(miette!(
				"unknown check `{name}` in {flag}; known checks: {}",
				sorted.join(", ")
			));
		}
	}
	Ok(())
}

/// Drive a set of checks concurrently, each on its own task.
///
/// spec: CHK
///
/// Spawning rather than pushing bare futures into one `FuturesUnordered` keeps
/// the checks off a single shared driver task: a blocking call in one check
/// can't stall another's in-flight future and corrupt its latency measurement
/// (an `Instant` straddling an `.await` counts wall-clock spent unpolled). A
/// panicking check surfaces as a `broken` result for that check alone rather
/// than taking the whole sweep down.
async fn run_checks_concurrently(
	checks: Vec<PreparedCheck>,
	progress: Option<&ProgressSender>,
) -> Vec<(usize, CheckOutcome)> {
	let mut pending = FuturesUnordered::new();
	for PreparedCheck {
		idx,
		name,
		subject,
		on_wire,
		fut,
		heal,
	} in checks
	{
		let task = tokio::spawn(async move {
			let result = fut.await;
			if let Some((heal, heal_ctx)) = heal
				&& result.status.is_fatal()
			{
				heal::spawn_if_due(name, heal, heal_ctx);
			}
			result
		});
		pending.push(async move { (idx, name, subject, on_wire, task.await) });
	}

	let mut completed: Vec<(usize, CheckOutcome)> = Vec::with_capacity(pending.len());
	while let Some((idx, name, subject, on_wire, joined)) = pending.next().await {
		let check = match joined {
			Ok(check) => check,
			Err(err) => {
				warn!(check = name, error = %err, "doctor check task did not complete");
				Check::broken(name, "check did not complete", err.to_string())
			}
		};
		let outcome = CheckOutcome {
			subject,
			check,
			on_wire,
		};
		if let Some(tx) = progress {
			let _ = tx.send(DoctorEvent::Completed(outcome.clone()));
		}
		completed.push((idx, outcome));
	}
	completed
}

#[expect(
	clippy::too_many_arguments,
	reason = "a sweep's inputs; grouping them into a params struct is a separate refactor"
)]
pub async fn perform_sweep(
	binary_version: &str,
	tamanu: Option<SweepTamanu>,
	http_client: reqwest::Client,
	selected_names: &[String],
	skip_names: &[String],
	cached_pg_version: Option<String>,
	progress: Option<ProgressSender>,
	canopy: Option<Arc<CanopyClient>>,
	enable_heal: bool,
) -> Result<SweepResult> {
	let tamanu_ctx = match &tamanu {
		Some(t) => {
			// Open a single connection up-front. Checks that need the DB share
			// it; the `db_connect` check separately measures the open latency
			// for reporting. Goes through `bestool_postgres::pool::connect_one`
			// so all DB opens in the project share one SSL fallback / auth
			// retry / app-name path.
			let db =
				match bestool_postgres::pool::connect_one(&t.database_url, "bestool-tamanu-doctor")
					.await
				{
					Ok(client) => Some(Arc::new(client)),
					Err(err) => {
						warn!(%err, "doctor could not open Tamanu DB; DB-dependent checks will skip");
						None
					}
				};

			// A generic (non-Tamanu) database has no Tamanu tables to inspect,
			// so don't probe it for kind or version; the value is unused since
			// every Tamanu-dependent check skips.
			let kind = if t.is_tamanu {
				let kind = bestool_tamanu::detect_kind(&t.config, db.as_deref()).await;
				debug!(?kind, "detected Tamanu server kind for doctor sweep");
				kind
			} else {
				bestool_tamanu::ApiServerKind::Central
			};

			// With a real install, the version is the env-file/install version.
			// Without one (a `TAMANU_DATABASE_URL`-only host), fall back to the
			// version Tamanu last recorded in its own DB (`currentVersion`), so
			// version-aware checks can still run against it.
			let tamanu_version = match (t.has_install, db.as_deref()) {
				(false, Some(client)) if t.is_tamanu => {
					bestool_tamanu::versions::current_version(client)
						.await
						.unwrap_or_else(|| t.version.clone())
				}
				_ => t.version.clone(),
			};

			Some(CheckContext {
				tamanu_version,
				tamanu_root: t.root.clone(),
				config: t.config.clone(),
				kind,
				database_url: t.database_url.clone(),
				db,
				http_client: http_client.clone(),
				has_install: t.has_install,
				is_tamanu: t.is_tamanu,
			})
		}
		None => None,
	};
	let db = tamanu_ctx.as_ref().and_then(|c| c.db.clone());
	// The version resolved above (install version, or the DB's `currentVersion`
	// for a database-only host), kept for the wire payload after `tamanu_ctx` is
	// moved into the check context below. The server kind and (when there's a
	// real install) its root go into the top-level status facts too.
	let resolved_version = tamanu_ctx
		.as_ref()
		.filter(|c| c.is_tamanu)
		.map(|c| c.tamanu_version.clone());
	let tamanu_server_kind = tamanu_ctx
		.as_ref()
		.filter(|c| c.is_tamanu)
		.map(|c| match c.kind {
			bestool_tamanu::ApiServerKind::Central => "central",
			bestool_tamanu::ApiServerKind::Facility => "facility",
		});
	let tamanu_root = tamanu
		.as_ref()
		.filter(|t| t.has_install)
		.map(|t| t.root.display().to_string());

	// The applications this sweep reports for. A Tamanu deployment is one; the
	// Postgres under it is another, whether a Tamanu uses it or the host has
	// nothing but a `DATABASE_URL`. A cluster reached at a remote address is
	// still reported, keyed apart so no key claims this machine hosts it.
	let mut applications: Vec<ApplicationRef> = Vec::new();
	if let Some(ctx) = tamanu_ctx.as_ref() {
		if ctx.is_tamanu {
			applications.push(ApplicationRef::tamanu(ApplicationKind::from(ctx.kind)));
		}
		applications.push(postgres_ref(&ctx.database_url));
	}

	let check_ctx = SweepContext::builder()
		.maybe_tamanu(tamanu_ctx)
		.http_client(http_client)
		.maybe_canopy(canopy)
		.enable_heal(enable_heal)
		.build();

	let registry = checks::all();
	validate_selection(&registry, selected_names, "--check")?;
	validate_selection(&registry, skip_names, "--skip")?;

	// A check whose subject this sweep has no instance of is omitted outright:
	// it never runs, and never appears in any subject's report.
	// One entry runs once per subject that admits it, so a check on a machine
	// with two Postgres clusters produces one result per cluster.
	let selected: Vec<(usize, &checks::CheckEntry, Subject)> = registry
		.iter()
		.enumerate()
		.flat_map(|(idx, entry)| {
			subjects_for(entry.scope, &applications)
				.into_iter()
				.map(move |subject| (idx, entry, subject))
		})
		.filter(|(_, entry, subject)| {
			let qualified = subject.qualify(entry.name);
			(selected_names.is_empty() || selected_names.contains(&qualified))
				&& !skip_names.contains(&qualified)
		})
		.collect();

	// Announce the plan before running anything, so a live display can show
	// every check pending from the start. The sweep is the only thing that knows
	// which applications the host has, having just resolved them.
	if let Some(tx) = progress.as_ref() {
		let planned = selected
			.iter()
			.map(|(_, entry, subject)| subject.identify(entry.name))
			.collect();
		let _ = tx.send(DoctorEvent::Planned(planned));
	}

	// Run all selected checks concurrently. Results are collated by registry
	// index before returning, so callers see a stable order regardless of
	// completion order. A progress channel can observe results as they land.
	let prepared: Vec<PreparedCheck> = selected
		.iter()
		.map(|(idx, entry, subject)| {
			// A check's heal action is spawned in the background once its result
			// is known, when the sweep enables healing (the daemon) and the
			// check failed. `spawn_if_due` applies the per-check rate-limit and
			// the one-attempt-in-flight guard, so this can fire on every sweep.
			let heal = check_ctx.enable_heal.then_some(entry.heal).flatten();
			PreparedCheck {
				idx: *idx,
				name: entry.name,
				subject: subject.clone(),
				on_wire: entry.on_wire,
				fut: (entry.run)(check_ctx.clone()),
				heal: heal.map(|h| (h, check_ctx.clone())),
			}
		})
		.collect();
	let mut completed = run_checks_concurrently(prepared, progress.as_ref()).await;
	completed.sort_by_key(|(idx, _)| *idx);
	let results: Vec<CheckOutcome> = completed.into_iter().map(|(_, o)| o).collect();

	// Resolve via the file path first so a doctor sweep can still report to
	// canopy when the DB is down — that's exactly the moment canopy most
	// needs to hear from us.
	let machine_id = match get_or_create_machine_id().await {
		Ok(id) => Some(id),
		Err(err) => {
			warn!("could not resolve machine id: {err}");
			None
		}
	};

	let mut facts = collect_server_facts(
		tamanu.as_ref().map(|t| t.config.as_ref()),
		db.as_deref(),
		cached_pg_version,
		tamanu.as_ref().is_none_or(|t| t.is_tamanu),
	)
	.await;
	facts.tamanu_root = tamanu_root;
	facts.tamanu_server_kind = tamanu_server_kind;
	let pg_version = facts.pg_version.clone();
	// `binary_version` is the running binary's (bestool's) version, threaded in
	// by the caller. Evaluating `env!("CARGO_PKG_VERSION")` here would resolve
	// to this library's version instead, which is the wrong answer for the wire
	// payload. On hosts with no Tamanu (including generic-database-only hosts),
	// `tamanuVersion` is omitted from the payload entirely. A Tamanu
	// database-only host reports the version resolved from its DB.
	let tamanu_version = resolved_version.map(|v| v.to_string());
	let (machine_info, tamanu_info, postgres_info) =
		server_info::gather(binary_version, tamanu_version, facts).await;

	// Each application's detail comes from its own fact block, so no application
	// carries a fact belonging to another.
	let application_details: Vec<(ApplicationRef, Value)> = applications
		.iter()
		.map(|app| -> Result<_> {
			// Matched exhaustively: a new application kind must state which facts
			// are its own rather than silently inheriting another's.
			let info = match app.kind {
				ApplicationKind::Postgres => serde_json::to_value(&postgres_info),
				ApplicationKind::TamanuCentral | ApplicationKind::TamanuFacility => {
					serde_json::to_value(&tamanu_info)
				}
			};
			Ok((app.clone(), info.into_diagnostic()?))
		})
		.collect::<Result<_>>()?;

	let overall = OverallResult::from_statuses(results.iter().map(|o| &o.check.status));
	let payload = build_payload(&machine_info, &application_details, &results)?;

	Ok(SweepResult {
		machine_id,
		results,
		overall,
		payload,
		pg_version,
	})
}

async fn collect_server_facts(
	config: Option<&TamanuConfig>,
	db: Option<&tokio_postgres::Client>,
	cached_pg_version: Option<String>,
	is_tamanu: bool,
) -> ServerFacts {
	let mut facts = ServerFacts {
		canonical_url: config
			.and_then(|c| c.canonical_url())
			.map(|u| u.to_string()),
		timezone: config
			.and_then(|c| c.primary_time_zone())
			.map(|s| s.to_string()),
		pg_version: cached_pg_version,
		..Default::default()
	};

	let Some(client) = db else {
		return facts;
	};

	if facts.pg_version.is_none() {
		match client.query_one("SELECT version()", &[]).await {
			Ok(row) => match row.try_get::<_, String>(0) {
				Ok(v) => facts.pg_version = Some(v),
				Err(err) => warn!("decoding pg_version: {err}"),
			},
			Err(err) => warn!("SELECT version() failed: {err}"),
		}
	}

	// `local_system_facts` is a Tamanu table; a generic database has no
	// sync tick to read.
	if is_tamanu {
		match client
			.query_opt(
				"SELECT value FROM local_system_facts WHERE key = 'currentSyncTick'",
				&[],
			)
			.await
		{
			Ok(Some(row)) => match row.try_get::<_, String>(0) {
				Ok(tick) => facts.current_sync_tick = Some(tick),
				Err(err) => warn!("decoding currentSyncTick: {err}"),
			},
			Ok(None) => {}
			Err(err) => warn!("querying currentSyncTick: {err}"),
		}
	}

	facts
}

/// The sweep's overall result, across every subject the payload describes.
///
/// A failing application makes the sweep failing just as a failing machine
/// does: the operator is looking at one host either way.
pub fn overall_from_payload(payload: &StatusPayload) -> OverallResult {
	let per_target = payload
		.machine
		.iter()
		.map(|m| &m.health)
		.chain(
			payload
				.applications
				.iter()
				.flat_map(|a| a.values())
				.map(|a| &a.health),
		)
		.flatten()
		.flatten();

	// The ungrouped array is read too. A sweep of this vintage always sends it
	// empty, but one from a daemon that predates the split puts every check
	// there and describes no targets — and reading only the targets would call
	// that sweep healthy however much of it had failed.
	//
	// One walk, matching the typed result rather than formatting it: the payload
	// was previously traversed twice and a string allocated per check just to
	// compare against a literal.
	let mut failing = false;
	let mut degraded = false;
	for result in payload
		.health
		.iter()
		.chain(per_target)
		.filter_map(|c| c.result.as_ref())
	{
		match result {
			CheckResult::Failed => failing = true,
			CheckResult::Warning | CheckResult::Broken => degraded = true,
			CheckResult::Passed | CheckResult::Skipped => {}
		}
	}

	if failing {
		OverallResult::Failing
	} else if degraded {
		OverallResult::Degraded
	} else {
		OverallResult::Healthy
	}
}

/// Assemble the split status push: the machine's checks and detail, each
/// application's checks and detail, and the name of the agent reporting.
///
/// The top-level `health[]` is sent empty. It is the legacy flat form, and a
/// source that files its checks per target has none to put there; canopy reads
/// an empty array as "this source currently reports no ungrouped checks".
///
/// spec: SUBJ
fn build_payload(
	machine_info: &server_info::MachineInfo,
	applications: &[(ApplicationRef, Value)],
	results: &[CheckOutcome],
) -> Result<StatusPayload> {
	let machine = TargetReport::builder()
		.detail(detail_for(
			serde_json::to_value(machine_info).into_diagnostic()?,
			results,
			&Subject::Machine,
		))
		.health(health_for(results, None))
		.build();

	let reports: HashMap<String, ApplicationReport> = applications
		.iter()
		.map(|(app, info)| {
			let subject = Subject::Application(app.clone());
			let report = ApplicationReport::builder()
				.type_(app.kind.type_slug().to_owned())
				.detail(detail_for(info.clone(), results, &subject))
				.health(health_for(results, Some(&app.key)))
				.build();
			(app.key.clone(), report)
		})
		.collect();

	let mut payload = StatusPayload::builder()
		.health(Vec::new())
		.machine(machine)
		.source(REPORTING_SOURCE.to_owned())
		.build();
	payload.applications = (!reports.is_empty()).then_some(reports);
	Ok(payload)
}

/// One subject's `detail`: its own facts, plus the `payload_extras` lifted from
/// the checks filed against it.
///
/// An extra travels with its check's subject, so the machine's addresses land
/// on the machine and an application's service inventory on the application.
fn detail_for(info: Value, results: &[CheckOutcome], subject: &Subject) -> Map<String, Value> {
	let mut detail: Map<String, Value> = match info {
		Value::Object(obj) => obj,
		_ => Map::new(),
	};
	for outcome in results.iter().filter(|o| &o.subject == subject) {
		for (key, value) in &outcome.check.payload_extras {
			detail.insert(key.clone(), value.clone());
		}
	}
	detail
}

/// One target's `health[]`: its on-wire checks, in registry order.
///
/// `key` is the application's, or `None` for the machine — the same identity
/// canopy keys the target by, so the payload and a later refresh of it cannot
/// disagree about which checks belong where.
fn health_for(results: &[CheckOutcome], key: Option<&str>) -> Vec<HealthCheck> {
	results
		.iter()
		.filter(|o| o.on_wire && o.subject.key() == key)
		.map(|o| o.check.to_health_check())
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::subject::ApplicationKind;

	fn outcome(subject: Subject, check: Check) -> CheckOutcome {
		CheckOutcome {
			subject,
			check,
			on_wire: true,
		}
	}

	fn machine(check: Check) -> CheckOutcome {
		outcome(Subject::Machine, check)
	}

	fn central(check: Check) -> CheckOutcome {
		outcome(
			Subject::Application(ApplicationRef::tamanu(ApplicationKind::TamanuCentral)),
			check,
		)
	}

	fn postgres(check: Check) -> CheckOutcome {
		outcome(
			Subject::Application(ApplicationRef::local_postgres(5432)),
			check,
		)
	}

	const CENTRAL_KEY: &str = "host-tamanu-central";
	const POSTGRES_KEY: &str = "host-postgres-5432";

	fn details(apps: &[(ApplicationRef, Value)]) -> Vec<(ApplicationRef, Value)> {
		apps.to_vec()
	}

	fn central_and_postgres() -> Vec<(ApplicationRef, Value)> {
		details(&[
			(
				ApplicationRef::tamanu(ApplicationKind::TamanuCentral),
				serde_json::to_value(application_info()).unwrap(),
			),
			(
				ApplicationRef::local_postgres(5432),
				serde_json::to_value(postgres_info()).unwrap(),
			),
		])
	}

	fn machine_info() -> server_info::MachineInfo {
		server_info::MachineInfo {
			bestool_version: "0.0.0-test".into(),
			hostname: Some("box".into()),
			os_timezone: Some("Pacific/Auckland".into()),
			uptime_secs: 1,
			cpu_cores: 1,
			total_memory_bytes: 1,
			os_kind: "linux",
			os_name: None,
			os_version: None,
			kernel: None,
			arch: "x86_64".into(),
			virtualised: None,
			virtualisation: None,
			filesystems: Vec::new(),
			ipv4: true,
			ipv6: false,
			nat64: false,
			instance_tags: None,
		}
	}

	fn application_info() -> server_info::TamanuInfo {
		server_info::TamanuInfo {
			tamanu_version: Some("2.0.0".into()),
			tamanu_server_kind: Some("central"),
			..Default::default()
		}
	}

	fn postgres_info() -> server_info::PostgresInfo {
		server_info::PostgresInfo {
			pg_version: Some("16.1".into()),
		}
	}

	/// A canopy status response with just the fields these tests turn on.
	fn status_response(
		check_severities: HashMap<String, CheckSeverity>,
		applications: Option<HashMap<String, bestool_canopy::schema::TargetResponse>>,
	) -> StatusResponse {
		use bestool_canopy::schema::{Entitlements, TagMap};

		let mut response = StatusResponse::builder()
			.backup_now(Vec::new())
			.check_severities(check_severities)
			.names(
				Entitlements::builder()
					.applications(Vec::new())
					.certificates(Vec::new())
					.domains(Vec::new())
					.may_manage_dns(false)
					.may_manage_tls(false)
					.paused(false)
					.registered_names(Vec::new())
					.build(),
			)
			.tags(TagMap(Default::default()))
			.build();
		response.applications = applications;
		response
	}

	fn target_response(
		check_severities: HashMap<String, CheckSeverity>,
	) -> bestool_canopy::schema::TargetResponse {
		bestool_canopy::schema::TargetResponse::builder()
			.check_severities(check_severities)
			.tags(bestool_canopy::schema::TagMap(Default::default()))
			.build()
	}

	fn wire_names(health: &Option<Vec<HealthCheck>>) -> Vec<String> {
		health.iter().flatten().map(|c| c.check.clone()).collect()
	}

	fn result_of(health: &Option<Vec<HealthCheck>>, name: &str) -> Option<String> {
		health
			.iter()
			.flatten()
			.find(|c| c.check == name)
			.and_then(|c| c.result.as_ref())
			.map(|r| r.to_string())
	}

	#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
	async fn blocking_check_does_not_inflate_a_sibling_latency() {
		use std::time::{Duration, Instant};

		// One check blocks its executor thread the way a synchronous subprocess
		// spawn (pm2 jlist on Windows) does inside an async fn.
		const BLOCK_MS: u64 = 500;
		fn blocking_check(_ctx: SweepContext) -> BoxFuture<'static, Check> {
			Box::pin(async move {
				tokio::task::yield_now().await;
				std::thread::sleep(Duration::from_millis(BLOCK_MS));
				Check::pass("blocker", "blocked")
			})
		}

		// A sibling measures its own wall-clock latency across an await, exactly
		// as db_connect does around the connect call. With the checks driven on
		// one shared task, the blocker stalls this future while its `Instant` is
		// running and the reported latency balloons toward BLOCK_MS; on its own
		// task it stays near the real 20ms.
		fn timed_check(_ctx: SweepContext) -> BoxFuture<'static, Check> {
			Box::pin(async move {
				let start = Instant::now();
				tokio::time::sleep(Duration::from_millis(20)).await;
				let latency_ms = start.elapsed().as_millis() as u64;
				Check::pass("timed", "ok").with_detail("latency_ms", latency_ms)
			})
		}

		let ctx = SweepContext::builder()
			.http_client(reqwest::Client::new())
			.build();
		let prepared = vec![
			PreparedCheck {
				idx: 0,
				name: "blocker",
				subject: Subject::Machine,
				on_wire: true,
				fut: blocking_check(ctx.clone()),
				heal: None,
			},
			PreparedCheck {
				idx: 1,
				name: "timed",
				subject: Subject::Machine,
				on_wire: true,
				fut: timed_check(ctx.clone()),
				heal: None,
			},
		];

		let results = run_checks_concurrently(prepared, None).await;
		let (_, timed) = results
			.iter()
			.find(|(idx, _)| *idx == 1)
			.expect("timed check result");
		let latency = timed
			.check
			.details
			.get("latency_ms")
			.and_then(Value::as_u64)
			.expect("latency_ms detail");
		assert!(
			latency < BLOCK_MS / 2,
			"timed check reported {latency}ms latency — a blocking sibling inflated it (checks are not isolated)"
		);
	}

	#[tokio::test]
	async fn sweep_without_tamanu_omits_application_checks() {
		// No Tamanu means no application subject, so an application check is not
		// run and appears nowhere — neither on the wire nor in the results. A
		// machine check still runs.
		let sweep = perform_sweep(
			"0.0.0-test",
			None,
			reqwest::Client::new(),
			&["tamanu-central:tamanu_http".into(), "machine:memory".into()],
			&[],
			None,
			None,
			None,
			false,
		)
		.await
		.unwrap();

		assert!(
			!sweep.results.iter().any(|o| o.check.name == "tamanu_http"),
			"an application check must be absent, not skipped, when there is no application",
		);
		let memory = sweep
			.results
			.iter()
			.find(|o| o.check.name == "memory")
			.expect("machine check should run");
		assert_eq!(memory.subject, Subject::Machine);
		assert!(!memory.check.status.is_skip());

		// And nothing describes an application at all.
		assert!(sweep.payload.applications.is_none());
		let machine = sweep.payload.machine.as_ref().expect("machine target");
		assert!(!wire_names(&machine.health).contains(&"tamanu_http".to_string()));
	}

	#[tokio::test]
	async fn the_sweep_announces_its_plan_before_any_result() {
		// The display cannot know which applications the host has, so the sweep
		// says what it will run — and says it before the first result lands.
		let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
		perform_sweep(
			"0.0.0-test",
			None,
			reqwest::Client::new(),
			&["machine:memory".into(), "machine:load".into()],
			&[],
			None,
			Some(tx),
			None,
			false,
		)
		.await
		.unwrap();

		let first = rx.try_recv().expect("an event");
		let DoctorEvent::Planned(planned) = first else {
			panic!("the plan must arrive before any result");
		};
		assert_eq!(planned, vec!["machine:memory", "machine:load"]);
	}

	#[test]
	fn a_planned_name_matches_the_result_it_will_receive() {
		// The plan and the results must agree on identity or rows never fill.
		let outcome = postgres(Check::pass("connect", "ok"));
		assert_eq!(
			outcome.subject.identify(outcome.check.name),
			outcome.row_id()
		);
		assert_eq!(outcome.row_id(), "postgres-5432:connect");
		// And the selection name stays type-level, reaching every cluster.
		assert_eq!(outcome.qualified_name(), "postgres:connect");
	}

	#[test]
	fn subjects_for_omits_application_checks_without_an_application() {
		assert_eq!(
			subjects_for(CheckScope::Machine, &[]),
			vec![Subject::Machine]
		);
		assert!(subjects_for(CheckScope::Tamanu, &[]).is_empty());
		assert!(subjects_for(CheckScope::Postgres, &[]).is_empty());
	}

	#[test]
	fn a_check_runs_once_per_admitting_instance() {
		// Two clusters mean two results for one registry entry, each filed
		// against its own application.
		let clusters = [
			ApplicationRef::local_postgres(5432),
			ApplicationRef::local_postgres(5433),
		];
		let subjects = subjects_for(CheckScope::Postgres, &clusters);
		assert_eq!(subjects.len(), 2);
		let keys: Vec<&str> = subjects.iter().filter_map(|s| s.key()).collect();
		assert_eq!(keys, vec!["host-postgres-5432", "host-postgres-5433"]);
		// And both are selected by the one type-level name.
		assert!(
			subjects
				.iter()
				.all(|s| s.qualify("connect") == "postgres:connect")
		);
	}

	#[test]
	fn subjects_for_files_a_central_check_against_the_central_application() {
		let central = [ApplicationRef::tamanu(ApplicationKind::TamanuCentral)];
		assert_eq!(
			subjects_for(CheckScope::Central, &central),
			vec![Subject::Application(ApplicationRef::tamanu(
				ApplicationKind::TamanuCentral
			))],
		);
		// The same check has no subject on a facility, so it does not run there.
		let facility = [ApplicationRef::tamanu(ApplicationKind::TamanuFacility)];
		assert!(subjects_for(CheckScope::Central, &facility).is_empty());
	}

	#[test]
	fn postgres_checks_are_filed_against_postgres_not_tamanu() {
		// The four database checks grade the server, not whatever uses it.
		let both = [
			ApplicationRef::tamanu(ApplicationKind::TamanuCentral),
			ApplicationRef::local_postgres(5432),
		];
		let subject = subjects_for(CheckScope::Postgres, &both);
		assert_eq!(subject.len(), 1);
		assert_eq!(subject[0].key(), Some("host-postgres-5432"));

		// And a check reading Tamanu's own tables is not filed against Postgres.
		let tamanu = subjects_for(CheckScope::Tamanu, &both);
		assert_eq!(tamanu.len(), 1);
		assert_eq!(tamanu[0].key(), Some("host-tamanu-central"));
	}

	#[test]
	fn a_local_cluster_is_resolved_from_its_connection_string() {
		for url in [
			"postgresql://u@localhost/db",
			"postgresql://u@127.0.0.1/db",
			"postgresql://u@[::1]/db",
		] {
			assert_eq!(postgres_ref(url).key, "host-postgres-5432", "{url}");
		}
	}

	/// A socket host is only a socket on a platform that has them: elsewhere the
	/// connection-string parser reads the path as an ordinary TCP name, and no
	/// such cluster could be reached anyway.
	#[cfg(unix)]
	#[test]
	fn a_socket_connection_resolves_to_the_local_cluster() {
		assert_eq!(
			postgres_ref("postgresql:///db?host=/var/run/postgresql").key,
			"host-postgres-5432",
		);
	}

	#[test]
	fn a_cluster_on_another_port_is_a_different_application() {
		let a = postgres_ref("postgresql://u@localhost:5432/db");
		let b = postgres_ref("postgresql://u@localhost:5433/db");
		assert_ne!(a.key, b.key);
	}

	#[test]
	fn a_remote_cluster_is_not_claimed_as_this_machines() {
		let app = postgres_ref("postgresql://u@db.example.com:5432/tamanu");
		assert_eq!(app.key, "remote-db.example.com-5432");
		assert!(!app.key.starts_with("host-"));
	}

	#[cfg(unix)]
	#[test]
	fn a_unix_socket_is_always_local_and_carries_its_port() {
		// The socket is named `.s.PGSQL.<port>`, so a socket connection has a
		// port and reaches the same cluster as TCP on that port.
		assert!(database_is_local(
			"postgresql:///db?host=/var/run/postgresql"
		));
		let socket = postgres_ref("postgresql:///db?host=/var/run/postgresql&port=5433");
		let tcp = postgres_ref("postgresql://u@localhost:5433/db");
		assert_eq!(socket.key, tcp.key);
	}

	#[test]
	fn bare_check_name_is_rejected_with_its_qualified_forms() {
		let registry = checks::all();
		let err = validate_selection(&registry, &["disk_free".into()], "--check")
			.expect_err("a bare name must not be accepted");
		let msg = format!("{err}");
		assert!(msg.contains("machine:disk_free"), "{msg}");
	}

	#[test]
	fn bare_unknown_name_says_names_are_qualified() {
		let registry = checks::all();
		let err = validate_selection(&registry, &["no_such_check".into()], "--check")
			.expect_err("an unknown name must be rejected");
		let msg = format!("{err}");
		assert!(msg.contains("machine:disk_free"), "{msg}");
	}

	#[test]
	fn qualified_name_for_the_wrong_subject_is_rejected() {
		// disk_free is the machine's; there is no application form of it.
		let registry = checks::all();
		assert!(
			validate_selection(&registry, &["tamanu-central:disk_free".into()], "--check").is_err()
		);
		assert!(validate_selection(&registry, &["machine:disk_free".into()], "--check").is_ok());
	}

	#[test]
	fn every_name_is_unique_within_its_subject() {
		// A name identifies a check only with its subject, so the pair must be
		// unique even though the bare name need not be.
		let registry = checks::all();
		let mut seen = std::collections::HashSet::new();
		for qualified in known_qualified_names(&registry) {
			assert!(
				seen.insert(qualified.clone()),
				"duplicate check {qualified}"
			);
		}
	}

	#[test]
	fn payload_splits_checks_by_subject() {
		let results = vec![
			machine(Check::pass("disk_free", "ok")),
			central(Check::fail("migrations", "behind", "reason")),
		];
		let payload = build_payload(&machine_info(), &central_and_postgres(), &results).unwrap();

		let machine_target = payload.machine.as_ref().unwrap();
		assert_eq!(wire_names(&machine_target.health), vec!["disk_free"]);

		let apps = payload.applications.as_ref().unwrap();
		let app = apps.get(CENTRAL_KEY).expect("central application");
		assert_eq!(app.type_, "tamanu-central");
		assert_eq!(wire_names(&app.health), vec!["migrations"]);
	}

	#[test]
	fn postgres_reports_the_server_version_and_tamanu_does_not() {
		// The Postgres version is the server's own fact, not that of whatever
		// connects to it.
		let payload = build_payload(&machine_info(), &central_and_postgres(), &[]).unwrap();
		let apps = payload.applications.as_ref().unwrap();

		let pg_detail = &apps.get(POSTGRES_KEY).unwrap().detail;
		assert_eq!(pg_detail.get("pgVersion").unwrap(), "16.1");

		let tamanu_detail = &apps.get(CENTRAL_KEY).unwrap().detail;
		assert!(!tamanu_detail.contains_key("pgVersion"));
		assert_eq!(tamanu_detail.get("tamanuVersion").unwrap(), "2.0.0");
	}

	#[test]
	fn postgres_and_tamanu_are_separate_applications() {
		let results = vec![
			central(Check::fail("migrations", "behind", "reason")),
			postgres(Check::pass("checksums", "on")),
		];
		let payload = build_payload(&machine_info(), &central_and_postgres(), &results).unwrap();
		let apps = payload.applications.as_ref().unwrap();

		assert_eq!(apps.len(), 2);
		assert_eq!(apps.get(POSTGRES_KEY).unwrap().type_, "postgres");
		assert_eq!(apps.get(CENTRAL_KEY).unwrap().type_, "tamanu-central");
		assert_eq!(
			wire_names(&apps.get(POSTGRES_KEY).unwrap().health),
			vec!["checksums"]
		);
		assert_eq!(
			wire_names(&apps.get(CENTRAL_KEY).unwrap().health),
			vec!["migrations"]
		);
	}

	#[test]
	fn the_four_postgres_checks_are_scoped_to_postgres() {
		// They grade the server, so none of them is filed against Tamanu.
		let registry = checks::all();
		for name in ["connect", "version", "tuning", "checksums"] {
			let entry = registry
				.iter()
				.find(|e| e.name == name)
				.unwrap_or_else(|| panic!("{name} should be registered"));
			assert_eq!(entry.scope, CheckScope::Postgres, "{name}");
		}
	}

	#[test]
	fn an_application_reports_no_bestool_version() {
		// The bestool version answers whether the agent on the machine needs
		// upgrading, so it is the machine's fact and no application carries it.
		let payload = build_payload(&machine_info(), &central_and_postgres(), &[]).unwrap();

		let machine_detail = &payload.machine.as_ref().unwrap().detail;
		assert!(machine_detail.contains_key("bestoolVersion"));
		assert!(machine_detail.contains_key("hostname"));

		let apps = payload.applications.as_ref().unwrap();
		let app_detail = &apps.get(CENTRAL_KEY).unwrap().detail;
		assert!(!app_detail.contains_key("bestoolVersion"));
		assert!(!app_detail.contains_key("hostname"));
		assert_eq!(app_detail.get("tamanuVersion").unwrap(), "2.0.0");
	}

	#[test]
	fn the_two_timezones_land_on_their_own_subjects() {
		// Tamanu's configured zone is the application's; the clock zone is the
		// machine's. Neither reports the other's as its own.
		let info = server_info::TamanuInfo {
			timezone: Some("Pacific/Fiji".into()),
			..application_info()
		};
		let payload = build_payload(
			&machine_info(),
			&details(&[(
				ApplicationRef::tamanu(ApplicationKind::TamanuCentral),
				serde_json::to_value(info).unwrap(),
			)]),
			&[],
		)
		.unwrap();

		let machine_detail = &payload.machine.as_ref().unwrap().detail;
		assert_eq!(
			machine_detail.get("osTimezone").unwrap(),
			"Pacific/Auckland"
		);
		assert!(!machine_detail.contains_key("timezone"));

		let apps = payload.applications.as_ref().unwrap();
		let app_detail = &apps.get(CENTRAL_KEY).unwrap().detail;
		assert_eq!(app_detail.get("timezone").unwrap(), "Pacific/Fiji");
		assert!(!app_detail.contains_key("osTimezone"));
	}

	#[test]
	fn payload_extras_follow_their_check_subject() {
		// `payload_extras` is for data a check wants alongside its subject's
		// facts, not in its per-check entry. It must land on the subject the
		// check was filed against.
		let results = vec![
			machine(
				Check::pass("ips", "ok")
					.with_payload_extra("lanIps", serde_json::json!(["10.0.0.1"])),
			),
			central(
				Check::pass("tamanu_service", "ok")
					.with_payload_extra("services", serde_json::json!({"supervisor": "systemd"})),
			),
		];
		let payload = build_payload(&machine_info(), &central_and_postgres(), &results).unwrap();

		let machine_detail = &payload.machine.as_ref().unwrap().detail;
		assert!(machine_detail.contains_key("lanIps"));
		assert!(!machine_detail.contains_key("services"));

		let apps = payload.applications.as_ref().unwrap();
		let app_detail = &apps.get(CENTRAL_KEY).unwrap().detail;
		assert!(app_detail.contains_key("services"));
		assert!(!app_detail.contains_key("lanIps"));
	}

	#[test]
	fn payload_names_its_source_and_sends_no_flat_health() {
		let payload = build_payload(&machine_info(), &[], &[]).unwrap();
		assert_eq!(payload.source.as_deref(), Some("alertd"));
		assert!(payload.health.is_empty());
		assert!(payload.applications.is_none());
	}

	#[test]
	fn off_wire_checks_stay_out_of_their_subjects_health() {
		let results = vec![
			machine(Check::pass("on", "ok")),
			CheckOutcome {
				on_wire: false,
				..machine(Check::pass("off", "ok"))
			},
		];
		let payload = build_payload(&machine_info(), &[], &results).unwrap();
		let machine_target = payload.machine.as_ref().unwrap();
		assert_eq!(wire_names(&machine_target.health), vec!["on"]);
	}

	#[test]
	fn overall_reads_every_subject() {
		// A failing application makes the sweep failing: the operator is looking
		// at one host either way.
		let results = vec![
			machine(Check::pass("disk_free", "ok")),
			central(Check::fail("migrations", "behind", "reason")),
		];
		let payload = build_payload(&machine_info(), &central_and_postgres(), &results).unwrap();
		assert_eq!(overall_from_payload(&payload), OverallResult::Failing);
	}

	#[test]
	fn overall_healthy_when_every_subject_passes() {
		let results = vec![machine(Check::pass("disk_free", "ok"))];
		let payload = build_payload(&machine_info(), &[], &results).unwrap();
		assert_eq!(overall_from_payload(&payload), OverallResult::Healthy);
	}

	#[test]
	fn severities_are_applied_per_subject() {
		// The same bare name is graded separately on each subject, which is the
		// whole reason a check is identified by subject and name together.
		let results = vec![
			machine(Check::fail("shared", "bad", "reason")),
			central(Check::fail("shared", "bad", "reason")),
		];
		let payload = build_payload(&machine_info(), &central_and_postgres(), &results).unwrap();
		let mut sweep = SweepResult {
			machine_id: None,
			results,
			overall: OverallResult::Failing,
			payload,
			pg_version: None,
		};

		let mut severities = SplitSeverities::default();
		severities
			.machine
			.insert("shared".into(), CheckSeverity::Skip);
		severities.applications.insert(
			CENTRAL_KEY.into(),
			HashMap::from([("shared".to_string(), CheckSeverity::Fail)]),
		);
		sweep.apply_severities(&severities);

		let status_of = |subject: Subject| {
			sweep
				.results
				.iter()
				.find(|o| o.subject == subject)
				.map(|o| o.check.status.wire_result())
				.unwrap()
		};
		assert_eq!(status_of(Subject::Machine), "skipped");
		assert_eq!(
			status_of(Subject::Application(ApplicationRef::tamanu(
				ApplicationKind::TamanuCentral
			))),
			"failed"
		);

		// Only the application's failure survives, so the sweep still fails.
		assert_eq!(sweep.overall, OverallResult::Failing);

		// And each target's wire health tracks its own capped status.
		let machine_target = sweep.payload.machine.as_ref().unwrap();
		assert_eq!(
			result_of(&machine_target.health, "shared").unwrap(),
			"skipped"
		);
		let apps = sweep.payload.applications.as_ref().unwrap();
		assert_eq!(
			result_of(&apps.get(CENTRAL_KEY).unwrap().health, "shared").unwrap(),
			"failed"
		);
	}

	#[test]
	fn apply_severities_absent_check_defaults_to_warn() {
		// A check canopy hasn't heard of yet is capped at warn, so a computed
		// failure is shown as a warning rather than promoted or left fatal.
		let results = vec![machine(Check::fail("brand_new", "bad", "reason"))];
		let payload = build_payload(&machine_info(), &[], &results).unwrap();
		let mut sweep = SweepResult {
			machine_id: None,
			results,
			overall: OverallResult::Failing,
			payload,
			pg_version: None,
		};
		sweep.apply_severities(&SplitSeverities::default());
		assert_eq!(sweep.results[0].check.status.wire_result(), "warning");
		assert_eq!(sweep.overall, OverallResult::Degraded);
	}

	#[test]
	fn an_unsplit_response_still_governs_application_checks() {
		// A canopy that answers only the ungrouped map must not leave application
		// checks ungoverned: they would all fall to the absent-check ceiling and a
		// real failure would render as a warning.
		let split = split_severities(&status_response(
			HashMap::from([("connect".to_string(), CheckSeverity::Fail)]),
			None,
		));

		let postgres = Subject::Application(ApplicationRef::local_postgres(5432));
		let map = split
			.for_subject(&postgres)
			.expect("an unsplit response must still govern applications");
		assert_eq!(map.get("connect"), Some(&CheckSeverity::Fail));
	}

	#[test]
	fn a_split_response_leaves_an_unknown_application_to_the_default() {
		// When canopy does answer per target, a key it omits is an application it
		// does not yet hold, so its checks are genuinely unknown and the
		// absent-check default is the right answer rather than the flat map.
		let split = split_severities(&status_response(
			HashMap::from([("connect".to_string(), CheckSeverity::Fail)]),
			Some(HashMap::from([(
				CENTRAL_KEY.to_string(),
				target_response(HashMap::new()),
			)])),
		));

		let unknown = Subject::Application(ApplicationRef::local_postgres(5432));
		assert!(split.for_subject(&unknown).is_none());
	}

	#[test]
	fn an_unparseable_url_still_reports_a_cluster_to_alert_on() {
		// Reporting nothing would drop the database's reachability check, and
		// being unable to alert on the database is the one thing alertd must never
		// do.
		let app = postgres_ref("this is not a connection string");
		assert_eq!(app.kind, ApplicationKind::Postgres);
		assert_eq!(app.key, "host-postgres-5432");
	}

	#[test]
	fn a_mixed_host_list_names_the_host_that_is_actually_remote() {
		// `database_is_local` calls a list local only when all of it is, so the
		// remote name must be the one that is not local — not simply the first.
		let app = postgres_ref("postgresql://u@localhost,db.example.com/tamanu");
		assert_eq!(app.key, "remote-db.example.com-5432");
	}

	#[test]
	fn every_check_reaches_the_wire_even_with_odd_details() {
		// A check used to vanish from the push if its details would not
		// deserialise, silently ceasing to be monitored.
		let odd = Check::fail("connect", "down", "refused")
			.with_detail("check", serde_json::json!({"not": "a string"}))
			.with_detail("result", serde_json::json!(42));
		let results = vec![postgres(odd)];
		let payload = build_payload(&machine_info(), &central_and_postgres(), &results).unwrap();

		let apps = payload.applications.as_ref().unwrap();
		let health = &apps.get(POSTGRES_KEY).unwrap().health;
		assert_eq!(wire_names(health), vec!["connect"]);
		assert_eq!(result_of(health, "connect").unwrap(), "failed");
	}

	#[test]
	fn a_checks_details_and_reason_reach_the_wire() {
		let check =
			Check::warning("connect", "slow", "latency high").with_detail("latency_ms", 900);
		let results = vec![postgres(check)];
		let payload = build_payload(&machine_info(), &central_and_postgres(), &results).unwrap();
		let apps = payload.applications.as_ref().unwrap();
		let entry = apps.get(POSTGRES_KEY).unwrap().health.as_ref().unwrap()[0].clone();
		assert_eq!(entry.extra.get("latency_ms").unwrap(), 900);
		assert_eq!(entry.extra.get("summary").unwrap(), "slow");
		assert_eq!(entry.extra.get("reason").unwrap(), "latency high");
	}

	#[test]
	fn flat_severities_govern_every_subject() {
		// The ungrouped endpoint answers by bare name alone, so its map applies
		// wherever that name is filed.
		let flat =
			SplitSeverities::flat(HashMap::from([("shared".to_string(), CheckSeverity::Skip)]));
		for subject in [
			Subject::Machine,
			Subject::Application(ApplicationRef::tamanu(ApplicationKind::TamanuCentral)),
			Subject::Application(ApplicationRef::local_postgres(5432)),
		] {
			let map = flat.for_subject(&subject).expect("a map for every subject");
			assert_eq!(map.get("shared"), Some(&CheckSeverity::Skip));
		}
	}

	#[test]
	fn resolve_prefers_tamanu_url_over_generic() {
		let resolved = resolve_sweep_tamanu_from(
			None,
			Some("postgresql://u@localhost/tamanu".into()),
			Some("postgresql://u@localhost/other".into()),
		)
		.unwrap()
		.unwrap();
		assert_eq!(resolved.database_url, "postgresql://u@localhost/tamanu");
		assert!(resolved.is_tamanu);
		assert!(!resolved.has_install);
	}

	#[test]
	fn resolve_falls_back_to_generic_database_url() {
		let resolved =
			resolve_sweep_tamanu_from(None, None, Some("postgresql://u@localhost/other".into()))
				.unwrap()
				.unwrap();
		assert_eq!(resolved.database_url, "postgresql://u@localhost/other");
		assert!(!resolved.is_tamanu);
		assert!(!resolved.has_install);
	}

	#[test]
	fn resolve_without_any_url_is_none() {
		assert!(
			resolve_sweep_tamanu_from(None, None, None)
				.unwrap()
				.is_none()
		);
	}

	#[test]
	fn payload_skip_result_on_wire() {
		// The whole point of distinguishing Skip from Fail/Warning is that
		// "we don't know" shouldn't fire alerts downstream of the wire format.
		let results = vec![
			machine(Check::pass("a", "ok")),
			machine(Check::skip("b", "not run", "reason")),
		];
		let payload = build_payload(&machine_info(), &[], &results).unwrap();
		let machine_target = payload.machine.as_ref().unwrap();
		assert_eq!(result_of(&machine_target.health, "b").unwrap(), "skipped");
	}
}
