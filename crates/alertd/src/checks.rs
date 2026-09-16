//! Doctor healthchecks. One module per check.
//!
//! Each module exposes a `pub async fn run` taking the context of the subject
//! it reports for: [`MachineCx`], [`PgCx`], or [`TamanuCx`]. The [`all`]
//! registry below ties names to runners so the dispatcher can filter by
//! `--check`.
//!
//! spec: SUBJ

use std::{path::PathBuf, sync::Arc};

use bestool_postgres::pool::{PgConnection, PgPool};
use futures::future::BoxFuture;
use node_semver::Version;

use bestool_canopy::CanopyClient;
use bestool_tamanu::{
	ApiServerKind,
	config::{Database, TamanuConfig},
};

use super::check::Check;
use super::heal::{self, HealAction};
use super::subject::{ApplicationKind, ApplicationRef, TamanuScope};

pub mod util;

pub mod billing_tags;
pub mod btrfs;
pub mod caddy_certs;
pub mod caddy_resolvers;
pub mod caddy_version;
pub mod caddyfile_version;
pub mod canopy_registration;
pub mod certificate_notification_errors;
pub mod db_connect;
pub mod db_version;
pub mod disk_free;
pub mod external_users;
pub mod fhir_config;
pub mod fhir_job_errors;
pub mod fhir_jobs;
pub mod fhir_materialisation;
pub mod fhir_service_requests_unresolved;
pub mod fhir_workers;
pub mod held_captures;
pub mod http_errors;
pub mod inodes;
pub mod ips;
pub mod ips_errors;
pub mod load;
pub mod memory;
pub mod migrations;
pub mod munin;
pub mod patient_communication_errors;
pub mod pg_checksums;
pub mod pg_tuning;
pub mod report_errors;
pub mod reporting_roles;
pub mod sync_facility_stale;
pub mod sync_lookup;
pub mod sync_restart_loop;
pub mod sync_session_errors;
pub mod sync_sessions;
pub mod sync_snapshot_tables;
pub mod tailscale;
pub mod tailscale_config;
pub mod tamanu_http;
pub mod tamanu_service;
pub mod time_sync;
pub mod uptime;
pub mod version_drift;

/// What a machine check is handed.
///
/// The machine is its subject, so it gets nothing scoped to an application: no
/// application key, no database, no application configuration. A machine check
/// therefore cannot read an application as though it were its subject, and the
/// compiler is what says so.
///
/// Built through its [`builder`](MachineCx::builder) so new fields don't churn
/// every construction site; the optional fields default to absent.
///
/// spec: SUBJ
#[derive(Clone, bon::Builder)]
pub struct MachineCx {
	/// Shared across checks and across the daemon's other consumers so TCP/TLS
	/// connections stay warm between ticks; HTTP checks apply per-request
	/// timeouts via `RequestBuilder::timeout`.
	pub http: reqwest::Client,
	/// Shared canopy client for checks that reach canopy during a sweep — a
	/// self-heal action that recovers state from canopy, in particular. `None`
	/// on a one-shot local sweep with no canopy connectivity.
	pub canopy: Option<Arc<CanopyClient>>,
	/// The Tamanu installed on this machine, where there is one.
	pub tamanu: Option<MachineTamanu>,
}

/// What a machine check may know about the Tamanu installed on its subject.
///
/// Two machine checks grade machine artefacts that Tamanu's installer put
/// there: `disk_free` considers the mount holding the install root alongside
/// `/`, and `caddyfile_version` grades the machine's Caddyfile marker against
/// the deployment's version. Which Tamanu is on the box is a fact about the
/// machine, not a reading taken from the application, so it is carried here
/// rather than either check changing subject.
///
/// Deliberately thin: a version and an install root. No configuration, no
/// database, no application key — a machine check has no business with those.
///
/// spec: SUBJ
#[derive(Clone, Debug)]
pub struct MachineTamanu {
	/// The deployment's version.
	pub version: Version,
	/// Where its files sit on this machine, when it has any. `None` for a
	/// deployment known only through its database.
	pub root: Option<PathBuf>,
}

/// What a Tamanu check is handed: the deployment it reports for, and the
/// parameters resolved for that one deployment.
///
/// One of these is built per deployment, so a check filed against two
/// deployments runs twice against two different contexts and reports each
/// subject's own readings rather than one subject's twice.
///
/// Each check picks the fields it needs and ignores the rest. The pool is
/// `Option` because not every check needs the database, and `connect` itself
/// runs before a connection is available.
///
/// spec: SUBJ
#[derive(Clone)]
pub struct TamanuCx {
	/// The deployment this check reports for: its role, and which one of that
	/// role.
	pub app: ApplicationRef,
	/// The deployment's version, or `0.0.0` where the sweep could not resolve
	/// one — which `version_drift` reads as nothing to compare against.
	pub version: Version,
	/// The deployment's configuration, synthesised from the database URL alone
	/// where there are no install files to have read it from, which
	/// [`installed_config`](TamanuCx::installed_config) is how a check tells
	/// apart.
	pub config: Arc<TamanuConfig>,
	/// Where the deployment's files sit on this machine, when it has any.
	/// `None` for one known only through its database.
	pub install_root: Option<PathBuf>,
	pub database_url: String,
	/// The database pool this deployment's checks draw from, when the database
	/// could be reached at all. Take a connection with [`TamanuCx::db`].
	pub pool: Option<PgPool>,
	/// Shared across checks and across the daemon's other consumers so TCP/TLS
	/// connections stay warm between ticks; HTTP checks apply per-request
	/// timeouts via `RequestBuilder::timeout`.
	pub http: reqwest::Client,
}

