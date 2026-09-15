//! Upstream Tamanu records that never became FHIR resources.
//!
//! Every other FHIR check measures the queue machinery rather than its outcome:
//! `fhir_jobs` measures pending work, `fhir_job_errors` measures work that
//! failed, `fhir_workers` measures worker liveness, and
//! `fhir_service_requests_unresolved` measures the resolution state of rows that
//! have already been materialised. None sees an upstream record for which no
//! materialisation was ever queued — a missed trigger, materialisation switched
//! off for that resource, or a truncated job queue without the re-materialisation
//! that recovering from it needs. In that state all four read green while the
//! record is invisible to any integration consuming the FHIR API.
//!
//! The age of the oldest gap carries the grading, not the count: there is always
//! a transient count in the moments between an upstream write and its
//! materialisation, so a count threshold either alerts constantly or is set high
//! enough to miss a real gap.
//!
//! Each resource is graded on its own clock. Most materialise off the upstream
//! write and are expected to keep pace with it; `MediciReport` is materialised
//! behind all of them and carries a large standing backlog by design, so the
//! thresholds that catch a stalled `Patient` would fail it permanently on a
//! deployment working exactly as intended.
//!
//! spec: CHK-FMA

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use tokio_postgres::{Client as PgClient, error::SqlState};

use super::util::humanise_age;
use super::{CheckContext, query_error_check};
use crate::Stat;
use crate::check::Check;

const NAME: &str = "fhir_materialisation";

/// How promptly a resource is expected to materialise, which sets the thresholds
/// its gap is graded against, the window its measurement is bounded to, and
/// whether its backlog joins the check's headline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pace {
	/// Materialised off the upstream write and expected to keep pace with it, so
	/// a gap persisting beyond minutes is an incident.
	Prompt,
	/// Materialised behind every prompt resource, so a large standing backlog is
	/// normal and only a gap old enough to mean materialisation has stopped
	/// altogether is an incident.
	Deferred,
}

impl Pace {
	/// Upstream records older than this are out of scope: a gap that old is a
	/// backfill concern rather than an incident, and bounding the measurement
	/// keeps it cheap enough to run on every sweep. There is no index on
	/// `fhir.*.upstream_id`, so each resource's join scans its FHIR table.
	///
	/// The window always outlasts [`fail_secs`](Self::fail_secs), or a gap would
	/// leave the measurement before it could age into failing.
	fn window(self) -> &'static str {
		match self {
			Pace::Prompt => "48 hours",
			Pace::Deferred => "7 days",
		}
	}

	/// Age at which the gap warns, or `None` for a resource whose backlog is
	/// expected: there is no degraded state between working and stopped.
	fn warn_secs(self) -> Option<i64> {
		match self {
			Pace::Prompt => Some(15 * 60),
			Pace::Deferred => None,
		}
	}

	/// Age at which the gap fails.
	fn fail_secs(self) -> i64 {
		match self {
			Pace::Prompt => 60 * 60,
			Pace::Deferred => 5 * 24 * 60 * 60,
		}
	}

	/// Whether the resource's backlog joins the check's headline count and the
	/// oldest gap it names. A deferred backlog is expected and routinely the
	/// largest number the check holds, so counting it there would bury the gaps
	/// that do mean something.
	fn in_headline(self) -> bool {
		matches!(self, Pace::Prompt)
	}

	fn as_str(self) -> &'static str {
		match self {
			Pace::Prompt => "prompt",
			Pace::Deferred => "deferred",
		}
	}
}

/// Setting and config key under which the per-resource materialisation flags
/// live — `fhir.worker.…` as a setting (Tamanu 2.60 and later),
/// `integrations.fhir.worker.…` in config (earlier).
const ENABLEMENT_KEY: &str = "fhir.worker.resourceMaterialisationEnabled";

/// One upstream table a resource materialises from, and the predicate narrowing
/// it to the rows Tamanu itself considers. Columns are qualified `u.` to match
/// the alias the gap query gives the upstream table.
struct Upstream {
	table: &'static str,
	filter: Option<&'static str>,
}

/// A materialised FHIR resource and where it materialises from.
///
/// The relationship is not recoverable from the schema: it is an arbitrary
/// declaration in Tamanu, and `upstream_id` is polymorphic where a resource has
/// more than one upstream, so there is no key to follow. `name` is the resource
/// name the enablement flags are keyed by, which `table` does not yield —
/// `non_fhir_medici_report` belongs to `MediciReport`.
struct Resource {
	name: &'static str,
	table: &'static str,
	pace: Pace,
	upstreams: &'static [Upstream],
}

