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
//! spec: CHK-FMA

use std::collections::{BTreeMap, BTreeSet};

use bestool_tamanu::ApiServerKind;
use serde_json::{Map, Value, json};
use tokio_postgres::{Client as PgClient, error::SqlState};

use super::util::humanise_age;
use super::{CheckContext, query_error_check};
use crate::doctor::Stat;
use crate::doctor::check::Check;

const NAME: &str = "fhir_materialisation";

const WARN_LAG_SECS: i64 = 15 * 60;
const FAIL_LAG_SECS: i64 = 60 * 60;

/// Upstream records older than this are out of scope: a gap that old is a
/// backfill concern rather than an incident, and bounding the measurement keeps
/// it cheap enough to run on every sweep. There is no index on
/// `fhir.*.upstream_id`, so each resource's join scans its FHIR table.
const WINDOW: &str = "48 hours";

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
	upstreams: &'static [Upstream],
}

const RESOURCES: &[Resource] = &[
	Resource {
		name: "ServiceRequest",
		table: "service_requests",
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
		upstreams: &[Upstream {
			table: "patients",
			filter: None,
		}],
	},
	Resource {
		name: "Practitioner",
		table: "practitioners",
		upstreams: &[Upstream {
			table: "users",
			filter: None,
		}],
	},
	Resource {
		name: "Organization",
		table: "organizations",
		upstreams: &[Upstream {
			table: "facilities",
			filter: None,
		}],
	},
	Resource {
		name: "Immunization",
		table: "immunizations",
		upstreams: &[Upstream {
			table: "administered_vaccines",
			filter: None,
		}],
	},
	Resource {
		name: "MedicationRequest",
		table: "medication_requests",
		upstreams: &[Upstream {
			table: "pharmacy_order_prescriptions",
			filter: None,
		}],
	},
	Resource {
		name: "Specimen",
		table: "specimens",
		upstreams: &[Upstream {
			table: "lab_requests",
			filter: Some("u.specimen_attached = true"),
		}],
	},
	Resource {
		name: "Encounter",
		table: "encounters",
		upstreams: &[Upstream {
			table: "encounters",
			filter: Some("u.encounter_type <> 'surveyResponse'"),
		}],
	},
	Resource {
		name: "MediciReport",
		table: "non_fhir_medici_report",
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
	source: Source,
	gap: i64,
	lag_secs: i64,
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
	if ctx.kind != ApiServerKind::Central {
		return Check::skip(
			NAME,
			"not applicable on facility server",
			"central-only check",
		);
	}
	if !ctx.config.fhir_worker_enabled() {
		return Check::skip(
			NAME,
			"FHIR worker not enabled",
			"no upstream record is expected to be materialised, so every resource would read as a total gap",
		);
	}
	let Some(client) = ctx.db.as_ref() else {
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
			None => match has_any_row(client, resource.table).await {
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

		match measure(client, resource).await {
			Ok((gap, lag_secs)) => measured.push(Measured {
				name: resource.name,
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

	let unmonitored = unmonitored(&discovered);

	if measured.is_empty() && errored.is_empty() && unmonitored.is_empty() {
		return Check::skip(
			NAME,
			"no resource has materialisation enabled",
			"nothing is expected to materialise, so there is no gap to measure",
		);
	}

	let worst = measured.iter().max_by_key(|m| m.lag_secs);
	let worst_lag = worst.map_or(0, |m| m.lag_secs);
	let total_gap: i64 = measured.iter().map(|m| m.gap).sum();

	let summary = match worst {
		Some(worst) if worst.gap > 0 => format!(
			"{} unmaterialised, oldest {} ({})",
			total_gap,
			humanise_age(worst.lag_secs),
			worst.name,
		),
		// Nothing measured is not the same as nothing missing, so it must not
		// read as a clean result.
		None if !errored.is_empty() => {
			format!("no resource measured, {} could not be read", errored.len())
		}
		None => "no resource measured".to_string(),
		Some(_) => format!("no materialisation gap across {} resources", measured.len()),
	};

	let mut check = if worst_lag > FAIL_LAG_SECS {
		Check::fail(
			NAME,
			summary,
			format!(
				"upstream record unmaterialised for over {}",
				humanise_age(FAIL_LAG_SECS)
			),
		)
	} else if worst_lag > WARN_LAG_SECS {
		Check::warning(
			NAME,
			summary,
			format!(
				"upstream record unmaterialised for over {}",
				humanise_age(WARN_LAG_SECS)
			),
		)
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
				 AND u.created_at > now() - interval '{WINDOW}'{filter}",
				upstream = upstream.table,
				resource = resource.table,
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
	use crate::doctor::check::CheckStatus;
	use crate::doctor::checks::test_support::{central_ctx, facility_ctx};

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
	/// Runs inside a transaction that is always rolled back, so it leaves the
	/// database as it found it. The client is this test's own connection, so the
	/// open transaction is not visible to anything else.
	#[tokio::test]
	async fn grades_a_seeded_gap_against_central() {
		let Some(ctx) = central_worker_enabled().await else {
			return;
		};
		let client = ctx.db.clone().expect("central_ctx carries a connection");

		client
			.batch_execute(
				"BEGIN; \
				 INSERT INTO settings (key, value) \
				 VALUES ('fhir.worker.resourceMaterialisationEnabled.Patient', 'true'); \
				 INSERT INTO patients \
				 (id, created_at, updated_at, display_id, first_name, last_name, sex) \
				 VALUES ('fhir-materialisation-probe', now() - interval '3 hours', now(), \
				 'FHIRMATPROBE', 'Gap', 'Probe', 'other');",
			)
			.await
			.expect("seeding the gap should succeed");

		let check = super::run(ctx).await;
		let rolled_back = client.batch_execute("ROLLBACK").await;

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

		rolled_back.expect("rollback should succeed");
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