/// What a Postgres check is handed: the cluster it reports for, and how to
/// reach it.
///
/// A cluster is an application in its own right rather than a part of whatever
/// uses it, so this carries none of a Tamanu's parameters. There is no version
/// field, because a cluster's server version is a fact read from the server
/// rather than a version of the application as installed; no install root,
/// because a cluster has no install of its own for a check to read; and no
/// deployment configuration, because a cluster is described by the URL it is
/// keyed and connected by.
///
/// Earlier these were a Tamanu's fields left empty or filled with markers on a
/// cluster's context. Splitting the context is what stops a Postgres check
/// reading a value that was never about its subject.
///
/// spec: SUBJ
#[derive(Clone)]
pub struct PgCx {
	/// The cluster this check reports for, keyed by the port it answers on.
	pub app: ApplicationRef,
	/// The cluster's connection details, parsed from the URL it is keyed and
	/// connected by, so what a check reports about it and what the pool opened
	/// cannot disagree.
	pub database: Database,
	pub database_url: String,
	/// The pool this cluster's checks draw from, when it could be reached at
	/// all. Take a connection with [`PgCx::db`].
	pub pool: Option<PgPool>,
}

/// How the sweep's pool is sized.
///
/// Three things have to hold at once, and earlier versions of this traded one
/// for another by moving a single number up and down.
///
/// **A small budget.** The pool points at the deployment's own database, which
/// the application is also connecting to. Sizing it to the check fan-out would
/// burst twenty-odd backends every minute — and `bestool tamanu doctor` opens a
/// second pool that can overlap the daemon's — which against a cluster on the
/// usual hundred connections, shared with Tamanu's own pools, risks the
/// healthcheck causing the outage it exists to report. Few connections held at
/// rest, for the same reason.
///
/// **Queueing must not look like an outage.** There are more checks than slots,
/// so checks wait; several report a missing connection as a failure. A deadline
/// short enough for ordinary waiting to reach would turn a busy sweep into a
/// database-down alert on a database that is up.
///
/// **The sweep must finish.** Waiting forever is not the answer either: one
/// check stuck on a lock would hold its slot, every queued check would wait
/// behind it, and the sweep would never return — so nothing reaches canopy and
/// the watchdog eventually restarts the daemon into the same hang. Silence is
/// the worst outcome for something whose job is to report trouble.
///
/// So the deadline is far longer than any healthy sweep and still finite. With
/// these slots and the checks' short queries, ordinary queueing finishes in
/// well under a second; reaching two minutes means the database has stopped
/// answering, which is worth reporting as such.
///
/// **Waiting for a slot is not waiting for a host.** That deadline covers
/// opening a connection as well as queueing for one, and a host that drops
/// packets rather than refusing would otherwise absorb all of it on a single
/// connect — every tick, delaying the outage report the daemon exists to make.
/// Connecting gets its own, much shorter, bound: a healthy connect is
/// milliseconds, so seconds are already generous, and an unreachable database
/// fails fast while a busy one still gets the patience above.
pub const POOL_SIZE: bestool_postgres::pool::PoolSize = bestool_postgres::pool::PoolSize {
	max_open: 8,
	max_idle: 2,
	max_idle_lifetime: Some(std::time::Duration::from_secs(300)),
	get_timeout: Some(std::time::Duration::from_secs(120)),
	connect_timeout: Some(std::time::Duration::from_secs(10)),
};

/// Take a connection from a pool, or `None` when there is none or it can't be
/// reached — in which case the check skips.
///
/// Each check gets its own connection so their queries run in parallel rather
/// than pipelining onto one backend, and gives it back when the connection
/// drops at the end of the check. The pool bounds how many run at once; a check
/// that has to wait for one simply starts later.
///
/// Logged at debug: a database the sweep can't reach is reported by the
/// Postgres `connect` check, which opens its own connection, so a warning per
/// check here would be noise on top of the real signal.
async fn take_connection(pool: Option<&PgPool>) -> Option<PgConnection> {
	match pool?.get().await {
		Ok(conn) => Some(conn),
		Err(err) => {
			tracing::debug!(%err, "could not take a DB connection for this check");
			None
		}
	}
}