const RESOURCES: &[Resource] = &[
	Resource {
		name: "ServiceRequest",
		table: "service_requests",
		pace: Pace::Prompt,
		upstreams: &[
			Upstream {
				table: "lab_requests",
				filter: None,
			},
			Upstream {
				table: "imaging_requests",
				filter: None,
			},
		],
	},
	Resource {
		name: "Patient",
		table: "patients",
		pace: Pace::Prompt,
		upstreams: &[Upstream {
			table: "patients",
			filter: None,
		}],
	},
	Resource {
		name: "Practitioner",
		table: "practitioners",
		pace: Pace::Prompt,
		upstreams: &[Upstream {
			table: "users",
			filter: None,
		}],
	},
	Resource {
		name: "Organization",
		table: "organizations",
		pace: Pace::Prompt,
		upstreams: &[Upstream {
			table: "facilities",
			filter: None,
		}],
	},
	Resource {
		name: "Immunization",
		table: "immunizations",
		pace: Pace::Prompt,
		upstreams: &[Upstream {
			table: "administered_vaccines",
			filter: None,
		}],
	},
	Resource {
		name: "MedicationRequest",
		table: "medication_requests",
		pace: Pace::Prompt,
		upstreams: &[Upstream {
			table: "pharmacy_order_prescriptions",
			filter: None,
		}],
	},
	Resource {
		name: "Specimen",
		table: "specimens",
		pace: Pace::Prompt,
		upstreams: &[Upstream {
			table: "lab_requests",
			filter: Some("u.specimen_attached = true"),
		}],
	},
	Resource {
		name: "Encounter",
		table: "encounters",
		pace: Pace::Prompt,
		upstreams: &[Upstream {
			table: "encounters",
			filter: Some("u.encounter_type <> 'surveyResponse'"),
		}],
	},
	// Not a FHIR resource served to integrations but a single-purpose report,
	// materialised behind everything else and so permanently backlogged.
	Resource {
		name: "MediciReport",
		table: "non_fhir_medici_report",
		pace: Pace::Deferred,
		upstreams: &[Upstream {
			table: "encounters",
			filter: Some("u.encounter_type <> 'surveyResponse'"),
		}],
	},
];

/// Which of the enablement sources answered for a resource, reported alongside
/// its numbers so an operator can tell a reading from an inference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
	/// The deployment's stored setting.
	Setting,
	/// The deployment's configuration, where Tamanu before 2.60 keeps the flags.
	Config,
	/// Inferred from whether the resource has ever materialised anything.
	Observed,
}

impl Source {
	fn as_str(self) -> &'static str {
		match self {
			Source::Setting => "setting",
			Source::Config => "config",
			Source::Observed => "observed",
		}
	}
}

/// One enabled resource's measurement.
struct Measured {
	name: &'static str,
	pace: Pace,
	source: Source,
	gap: i64,
	lag_secs: i64,
}

/// Where one resource's oldest gap sits against that resource's own thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Grade {
	Clean,
	Warn,
	Fail,
}

impl Measured {
	fn grade(&self) -> Grade {
		if self.lag_secs > self.pace.fail_secs() {
			Grade::Fail
		} else if self
			.pace
			.warn_secs()
			.is_some_and(|warn| self.lag_secs > warn)
		{
			Grade::Warn
		} else {
			Grade::Clean
		}
	}

	/// The threshold this measurement crossed, for the check's reason to name.
	/// `None` when it crossed neither.
	fn crossed(&self) -> Option<i64> {
		match self.grade() {
			Grade::Fail => Some(self.pace.fail_secs()),
			Grade::Warn => self.pace.warn_secs(),
			Grade::Clean => None,
		}
	}
}

/// The materialised resources this deployment has: tables in the `fhir` schema
/// carrying an `upstream_id`. That is exactly the set of resources Tamanu
/// materialises — resources computed on read have no table at all — so it needs
/// no filtering against the resource names.
const DISCOVER_SQL: &str = "\
	SELECT table_name FROM information_schema.columns \
	WHERE table_schema = 'fhir' AND column_name = 'upstream_id'";

/// Stored per-resource flags, leaf rows and the whole-object row alike. Global
/// and facility rows come back together: the deployment merges the flags across
/// facilities so that enabling a resource for one facility enables it
/// server-wide, which is a union, so any `true` wins.
const SETTINGS_SQL: &str = "\
	SELECT key, value FROM settings \
	WHERE (key = $1 OR key LIKE $1 || '.%') AND deleted_at IS NULL";