impl TamanuCx {
	/// Whether this deployment is a central or a facility.
	///
	/// Derived from the subject rather than stored, so it cannot describe a
	/// different deployment from the one the context was built for. Which role
	/// a Tamanu plays is decided once at sweep startup from the most
	/// authoritative available signals (DB `local_system_facts` first, then
	/// config) and reaches here through the application it identified.
	pub fn server_kind(&self) -> ApiServerKind {
		match self.app.kind {
			ApplicationKind::TamanuFacility => ApiServerKind::Facility,
			// A Tamanu context is only ever built for a Tamanu subject, and the
			// registry's arms are what say so.
			_ => ApiServerKind::Central,
		}
	}

	/// The deployment's configuration as read from its install files, or `None`
	/// when there are none to have read it from.
	///
	/// A check that grades what the configuration says needs to know it came
	/// from a real config rather than being synthesised from a database URL.
	pub fn installed_config(&self) -> Option<&TamanuConfig> {
		self.install_root.is_some().then(|| self.config.as_ref())
	}

	/// Take a connection for this check. See [`take_connection`].
	pub async fn db(&self) -> Option<PgConnection> {
		take_connection(self.pool.as_ref()).await
	}
}

impl PgCx {
	/// Take a connection for this check. See [`take_connection`].
	pub async fn db(&self) -> Option<PgConnection> {
		take_connection(self.pool.as_ref()).await
	}
}

/// Whether the doctor is running as root (euid 0).
///
/// Asks the kernel via `geteuid()` rather than parsing `/proc/self/status`: the
/// daemon runs under a sandbox that can refuse `/proc`, and a failed read there
/// would wrongly read as "not root" — making the root daemon try to `sudo`.
#[cfg(unix)]
fn is_root() -> bool {
	rustix::process::geteuid().is_root()
}

#[cfg(not(unix))]
fn is_root() -> bool {
	false
}

/// Build a command that runs `program` as root: directly when we already are,
/// else via `sudo -n`. Some checks read root-only interfaces — btrfs ioctls
/// (`device stats`), and the like — that see nothing as a normal user. The
/// alertd daemon runs the sweep as root (direct); an interactive `bestool tamanu
/// doctor` elevates instead of reporting blind. `-n` means it never waits on a
/// password prompt — with passwordless sudo it works, otherwise it fails fast
/// (and the check degrades) rather than blocking on a tty the daemon hasn't got.
pub(crate) fn privileged(program: &str) -> tokio::process::Command {
	if is_root() {
		tokio::process::Command::new(program)
	} else {
		let mut cmd = tokio::process::Command::new("sudo");
		cmd.arg("-n").arg(program);
		cmd
	}
}

/// `tokio_postgres::Error`'s top-level Display is the unhelpful `"db error"`.
/// The actual SQL message lives in the optional `DbError` underneath; this
/// helper surfaces it where present and falls back to the source chain.
pub fn fmt_db_error(err: &tokio_postgres::Error) -> String {
	if let Some(db) = err.as_db_error() {
		let mut s = format!("{}: {}", db.severity(), db.message());
		if let Some(detail) = db.detail() {
			s.push_str(" — ");
			s.push_str(detail);
		}
		return s;
	}

	fmt_chain(err)
}

/// Build the Check for a query that errored, classified by SQLSTATE.
///
/// Class 42 ("syntax error or access rule violation": dropped or renamed
/// columns, json/jsonb drift, missing functions) means the check's own SQL no
/// longer matches the schema — a fault in the healthcheck, not the deployment
/// — so it reports as BROKEN rather than flagging the server as failing.
/// Everything else stays FAIL.
pub fn query_error_check(name: &'static str, err: &tokio_postgres::Error) -> Check {
	let reason = fmt_db_error(err);
	if err
		.as_db_error()
		.is_some_and(|db| db.code().code().starts_with("42"))
	{
		Check::broken(name, "healthcheck query broken", reason)
	} else {
		Check::fail(name, "query failed", reason)
	}
}

/// Walk a `std::error::Error`'s source chain and join all the messages.
///
/// `reqwest::Error`'s Display is just "error sending request for url (...)";
/// the actual cause (DNS error, connection refused, timed out, proxy failure)
/// is one or two `.source()` calls down. Without walking the chain, doctor
/// `FAIL` rows lose the only diagnostic that matters.
pub fn fmt_chain<E: std::error::Error + ?Sized>(err: &E) -> String {
	use std::error::Error;

	let mut parts = vec![err.to_string()];
	let mut src: Option<&dyn Error> = err.source();
	while let Some(s) = src {
		parts.push(s.to_string());
		src = s.source();
	}
	parts.join(": ")
}

/// What to run for one check, and the self-heal action that goes with it.
///
/// The heal travels with the runner rather than beside it because a heal needs
/// the context its check ran with, and that is the type parameter here.
pub struct Runner<Cx> {
	pub run: fn(Cx) -> BoxFuture<'static, Check>,
	/// Optional self-heal action, run in the background by the daemon while the
	/// check is failing. See [`crate::heal`].
	pub heal: Option<HealAction<Cx>>,
}