pub async fn run(ctx: CheckContext) -> Check {
	if !ctx.config.fhir_worker_enabled() {
		return Check::skip(
			NAME,
			"FHIR worker not enabled",
			"no upstream record is expected to be materialised, so every resource would read as a total gap",
		);
	}
	let Some(client) = ctx.db().await else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let discovered = match client.query(DISCOVER_SQL, &[]).await {
		Ok(rows) => rows
			.iter()
			.filter_map(|row| row.try_get::<_, String>("table_name").ok())
			.collect::<BTreeSet<_>>(),
		Err(err) => return query_error_check(NAME, &err),
	};
	if discovered.is_empty() {
		return Check::skip(
			NAME,
			"no materialised FHIR resources",
			"the fhir schema has no table carrying an upstream_id",
		);
	}

	// A deployment old enough to have no settings table declares enablement in
	// config instead, so its absence falls through rather than breaking the check.
	let settings = match client.query(SETTINGS_SQL, &[&ENABLEMENT_KEY]).await {
		Ok(rows) => enablement_from_settings(&rows),
		Err(err) if is_missing_relation(&err) => BTreeMap::new(),
		Err(err) => return query_error_check(NAME, &err),
	};

	let mut measured: Vec<Measured> = Vec::new();
	let mut disabled: BTreeMap<&str, &str> = BTreeMap::new();
	let mut absent: Vec<&str> = Vec::new();
	let mut errored: BTreeMap<&str, String> = BTreeMap::new();

	for resource in RESOURCES {
		if !discovered.contains(resource.table) {
			// The resource has no table on this Tamanu version. Absent by
			// design, not a gap in coverage.
			continue;
		}

		let (enabled, source) = match resolve_enablement(resource, &settings, &ctx) {
			Some(resolved) => resolved,
			None => match has_any_row(&client, resource.table).await {
				Ok(any) => (any, Source::Observed),
				Err(err) => {
					errored.insert(resource.name, err.to_string());
					continue;
				}
			},
		};
		if !enabled {
			disabled.insert(resource.name, source.as_str());
			continue;
		}

		match measure(&client, resource).await {
			Ok((gap, lag_secs)) => measured.push(Measured {
				name: resource.name,
				pace: resource.pace,
				source,
				gap,
				lag_secs,
			}),
			Err(err) if is_missing_relation(&err) => absent.push(resource.name),
			Err(err) => {
				errored.insert(resource.name, err.to_string());
			}
		}
	}

	// Every query is done; what follows is grading. This check is the sweep's
	// long pole — schema discovery plus up to two aggregate queries per resource
	// — so holding a slot past the last of them would make everything still
	// queueing wait on arithmetic.
	drop(client);

	let unmonitored = unmonitored(&discovered);

	if measured.is_empty() && errored.is_empty() && unmonitored.is_empty() {
		return Check::skip(
			NAME,
			"no resource has materialisation enabled",
			"nothing is expected to materialise, so there is no gap to measure",
		);
	}

	let summary = summarise(&measured, errored.len());

	// Thresholds differ per resource, so the reason names which one drove the
	// grade rather than quoting a single threshold for the check as a whole.
	let breach = breach(&measured).map(|(m, threshold)| {
		(
			m.grade(),
			format!(
				"{} unmaterialised for over {}",
				m.name,
				humanise_age(threshold)
			),
		)
	});

	let mut check = if let Some((grade, reason)) = breach {
		if grade == Grade::Fail {
			Check::fail(NAME, summary, reason)
		} else {
			Check::warning(NAME, summary, reason)
		}
	} else if !unmonitored.is_empty() {
		Check::warning(
			NAME,
			summary,
			format!(
				"materialised resource this check does not know about: {}",
				unmonitored.join(", ")
			),
		)
	} else if !errored.is_empty() {
		Check::warning(
			NAME,
			summary,
			format!(
				"could not measure: {}",
				errored.keys().copied().collect::<Vec<_>>().join(", ")
			),
		)
	} else {
		Check::pass(NAME, summary)
	};

	let mut breakdown = Map::new();
	for m in &measured {
		breakdown.insert(
			m.name.to_string(),
			json!({
				"gap": m.gap,
				"lag_seconds": m.lag_secs,
				"enablement": m.source.as_str(),
				"pace": m.pace.as_str(),
				"fails_after_seconds": m.pace.fail_secs(),
			}),
		);
		check = check
			.with_stat(
				Stat::gauge("gap", m.gap as f64)
					.label("resource", m.name)
					.group("gap")
					.help("Upstream records with no materialised FHIR resource"),
			)
			.with_stat(
				Stat::gauge("lag_seconds", m.lag_secs as f64)
					.label("resource", m.name)
					.group("lag_seconds")
					.help("Age of the oldest upstream record with no materialised FHIR resource"),
			);
	}

	check = check
		.with_detail("resources", Value::Object(breakdown))
		.with_stat(
			Stat::gauge("unmonitored", unmonitored.len() as f64)
				.help("Materialised FHIR resources this check has no upstream relationship for"),
		);

	if !disabled.is_empty() {
		check = check.with_detail("disabled", json!(disabled));
	}
	if !absent.is_empty() {
		check = check.with_detail("upstream_absent", json!(absent));
	}
	if !errored.is_empty() {
		check = check.with_detail("errored", json!(errored));
	}
	if !unmonitored.is_empty() {
		check = check.with_detail("unmonitored", json!(unmonitored));
	}

	check
}

/// The measurement driving the check's grade, and the threshold it crossed.
///
/// Only a resource past one of its own thresholds can grade the check. Between
/// two that are, the more severe grade wins; between two of the same grade, the
/// one furthest past its threshold in proportion to it, so an hour past an hour
/// outranks a day past five days.
fn breach(measured: &[Measured]) -> Option<(&Measured, i64)> {
	measured
		.iter()
		.filter_map(|m| Some((m, m.crossed()?)))
		.max_by_key(|(m, threshold)| {
			(
				m.grade(),
				m.lag_secs.saturating_mul(100) / (*threshold).max(1),
			)
		})
}