/// Which subject a check reports for, and what to run for it.
///
/// One arm per kind of subject, each carrying the context that subject can
/// describe. A runner paired with a subject its context does not fit is not
/// representable, so a Postgres check cannot be handed a deployment's version
/// or install root and a Tamanu check cannot be filed against a cluster.
///
/// The scope narrows only where a subject has roles to narrow to: a Tamanu is a
/// central or a facility, while a machine and a cluster each need no further
/// saying. Every arm carries a heal because any of them may have one.
///
/// spec: SUBJ
pub enum Run {
	Machine(Runner<MachineCx>),
	Postgres(Runner<PgCx>),
	Tamanu(TamanuScope, Runner<TamanuCx>),
}

/// One check's name + runner.
pub struct CheckEntry {
	/// Stable identifier, unique within the check's subject rather than across
	/// the whole registry: a machine `foo` and an application `foo` would be two
	/// different checks.
	pub name: &'static str,
	/// `false` means the check is rendered to the CLI but NOT included in the
	/// canopy `health[]` wire array (e.g. `tailscale`, which canopy already
	/// tracks elsewhere).
	pub on_wire: bool,
	/// What to run, and for which subject. A subject this does not admit never
	/// runs the check and never carries its result.
	pub run: Run,
}

impl CheckEntry {
	/// Every `subject:name` slug this check could be selected by.
	pub fn possible_slugs(&self) -> Vec<&'static str> {
		match &self.run {
			Run::Machine(_) => vec!["machine"],
			Run::Postgres(_) => vec![ApplicationKind::Postgres.type_slug()],
			Run::Tamanu(scope, _) => scope.possible_slugs(),
		}
	}
}

/// Register a check: its name, its module, and the subject it reports for.
///
/// The subject is the only axis. What a check needs to run is resolved into the
/// context it is handed, so there is nothing left for a second axis to gate: a
/// check filed against a subject this host does not have is absent rather than
/// skipped, and one filed against a subject it does have is handed that
/// subject's own parameters.
macro_rules! entry {
	($name:literal, $module:ident, $subject:ident $(, $opt:tt)*) => {
		CheckEntry {
			name: $name,
			on_wire: entry!(@on_wire $($opt),*),
			run: entry!(@run $module, $subject $(, $opt)*),
		}
	};

	// Rendered to the CLI but kept OFF the canopy `health[]` wire array — for
	// checks reporting a value already carried as a status fact, so they're
	// useful locally but shouldn't alert.
	(@on_wire off_wire $(, $rest:tt)*) => { false };
	(@on_wire $($rest:tt)*) => { true };

	// The heal expands inside the arm its check sits in, so a heal is only ever
	// written against the context its check runs with: a machine heal on an
	// application check does not compile.
	(@heal) => { None };
	(@heal off_wire) => { None };
	(@heal off_wire, $heal:expr, $interval:expr) => { entry!(@heal $heal, $interval) };
	(@heal $heal:expr, $interval:expr) => {
		Some(HealAction { run: $heal, min_interval: $interval })
	};

	(@run $module:ident, machine $(, $opt:tt)*) => {
		Run::Machine(Runner {
			run: |ctx| Box::pin($module::run(ctx)),
			heal: entry!(@heal $($opt),*),
		})
	};
	(@run $module:ident, postgres $(, $opt:tt)*) => {
		Run::Postgres(Runner {
			run: |ctx| Box::pin($module::run(ctx)),
			heal: entry!(@heal $($opt),*),
		})
	};
	(@run $module:ident, tamanu_app $(, $opt:tt)*) => {
		entry!(@tamanu $module, TamanuScope::Any $(, $opt)*)
	};
	(@run $module:ident, central $(, $opt:tt)*) => {
		entry!(@tamanu $module, TamanuScope::Central $(, $opt)*)
	};
	(@run $module:ident, facility $(, $opt:tt)*) => {
		entry!(@tamanu $module, TamanuScope::Facility $(, $opt)*)
	};

	(@tamanu $module:ident, $scope:expr $(, $opt:tt)*) => {
		Run::Tamanu(
			$scope,
			Runner {
				run: |ctx| Box::pin($module::run(ctx)),
				heal: entry!(@heal $($opt),*),
			},
		)
	};
}

/// Registry of every check the doctor knows how to run.
///
/// Order here is the order they appear in the CLI render.
pub fn all() -> Vec<CheckEntry> {
	vec![
		entry!("connect", db_connect, postgres),
		// Reports the postgres version, which is already the application's
		// `pgVersion` fact — useful in the CLI render, but off the wire.
		entry!("version", db_version, postgres, off_wire),
		entry!("migrations", migrations, tamanu_app),
		entry!("reporting_roles", reporting_roles, tamanu_app),
		// An application check that still reads the machine's total memory for its
		// denominator. Interim, and not an oversight: the substrate work replaces
		// that reading with the Postgres service's own declared ceiling.
		entry!("tuning", pg_tuning, postgres),
		entry!("checksums", pg_checksums, postgres),
		entry!("disk_free", disk_free, machine),
		entry!("inodes", inodes, machine),
		entry!("btrfs", btrfs, machine),
		// Filesystem-level snapshots, and what they capture is not confined to any
		// one application's database, so they are the machine's concern.
		entry!("held_captures", held_captures, machine),
		entry!("memory", memory, machine),
		entry!("load", load, machine),
		// Uptime is already a machine fact; the soft "recently rebooted" warning is
		// CLI-only, so keep it off the wire.
		entry!("uptime", uptime, machine, off_wire),
		entry!("time_sync", time_sync, machine),
		// Needs a reachable Tamanu DB / deployment but not the config files, so it
		// runs against a `TAMANU_DATABASE_URL`-only host too.
		entry!("tamanu_http", tamanu_http, tamanu_app),
		// The front-end software itself is the machine's: these grade what is
		// installed on the box, not what it serves. They skip gracefully when caddy
		// isn't present.
		entry!("caddy_version", caddy_version, machine),
		entry!("caddy_resolvers", caddy_resolvers, machine),
		// The certificates, by contrast, are the application's: they are issued for
		// the names it answers on.
		entry!("caddy_certs", caddy_certs, tamanu_app),
		// Grades the machine's Caddyfile version marker, so it reports for the
		// machine — reading the deployment's version off the machine's context to
		// tell whether the marker is stale. Windows-only, and self-skips when
		// there is no Tamanu on the host or caddy isn't present.
		entry!("caddyfile_version", caddyfile_version, machine),
		// The error rates are the application's traffic, however the front end in
		// front of it happens to be reached.
		entry!("http_errors", http_errors, tamanu_app),
		entry!("tailscale", tailscale, machine, off_wire),
		entry!("tailscale_config", tailscale_config, machine),
		// bestool's own Canopy enrolment is the machine's: it reports so Canopy sees
		// an incomplete registration before it blocks backups.
		entry!(
			"canopy_registration",
			canopy_registration,
			machine,
			(|ctx| Box::pin(canopy_registration::heal(ctx))),
			(heal::DEFAULT_MIN_INTERVAL)
		),
		// Reports the machine's LAN and best-guess WAN addresses as facts (off the
		// wire; carried in the machine's detail, like the timezone).
		entry!("ips", ips, machine, off_wire),
		// Reports whether munin-node is installed as a machine fact (off the wire,
		// like `ips`); not a health signal.
		entry!("munin", munin, machine, off_wire),
		// Read against the machine, so a machine hosting several applications
		// carries one set of tags rather than one per application.
		entry!("billing_tags", billing_tags, machine),
		// The config-derived FHIR expectation degrades to Unknown without config
		// (see `services::expected`); the rest is DB/host-derived.
		entry!("tamanu_service", tamanu_service, tamanu_app),
		// Compares running container tags against the deployment's version, which is
		// the install's env-file version when present and the DB's recorded
		// `currentVersion` otherwise. It self-skips if neither is available.
		entry!("version_drift", version_drift, tamanu_app),
		entry!("external_users", external_users, machine),
		entry!("sync_sessions", sync_sessions, tamanu_app),
		// Config-derived: the FHIR API and worker toggles must agree.
		entry!("fhir_config", fhir_config, tamanu_app),
		// Restart the FHIR workers when the backlog check fails, capped at one
		// attempt an hour so a queue that drains slowly isn't repeatedly kicked.
		// The heal is central-only even though the check itself is not.
		entry!(
			"fhir_jobs",
			fhir_jobs,
			tamanu_app,
			(|ctx| Box::pin(fhir_jobs::heal(ctx))),
			(std::time::Duration::from_secs(60 * 60))
		),
		entry!("fhir_workers", fhir_workers, central),
		entry!(
			"certificate_notification_errors",
			certificate_notification_errors,
			central
		),
		entry!("ips_errors", ips_errors, central),
		entry!(
			"patient_communication_errors",
			patient_communication_errors,
			central
		),
		entry!("report_errors", report_errors, central),
		entry!("fhir_job_errors", fhir_job_errors, central),
		entry!("sync_session_errors", sync_session_errors, central),
		entry!("sync_facility_stale", sync_facility_stale, central),
		entry!("sync_snapshot_tables", sync_snapshot_tables, central),
		entry!("sync_lookup", sync_lookup, central),
		entry!("sync_restart_loop", sync_restart_loop, central),
		entry!(
			"fhir_service_requests_unresolved",
			fhir_service_requests_unresolved,
			central
		),
		// Measures the outcome of materialisation rather than its queue: upstream
		// records that never became FHIR resources, which every other fhir_* check
		// reads as green.
		entry!("fhir_materialisation", fhir_materialisation, central),
	]
}

#[cfg(test)]
pub mod test_support {
	//! Helpers for DB-backed check tests.
	//!
	//! Each check is central-only and DB-backed, so its tests need an
	//! [`TamanuCx`] wired to one of the local `tamanu-central` /
	//! `tamanu-facility` databases. These connect lazily and return `None` when
	//! the DB is unavailable so the suite degrades gracefully off-CI.