/// The check's headline.
///
/// The count and the oldest gap it names are drawn from the prompt resources
/// alone. A deferred resource's backlog is expected and routinely larger than
/// every other number here, so folding it in would bury them; it gets its own
/// clause instead, so its state is still on the headline without distorting the
/// rest.
fn summarise(measured: &[Measured], errored: usize) -> String {
	// Nothing measured is not the same as nothing missing, so it must not read
	// as a clean result.
	if measured.is_empty() {
		return if errored > 0 {
			format!("no resource measured, {errored} could not be read")
		} else {
			"no resource measured".to_string()
		};
	}

	let headline: Vec<&Measured> = measured.iter().filter(|m| m.pace.in_headline()).collect();
	let total_gap: i64 = headline.iter().map(|m| m.gap).sum();

	let mut parts = Vec::new();
	match headline.iter().max_by_key(|m| m.lag_secs) {
		Some(oldest) if total_gap > 0 => parts.push(format!(
			"{total_gap} unmaterialised, oldest {} ({})",
			humanise_age(oldest.lag_secs),
			oldest.name,
		)),
		Some(_) => parts.push(format!(
			"no materialisation gap across {} resources",
			headline.len()
		)),
		None => {}
	}
	for m in measured.iter().filter(|m| !m.pace.in_headline()) {
		parts.push(if m.gap > 0 {
			format!("{} {} behind", m.name, humanise_age(m.lag_secs))
		} else {
			format!("{} up to date", m.name)
		});
	}

	parts.join("; ")
}

/// Materialised resources in the schema that [`RESOURCES`] has no relationship
/// for, so a Tamanu version that adds one reports that this check has gone out
/// of date rather than silently narrowing its coverage.
///
/// The cross-reference runs schema→known only. A resource this check knows about
/// with no table in the deployment is absent by design on that version.
fn unmonitored(discovered: &BTreeSet<String>) -> Vec<String> {
	discovered
		.iter()
		.filter(|table| !RESOURCES.iter().any(|r| r.table == table.as_str()))
		.cloned()
		.collect()
}

/// Per-resource flags from the settings rows, keyed by resource name.
fn enablement_from_settings(rows: &[tokio_postgres::Row]) -> BTreeMap<String, bool> {
	let pairs = rows
		.iter()
		.filter_map(|row| {
			Some((
				row.try_get::<_, String>("key").ok()?,
				row.try_get::<_, Value>("value").ok()?,
			))
		})
		.collect::<Vec<_>>();
	merge_enablement(&pairs)
}

/// Merge settings key/value pairs into per-resource flags.
///
/// Rows are written at leaf granularity (`…resourceMaterialisationEnabled.Patient`),
/// but the whole object can also be stored under the parent key, so both shapes
/// are read. Values are union-merged, matching how the deployment merges the
/// setting across facilities: a resource enabled for any one facility is enabled
/// server-wide.
fn merge_enablement(pairs: &[(String, Value)]) -> BTreeMap<String, bool> {
	let mut flags: BTreeMap<String, bool> = BTreeMap::new();
	let mut merge = |name: String, value: &Value| {
		let enabled = value.as_bool().unwrap_or(false);
		let entry = flags.entry(name).or_insert(false);
		*entry = *entry || enabled;
	};

	for (key, value) in pairs {
		if let Some(name) = key.strip_prefix(ENABLEMENT_KEY).and_then(|rest| {
			rest.strip_prefix('.')
				.filter(|name| !name.is_empty() && !name.contains('.'))
		}) {
			merge(name.to_string(), value);
		} else if key == ENABLEMENT_KEY
			&& let Value::Object(obj) = value
		{
			for (name, value) in obj {
				merge(name.clone(), value);
			}
		}
	}

	flags
}

/// Enablement as declared: the stored setting first, then configuration, which
/// is where Tamanu before 2.60 keeps the same flags. `None` when neither
/// declares this resource, leaving it to be inferred.
fn resolve_enablement(
	resource: &Resource,
	settings: &BTreeMap<String, bool>,
	ctx: &CheckContext,
) -> Option<(bool, Source)> {
	if let Some(&enabled) = settings.get(resource.name) {
		return Some((enabled, Source::Setting));
	}
	ctx.config
		.fhir_resource_materialisation_enabled()
		.get(resource.name)
		.map(|&enabled| (enabled, Source::Config))
}

/// Whether the resource has ever materialised anything, as the last resort for
/// enablement. Stops at the first row rather than counting.
async fn has_any_row(client: &PgClient, table: &str) -> Result<bool, tokio_postgres::Error> {
	let sql = format!("SELECT EXISTS (SELECT 1 FROM fhir.{table} LIMIT 1) AS present");
	client.query_one(&sql, &[]).await?.try_get("present")
}

/// The number of upstream records inside the window with no FHIR row, and the
/// age in seconds of the oldest of them.
async fn measure(
	client: &PgClient,
	resource: &Resource,
) -> Result<(i64, i64), tokio_postgres::Error> {
	let row = client.query_one(&gap_query(resource), &[]).await?;
	Ok((row.try_get("gap")?, row.try_get("lag_seconds")?))
}