	use std::sync::Arc;

	use bestool_postgres::pool::PgPool;
	use node_semver::Version;

	use bestool_tamanu::config::{Database, TamanuConfig};

	use super::{PgCx, TamanuCx};
	use crate::subject::{ApplicationKind, ApplicationRef};

	fn central_config() -> TamanuConfig {
		serde_json::from_value(serde_json::json!({
			"db": { "name": "tamanu-central", "username": "u", "password": "p" },
		}))
		.expect("central test config should parse")
	}

	fn facility_config() -> TamanuConfig {
		serde_json::from_value(serde_json::json!({
			"db": { "name": "tamanu-facility", "username": "u", "password": "p" },
			"serverFacilityIds": ["facility-1"],
		}))
		.expect("facility test config should parse")
	}

	/// A pool for one of the local test databases, or `None` when it can't be
	/// reached — building a pool checks that it can connect.
	async fn connect(db_name: &str) -> Option<PgPool> {
		let url = format!("postgresql://localhost/{db_name}");
		bestool_postgres::pool::create_pool_sized(
			&url,
			"bestool-alertd-test",
			super::POOL_SIZE,
			bestool_postgres::pool::Prompt::Never,
		)
		.await
		.ok()
	}

	/// A central [`TamanuCx`] backed by `tamanu-central`, or `None` if that DB
	/// can't be reached.
	pub async fn central_ctx() -> Option<TamanuCx> {
		let pool = connect("tamanu-central").await?;
		Some(TamanuCx {
			app: ApplicationRef::tamanu(ApplicationKind::TamanuCentral),
			version: Version::parse("0.0.0").unwrap(),
			config: Arc::new(central_config()),
			install_root: Some(std::path::PathBuf::from("/nonexistent")),
			database_url: "postgresql://localhost/tamanu-central".into(),
			pool: Some(pool),
			http: reqwest::Client::new(),
		})
	}

	/// A facility [`TamanuCx`] with no DB; central-only checks skip on it before
	/// ever touching the database.
	pub fn facility_ctx() -> TamanuCx {
		TamanuCx {
			app: ApplicationRef::tamanu(ApplicationKind::TamanuFacility),
			version: Version::parse("0.0.0").unwrap(),
			config: Arc::new(facility_config()),
			install_root: Some(std::path::PathBuf::from("/nonexistent")),
			database_url: "postgresql://localhost/tamanu-facility".into(),
			pool: None,
			http: reqwest::Client::new(),
		}
	}

	/// A [`PgCx`] for the cluster serving the local `tamanu-central` database,
	/// or `None` if it can't be reached.
	pub async fn cluster_ctx() -> Option<PgCx> {
		let url = "postgresql://localhost/tamanu-central";
		Some(PgCx {
			app: ApplicationRef::local_postgres(5432),
			database: Database::from_url(url).expect("a literal URL parses"),
			database_url: url.into(),
			pool: Some(connect("tamanu-central").await?),
		})
	}

	/// A [`PgCx`] with no pool; checks needing a connection skip on it before
	/// ever reaching the server.
	pub fn unreachable_cluster_ctx() -> PgCx {
		let url = "postgresql://localhost/tamanu-facility";
		PgCx {
			app: ApplicationRef::local_postgres(5432),
			database: Database::from_url(url).expect("a literal URL parses"),
			database_url: url.into(),
			pool: None,
		}
	}
}

#[cfg(test)]
mod tests {
	use serde_json::Value;

	use std::sync::Arc;

	use super::{
		CheckEntry, PgCx, Run, Runner, TamanuCx, all, fmt_db_error, query_error_check,
		test_support::central_ctx,
	};
	use crate::check::CheckStatus;

	/// Checks run concurrently, so each must get its own backend rather than
	/// queueing their queries onto one shared connection. Two connections taken
	/// at once must therefore be two different backends.
	///
	/// Skipped when the test database isn't reachable, like the other DB-backed
	/// tests here.
	#[tokio::test]
	async fn concurrent_checks_get_their_own_connections() {
		let Some(ctx) = central_ctx().await else {
			return;
		};

		let (first, second) = tokio::join!(ctx.db(), ctx.db());
		let first = first.expect("central_ctx reached the DB, so a connection is available");
		let second = second.expect("the pool serves more than one connection");

		let pid = async |conn: &bestool_postgres::pool::PgConnection| -> i32 {
			conn.query_one("SELECT pg_backend_pid()", &[])
				.await
				.expect("pg_backend_pid should answer")
				.get(0)
		};
		assert_ne!(
			pid(&first).await,
			pid(&second).await,
			"two checks sharing one backend would serialise their queries"
		);
	}

	/// A host with no reachable database has no pool, and every DB-dependent
	/// check skips rather than waiting on an acquire that cannot succeed.
	#[tokio::test]
	async fn no_pool_means_no_connection() {
		let ctx = super::test_support::facility_ctx();
		assert!(ctx.pool.is_none());
		assert!(ctx.db().await.is_none());
	}