/// The gap query for one resource.
///
/// A resource materialising from more than one upstream table unions them before
/// aggregating, so it reports one gap and one age across all of them rather than
/// one per upstream.
///
/// `now()` is right and `localtimestamp` would be wrong: Tamanu stores clinical
/// datetimes as naive strings in the deployment's primary timezone, but the audit
/// columns this reads are `timestamp with time zone`.
///
/// Presence of the FHIR row is the whole test, and `resolved` is not consulted: a
/// materialised but unresolved row is graded by
/// `fhir_service_requests_unresolved` and must not count as a gap as well.
/// Soft-deleted upstream records are excluded so a cancelled clinical record does
/// not read as a gap.
fn gap_query(resource: &Resource) -> String {
	let branches = resource
		.upstreams
		.iter()
		.map(|upstream| {
			let filter = upstream
				.filter
				.map(|f| format!(" AND {f}"))
				.unwrap_or_default();
			format!(
				"SELECT u.created_at FROM {upstream} u \
				 LEFT JOIN fhir.{resource} r ON r.upstream_id = u.id \
				 WHERE r.id IS NULL AND u.deleted_at IS NULL \
				 AND u.created_at > now() - interval '{window}'{filter}",
				upstream = upstream.table,
				resource = resource.table,
				window = resource.pace.window(),
			)
		})
		.collect::<Vec<_>>()
		.join(" UNION ALL ");

	format!(
		"SELECT count(*)::bigint AS gap, \
		 coalesce(max(extract(epoch FROM now() - g.created_at)), 0)::bigint AS lag_seconds \
		 FROM ({branches}) g"
	)
}

/// Whether the error is Postgres reporting a table or schema that isn't there,
/// which for an upstream table means this Tamanu version predates it.
fn is_missing_relation(err: &tokio_postgres::Error) -> bool {
	err.as_db_error().is_some_and(|db| {
		db.code() == &SqlState::UNDEFINED_TABLE || db.code() == &SqlState::INVALID_SCHEMA_NAME
	})
}

#[cfg(test)]
mod tests {
	use std::sync::Arc;

	use bestool_tamanu::config::TamanuConfig;

	use super::*;
	use crate::check::CheckStatus;
	use crate::checks::test_support::{central_ctx, facility_ctx};

	/// A central config with the FHIR worker on, so the check proceeds past the
	/// applicability gate, carrying the given per-resource flags where Tamanu
	/// before 2.60 keeps them.
	fn worker_config(flags: &[(&str, bool)]) -> Arc<TamanuConfig> {
		let materialisation: Map<String, Value> = flags
			.iter()
			.map(|(name, on)| ((*name).to_string(), json!(on)))
			.collect();
		Arc::new(
			serde_json::from_value(json!({
				"db": { "name": "tamanu-central", "username": "u", "password": "p" },
				"integrations": { "fhir": { "worker": {
					"enabled": true,
					"resourceMaterialisationEnabled": materialisation,
				}}},
			}))
			.expect("test config should parse"),
		)
	}

	/// A central context whose FHIR worker is enabled. `None` when the local
	/// database is unavailable, as with [`central_ctx`].
	async fn central_worker_enabled() -> Option<CheckContext> {
		let mut ctx = central_ctx().await?;
		ctx.config = worker_config(&[]);
		Some(ctx)
	}