	/// The runner for the check named `name`, and the arm it is filed under.
	fn entry_of(name: &str) -> CheckEntry {
		all().into_iter().find(|e| e.name == name).unwrap()
	}

	/// A deployment context for a Tamanu known only through its database: no
	/// install root, and a URL pointing at a closed port so connection attempts
	/// fail fast.
	fn db_only_ctx() -> TamanuCx {
		use bestool_tamanu::config::{Database, TamanuConfig};
		use node_semver::Version;

		use crate::subject::{ApplicationKind, ApplicationRef};

		let db = Database::from_url("postgresql://u@127.0.0.1:1/tamanu").unwrap();
		TamanuCx {
			app: ApplicationRef::tamanu(ApplicationKind::TamanuCentral),
			version: Version::parse("0.0.0").unwrap(),
			config: Arc::new(TamanuConfig::from_database(db)),
			install_root: None,
			database_url: "postgresql://u@127.0.0.1:1/tamanu".into(),
			pool: None,
			http: reqwest::Client::new(),
		}
	}

	/// The Postgres cluster on the same host, reached at the same closed port.
	fn postgres_ctx() -> PgCx {
		use bestool_tamanu::config::Database;

		use crate::subject::ApplicationRef;

		PgCx {
			app: ApplicationRef::local_postgres(1),
			database: Database::from_url("postgresql://u@127.0.0.1:1/tamanu").unwrap(),
			database_url: "postgresql://u@127.0.0.1:1/tamanu".into(),
			pool: None,
		}
	}

	/// The Tamanu runner for `name`.
	fn tamanu_runner(entry: &CheckEntry) -> &Runner<TamanuCx> {
		match &entry.run {
			Run::Tamanu(_, runner) => runner,
			Run::Postgres(_) => panic!("{} is a Postgres check", entry.name),
			Run::Machine(_) => panic!("{} is a machine check", entry.name),
		}
	}

	/// The Postgres runner for `name`.
	fn pg_runner(entry: &CheckEntry) -> &Runner<PgCx> {
		match &entry.run {
			Run::Postgres(runner) => runner,
			Run::Tamanu(..) => panic!("{} is a Tamanu check", entry.name),
			Run::Machine(_) => panic!("{} is a machine check", entry.name),
		}
	}

	/// The reason `fhir_config` gives when there are no install files to read
	/// its toggles from — the one check that legitimately gates on the install,
	/// and so the summary every other check must not produce.
	const NO_INSTALL_SUMMARY: &str = "no Tamanu config on this host";

	#[tokio::test]
	async fn checks_run_against_an_application_with_no_install() {
		// An application known only through its database gates only the checks
		// that genuinely read install files. `fhir_config` is the one that does,
		// and it is the positive control: without it this asserts nothing.
		let entry = entry_of("fhir_config");
		let check = (tamanu_runner(&entry).run)(db_only_ctx()).await;
		assert!(
			matches!(check.status, CheckStatus::Skip(_)),
			"fhir_config reads the install's config, so it skips without one"
		);
		assert_eq!(check.summary, NO_INSTALL_SUMMARY);

		// Everything else executes and, where its target is absent on the test
		// box, skips for its own reason rather than for want of an install.
		for name in [
			"tamanu_http",
			"tamanu_service",
			"version_drift",
			"caddy_certs",
			"http_errors",
		] {
			let entry = entry_of(name);
			let check = (tamanu_runner(&entry).run)(db_only_ctx()).await;
			assert_ne!(
				check.summary, NO_INSTALL_SUMMARY,
				"{name} should not be install-gated"
			);
		}
	}

	#[tokio::test]
	async fn postgres_connect_runs_against_an_application_with_no_install() {
		// `connect` only needs the URL. An unreachable one must FAIL (an alert),
		// proving it wasn't skipped for want of an install.
		let entry = entry_of("connect");
		let check = (pg_runner(&entry).run)(postgres_ctx()).await;
		assert!(
			matches!(check.status, CheckStatus::Fail(_)),
			"connect should run (and fail) against an application with no install, got {:?}",
			check.to_wire()["result"]
		);
	}

	/// The registry files each check under exactly one subject, and the arm it
	/// sits in is what says which. A machine runner carrying an application
	/// scope is not representable, so this asserts only that each check is in
	/// the arm its subject calls for.
	///
	/// spec: SUBJ
	#[test]
	fn checks_are_filed_under_the_subject_they_report_for() {
		use crate::subject::TamanuScope;

		#[derive(Debug, PartialEq, Eq)]
		enum Arm {
			Machine,
			Postgres,
			Tamanu(TamanuScope),
		}

		let arm_of = |name: &str| match entry_of(name).run {
			Run::Machine(_) => Arm::Machine,
			Run::Postgres(_) => Arm::Postgres,
			Run::Tamanu(scope, _) => Arm::Tamanu(scope),
		};

		// The machine's filesystems, its clock, and the front-end software
		// installed on it.
		for name in [
			"disk_free",
			"time_sync",
			"caddy_version",
			"caddyfile_version",
		] {
			assert_eq!(arm_of(name), Arm::Machine, "{name} reports for the machine");
		}
		// The cluster itself, not whatever reads from it.
		for name in ["connect", "version", "tuning", "checksums"] {
			assert_eq!(
				arm_of(name),
				Arm::Postgres,
				"{name} reports for the Postgres application"
			);
		}
		// The application's own data, traffic and certificates.
		for name in ["migrations", "caddy_certs", "http_errors"] {
			assert_eq!(
				arm_of(name),
				Arm::Tamanu(TamanuScope::Any),
				"{name} reports for a Tamanu application"
			);
		}
		assert_eq!(arm_of("fhir_workers"), Arm::Tamanu(TamanuScope::Central));
	}

	/// A context describes the deployment it was built for, so the role it
	/// reports comes from the subject rather than from whatever the sweep
	/// happened to resolve alongside it.
	///
	/// That a cluster answers with no role is not asserted here: `PgCx` has no
	/// `server_kind` to call, so asking one for a Tamanu role does not compile.
	///
	/// spec: SUBJ
	#[test]
	fn a_context_reports_the_role_of_its_own_subject() {
		use bestool_tamanu::ApiServerKind;

		use crate::subject::{ApplicationKind, ApplicationRef};

		let central = db_only_ctx();
		assert_eq!(central.server_kind(), ApiServerKind::Central);

		let facility = TamanuCx {
			app: ApplicationRef::tamanu(ApplicationKind::TamanuFacility),
			..db_only_ctx()
		};
		assert_eq!(facility.server_kind(), ApiServerKind::Facility);
	}

	/// A check is named within its subject, so the registry's names are unique
	/// per subject rather than across the whole catalogue.
	///
	/// spec: SUBJ
	#[test]
	fn qualified_names_are_unique() {
		let mut seen = std::collections::HashSet::new();
		for entry in all() {
			for slug in entry.possible_slugs() {
				let qualified = format!("{slug}:{}", entry.name);
				assert!(
					seen.insert(qualified.clone()),
					"two registry entries both produce {qualified}"
				);
			}
		}
	}

	/// Each check's heal travels in the same arm as the check, so it is handed
	/// the context the check ran with rather than a sweep-wide one.
	///
	/// spec: CHK#self-healing
	#[test]
	fn heals_sit_in_their_check_s_arm() {
		let machine_heal = match entry_of("canopy_registration").run {
			Run::Machine(runner) => runner.heal.is_some(),
			_ => panic!("canopy_registration is a machine check"),
		};
		assert!(machine_heal, "canopy_registration declares a machine heal");

		let tamanu_heal = match entry_of("fhir_jobs").run {
			Run::Tamanu(_, runner) => runner.heal.is_some(),
			_ => panic!("fhir_jobs is a Tamanu check"),
		};
		assert!(tamanu_heal, "fhir_jobs declares a Tamanu heal");
	}

	async fn query_err(sql: &str) -> Option<tokio_postgres::Error> {
		let ctx = central_ctx().await?;
		let client = ctx.db().await.expect("central_ctx always has a client");
		Some(
			client
				.query(sql, &[])
				.await
				.expect_err("query should error"),
		)
	}

	#[tokio::test]
	async fn schema_drift_is_broken() {
		// 42P01 undefined_table — the shape a dropped/renamed relation takes.
		let Some(err) = query_err("SELECT nope FROM no_such_table_bestool_test").await else {
			return;
		};
		let check = query_error_check("x", &err);
		assert!(matches!(check.status, CheckStatus::Broken(_)));
		assert_eq!(check.to_wire()["result"], Value::from("broken"));
	}

	#[tokio::test]
	async fn syntax_error_is_broken() {
		// 42601 syntax_error.
		let Some(err) = query_err("SELECT FROM WHERE").await else {
			return;
		};
		let check = query_error_check("x", &err);
		assert!(matches!(check.status, CheckStatus::Broken(_)));
	}

	#[tokio::test]
	async fn runtime_db_error_still_fails() {
		// 22012 division_by_zero — not class 42, so the deployment is blamed.
		let Some(err) = query_err("SELECT 1/0").await else {
			return;
		};
		let check = query_error_check("x", &err);
		assert!(check.status.is_fatal());
		assert_eq!(check.to_wire()["result"], Value::from("failed"));
	}

	#[tokio::test]
	async fn fmt_db_error_surfaces_the_real_message() {
		// A genuine DbError (as opposed to an IO/connect failure) must not
		// collapse to tokio_postgres::Error's unhelpful top-level Display
		// ("db error") — that's the whole reason this helper exists.
		let Some(err) = query_err("SELECT 1/0").await else {
			return;
		};
		let formatted = fmt_db_error(&err);
		assert_ne!(formatted, "db error");
		assert!(
			formatted.contains("division by zero"),
			"expected the real postgres message, got {formatted:?}"
		);
	}
}