	#[tokio::test]
	async fn runs_against_central() {
		let Some(ctx) = central_worker_enabled().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, NAME);
		assert!(
			matches!(
				check.status,
				CheckStatus::Pass | CheckStatus::Warning(_) | CheckStatus::Fail(_)
			),
			"the check should reach a verdict on a central with the worker on: {:?}",
			check.status
		);
	}

	#[tokio::test]
	async fn skips_when_the_worker_is_disabled() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		// The stock test config has no `integrations` block, so the worker reads
		// as off and every resource would report a total gap.
		let check = super::run(ctx).await;
		assert!(check.status.is_skip());
	}

	#[tokio::test]
	async fn skips_on_facility() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}

	/// Seed an upstream record that never materialised and check the whole path
	/// grades it: schema discovery, the stored setting winning over the absent
	/// config flag, the gap and its age, and the resulting failure.
	///
	/// The fixture has to be committed, because the check reads through its own
	/// pooled connection and would not see rows left uncommitted on this one.
	/// That makes this test write for real to whichever database answers at the
	/// central URL — on a developer or ops machine, a live Tamanu — so it runs
	/// only when asked for by name:
	///
	/// ```text
	/// BESTOOL_TEST_DESTRUCTIVE_DB=1 cargo test -p bestool-alertd grades_a_seeded_gap
	/// ```
	///
	/// It only ever creates and removes its own rows. It never edits a setting
	/// the deployment already has: if one is present it declines to run, rather
	/// than saving and restoring a value that might itself be the leftover of an
	/// earlier run that was interrupted before it could clean up. The cleanup
	/// runs before the assertions, so a failing assertion still leaves the
	/// database as it found it.
	#[tokio::test]
	async fn grades_a_seeded_gap_against_central() {
		/// The setting the check reads to decide the resource is enabled. A
		/// deployment may legitimately have its own value here.
		const SETTING: &str = "fhir.worker.resourceMaterialisationEnabled.Patient";
		const PROBE: &str = "fhir-materialisation-probe";
		const LIVE_SETTING: &str = "SELECT value FROM settings \
			 WHERE key = $1 AND facility_id IS NULL AND deleted_at IS NULL";

		if std::env::var_os("BESTOOL_TEST_DESTRUCTIVE_DB").is_none() {
			return;
		}

		let Some(ctx) = central_worker_enabled().await else {
			return;
		};
		let client = ctx.db().await.expect("central_ctx carries a connection");

		// The test owns the setting for the duration or it doesn't run. A row
		// already here is either the deployment's own configuration, which is
		// not ours to edit, or the residue of an interrupted run, which we
		// can't tell apart from it — so leave both alone and say so.
		let existing = client
			.query_opt(LIVE_SETTING, &[&SETTING])
			.await
			.expect("reading the current setting should succeed");
		assert!(
			existing.is_none(),
			"{SETTING} is already set on this database; this test will not edit \
			 an existing setting. If an interrupted run left it behind, remove \
			 the row (and any '{PROBE}' patient) and run again."
		);

		// The patient first and the setting last. Everything before the setting
		// can fail without consequence — the probe is a row only this test
		// creates, and the next run's own delete clears it — whereas a failure
		// after the setting is written leaves materialisation switched on for
		// the deployment's FHIR worker, which is exactly what this test is not
		// allowed to do.
		client
			.execute("DELETE FROM patients WHERE id = $1", &[&PROBE])
			.await
			.expect("clearing any stale probe should succeed");
		client
			.execute(
				"INSERT INTO patients \
				 (id, created_at, updated_at, display_id, first_name, last_name, sex) \
				 VALUES ($1, now() - interval '3 hours', now(), \
				 'FHIRMATPROBE', 'Gap', 'Probe', 'other')",
				&[&PROBE],
			)
			.await
			.expect("seeding the gap should succeed");
		client
			.execute(
				"INSERT INTO settings (key, value) VALUES ($1, 'true')",
				&[&SETTING],
			)
			.await
			.expect("enabling the resource should succeed");

		let check = super::run(ctx).await;

		// Both rows are ours, so this puts the database back as it was found.
		// Run both regardless of each other: short-circuiting on the patient
		// would leave the setting behind, which both changes the deployment's
		// behaviour and blocks every later run through the guard above.
		let patient_removed = client
			.execute("DELETE FROM patients WHERE id = $1", &[&PROBE])
			.await;
		let setting_removed = client
			.execute(
				"DELETE FROM settings \
				 WHERE key = $1 AND facility_id IS NULL AND deleted_at IS NULL",
				&[&SETTING],
			)
			.await;
		let cleaned_up = patient_removed.and(setting_removed);

		assert!(
			matches!(check.status, CheckStatus::Fail(_)),
			"a 3h-old unmaterialised record should fail: {:?} — {}",
			check.status,
			check.summary
		);

		let patient = check
			.details
			.get("resources")
			.and_then(|r| r.get("Patient"))
			.expect("the enabled resource should appear in the breakdown")
			.clone();
		assert_eq!(
			patient["enablement"], "setting",
			"the stored setting should answer, not inference"
		);
		assert!(
			patient["gap"].as_i64().unwrap_or(0) >= 1,
			"the seeded record should be counted: {patient}"
		);
		assert!(
			patient["lag_seconds"].as_i64().unwrap_or(0) >= 3 * 60 * 60,
			"the age should be at least the seeded 3h: {patient}"
		);
		assert!(
			check.stats.iter().any(|s| s.name == "lag_seconds"
				&& s.labels
					.iter()
					.any(|(k, v)| *k == "resource" && v == "Patient")),
			"the resource should be a metric label"
		);

		cleaned_up.expect("restoring the database should succeed");
	}

	#[test]
	fn every_resource_has_at_least_one_upstream() {
		for resource in RESOURCES {
			assert!(
				!resource.upstreams.is_empty(),
				"{} has no upstream table",
				resource.name
			);
		}
	}

	#[test]
	fn resource_names_and_tables_are_unique() {
		let names: BTreeSet<_> = RESOURCES.iter().map(|r| r.name).collect();
		let tables: BTreeSet<_> = RESOURCES.iter().map(|r| r.table).collect();
		assert_eq!(names.len(), RESOURCES.len());
		assert_eq!(tables.len(), RESOURCES.len());
	}

	#[test]
	fn unmonitored_flags_a_resource_the_map_does_not_know() {
		let discovered = ["service_requests", "patients", "appointments"]
			.into_iter()
			.map(String::from)
			.collect();
		assert_eq!(unmonitored(&discovered), vec!["appointments".to_string()]);
	}

	#[test]
	fn unmonitored_is_empty_when_the_map_covers_the_schema() {
		let discovered = RESOURCES.iter().map(|r| r.table.to_string()).collect();
		assert!(unmonitored(&discovered).is_empty());
	}

	#[test]
	fn unmonitored_ignores_a_resource_absent_from_this_version() {
		// A known resource with no table is absent by design on that version,
		// not a gap in coverage: the cross-reference runs schema→known only.
		let discovered = ["service_requests"].into_iter().map(String::from).collect();
		assert!(unmonitored(&discovered).is_empty());
	}

	#[test]
	fn gap_query_unions_multiple_upstreams() {
		let service_requests = RESOURCES
			.iter()
			.find(|r| r.name == "ServiceRequest")
			.unwrap();
		let sql = gap_query(service_requests);
		assert_eq!(sql.matches("UNION ALL").count(), 1);
		assert!(sql.contains("FROM lab_requests u"));
		assert!(sql.contains("FROM imaging_requests u"));
		assert!(sql.contains("LEFT JOIN fhir.service_requests r ON r.upstream_id = u.id"));
	}

	#[test]
	fn gap_query_applies_the_upstream_filter() {
		let specimens = RESOURCES.iter().find(|r| r.name == "Specimen").unwrap();
		let sql = gap_query(specimens);
		assert!(sql.contains("AND u.specimen_attached = true"));
		assert!(!sql.contains("UNION ALL"));

		let encounters = RESOURCES.iter().find(|r| r.name == "Encounter").unwrap();
		assert!(
			gap_query(encounters).contains("AND u.encounter_type <> 'surveyResponse'"),
			"encounter filter missing"
		);
	}

	/// A measurement as it would come back from [`measure`], for grading and
	/// summarising without a database.
	fn measured(name: &'static str, pace: Pace, gap: i64, lag_secs: i64) -> Measured {
		Measured {
			name,
			pace,
			source: Source::Setting,
			gap,
			lag_secs,
		}
	}

	/// A window interval literal in seconds, so the invariant that a window
	/// outlasts its fail threshold can be asserted rather than eyeballed.
	fn window_secs(window: &str) -> i64 {
		let (count, unit) = window.split_once(' ').expect("window is '<count> <unit>'");
		let count: i64 = count.parse().expect("window count should be a number");
		match unit {
			"hours" => count * 60 * 60,
			"days" => count * 24 * 60 * 60,
			other => panic!("window unit {other} is not handled here"),
		}
	}

	#[test]
	fn medici_report_is_the_only_deferred_resource() {
		let deferred: Vec<_> = RESOURCES
			.iter()
			.filter(|r| r.pace == Pace::Deferred)
			.map(|r| r.name)
			.collect();
		assert_eq!(deferred, vec!["MediciReport"]);
	}

	#[test]
	fn every_window_outlasts_its_fail_threshold() {
		// A gap that leaves the measurement before reaching the threshold could
		// never fail the check, so the deferred window has to cover its five days.
		for pace in [Pace::Prompt, Pace::Deferred] {
			assert!(
				window_secs(pace.window()) > pace.fail_secs(),
				"the {} window of {} does not outlast its fail threshold of {}",
				pace.as_str(),
				pace.window(),
				humanise_age(pace.fail_secs()),
			);
		}
	}

	#[test]
	fn a_deferred_resource_is_graded_on_its_own_clock() {
		// Three days behind is a stalled prompt resource many times over, and
		// well within what a deferred one runs at by design.
		assert_eq!(
			measured("Patient", Pace::Prompt, 12, 3 * 86400).grade(),
			Grade::Fail
		);
		assert_eq!(
			measured("MediciReport", Pace::Deferred, 40_000, 3 * 86400).grade(),
			Grade::Clean
		);
		assert_eq!(
			measured("MediciReport", Pace::Deferred, 40_000, 6 * 86400).grade(),
			Grade::Fail
		);
	}

	#[test]
	fn a_deferred_resource_never_warns() {
		// Its backlog is expected, so there is no degraded state between working
		// and stopped: it is clean right up to the moment it fails.
		let at_the_threshold = measured(
			"MediciReport",
			Pace::Deferred,
			40_000,
			Pace::Deferred.fail_secs(),
		);
		assert_eq!(at_the_threshold.grade(), Grade::Clean);
		assert_eq!(at_the_threshold.crossed(), None);
	}

	#[test]
	fn a_deferred_backlog_does_not_grade_the_check() {
		let all = [
			measured("Patient", Pace::Prompt, 2, 90),
			measured("MediciReport", Pace::Deferred, 40_000, 3 * 86400),
		];
		assert!(
			breach(&all).is_none(),
			"a backlog within the deferred clock must leave the check passing"
		);
	}

	#[test]
	fn a_deferred_resource_fails_once_it_has_plainly_stopped() {
		let all = [
			measured("Patient", Pace::Prompt, 0, 30),
			measured("MediciReport", Pace::Deferred, 40_000, 6 * 86400),
		];
		let (worst, threshold) = breach(&all).expect("the stalled resource should grade");
		assert_eq!(worst.name, "MediciReport");
		assert_eq!(worst.grade(), Grade::Fail);
		assert_eq!(threshold, 5 * 86400, "the reason quotes the deferred clock");
	}

	#[test]
	fn the_worst_breach_is_the_one_furthest_past_its_own_threshold() {
		let all = [
			measured("Patient", Pace::Prompt, 3, 2 * 3600),
			measured("MediciReport", Pace::Deferred, 40_000, 6 * 86400),
		];
		let (worst, _) = breach(&all).expect("both have crossed a threshold");
		assert_eq!(
			worst.name, "Patient",
			"twice an hour outranks a day past five days"
		);
	}

	#[test]
	fn a_deferred_backlog_stays_out_of_the_headline_count() {
		let summary = summarise(
			&[
				measured("Patient", Pace::Prompt, 2, 90),
				measured("MediciReport", Pace::Deferred, 40_000, 3 * 86400),
			],
			0,
		);
		assert_eq!(
			summary, "2 unmaterialised, oldest 1m (Patient); MediciReport 3d behind",
			"the deferred backlog is reported in its own right, not added to the count"
		);
	}

	#[test]
	fn a_clean_deferred_resource_still_reports() {
		let summary = summarise(
			&[
				measured("Patient", Pace::Prompt, 0, 0),
				measured("MediciReport", Pace::Deferred, 0, 0),
			],
			0,
		);
		assert_eq!(
			summary,
			"no materialisation gap across 1 resources; MediciReport up to date"
		);
	}

	#[test]
	fn summary_without_measurements_does_not_read_as_clean() {
		assert_eq!(summarise(&[], 0), "no resource measured");
		assert_eq!(
			summarise(&[], 2),
			"no resource measured, 2 could not be read"
		);
	}

	#[test]
	fn deferred_resource_gets_the_longer_window() {
		let medici = RESOURCES.iter().find(|r| r.name == "MediciReport").unwrap();
		assert!(
			gap_query(medici).contains("u.created_at > now() - interval '7 days'"),
			"a five-day threshold needs a window that outlasts it"
		);
	}

	#[test]
	fn gap_query_excludes_deleted_and_bounds_the_window() {
		let patients = RESOURCES.iter().find(|r| r.name == "Patient").unwrap();
		let sql = gap_query(patients);
		assert!(sql.contains("u.deleted_at IS NULL"));
		assert!(sql.contains("u.created_at > now() - interval '48 hours'"));
		assert!(sql.contains("r.id IS NULL"));
		// Resolution state is graded elsewhere; presence is the whole test.
		assert!(!sql.contains("resolved"));
	}

	/// Settings rows as key/value pairs, the shape they reach
	/// [`merge_enablement`] in once read off the database.
	fn flags(pairs: &[(&str, Value)]) -> BTreeMap<String, bool> {
		let owned = pairs
			.iter()
			.map(|(key, value)| ((*key).to_string(), value.clone()))
			.collect::<Vec<_>>();
		merge_enablement(&owned)
	}

	#[test]
	fn settings_leaf_rows_resolve_per_resource() {
		let parsed = flags(&[
			(
				"fhir.worker.resourceMaterialisationEnabled.ServiceRequest",
				json!(true),
			),
			(
				"fhir.worker.resourceMaterialisationEnabled.Patient",
				json!(false),
			),
		]);
		assert_eq!(parsed.get("ServiceRequest"), Some(&true));
		assert_eq!(parsed.get("Patient"), Some(&false));
	}

	#[test]
	fn settings_object_row_resolves_per_resource() {
		let parsed = flags(&[(
			"fhir.worker.resourceMaterialisationEnabled",
			json!({ "ServiceRequest": true, "Specimen": false }),
		)]);
		assert_eq!(parsed.get("ServiceRequest"), Some(&true));
		assert_eq!(parsed.get("Specimen"), Some(&false));
	}

	#[test]
	fn settings_union_merge_lets_any_enablement_win() {
		// A facility row and a global row for the same resource come back
		// together; enabling a resource for one facility enables it server-wide.
		let parsed = flags(&[
			(
				"fhir.worker.resourceMaterialisationEnabled.Specimen",
				json!(false),
			),
			(
				"fhir.worker.resourceMaterialisationEnabled.Specimen",
				json!(true),
			),
		]);
		assert_eq!(parsed.get("Specimen"), Some(&true));
	}

	#[test]
	fn settings_ignore_keys_below_the_resource_level() {
		let parsed = flags(&[(
			"fhir.worker.resourceMaterialisationEnabled.Patient.nested",
			json!(true),
		)]);
		assert!(parsed.is_empty());
	}

	#[test]
	fn config_answers_when_no_setting_row_exists() {
		let ctx = facility_ctx();
		let resource = RESOURCES.iter().find(|r| r.name == "Patient").unwrap();
		// Neither source carries it, so enablement is left to be inferred.
		assert_eq!(resolve_enablement(resource, &BTreeMap::new(), &ctx), None);
	}

	#[test]
	fn setting_takes_precedence_over_config() {
		let ctx = config_ctx(&[("ServiceRequest", false)]);
		let resource = RESOURCES
			.iter()
			.find(|r| r.name == "ServiceRequest")
			.unwrap();

		assert_eq!(
			resolve_enablement(resource, &BTreeMap::new(), &ctx),
			Some((false, Source::Config)),
			"config answers on a Tamanu that keeps the flags there"
		);

		let settings = BTreeMap::from([("ServiceRequest".to_string(), true)]);
		assert_eq!(
			resolve_enablement(resource, &settings, &ctx),
			Some((true, Source::Setting)),
			"the stored setting wins once the deployment has one"
		);
	}

	/// A context whose config carries the pre-2.60 per-resource flags. Needs no
	/// database: enablement resolution reads config and the settings map only.
	fn config_ctx(flags: &[(&str, bool)]) -> CheckContext {
		let mut ctx = facility_ctx();
		ctx.config = worker_config(flags);
		ctx
	}
}
