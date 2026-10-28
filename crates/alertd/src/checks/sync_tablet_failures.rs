//! Tablets that keep failing to complete their syncs.
//!
//! A tablet that loses its connection in the field leaves a sync unfinished,
//! which says little about it. One that fails again and again points to
//! something an operator can act on. The check counts each tablet's failed syncs
//! in the last 24 hours and fails the tablet at five or more, unless it also
//! completed a sync in that time: its records are still reaching central.
//!
//! A sync fails when central closed its session because the tablet went silent,
//! when central closed it because the tablet reconnected and began a new session,
//! or when the session is still open with its tablet silent past central's stale
//! session window. A closed session is dated by when central closed it, a left-open
//! one by when its tablet last connected.
//!
//! A tablet is a mobile device, one instance keyed by its device id, so one known
//! to be flaky can be silenced in Canopy without quieting the rest. Only tablets
//! with a failure in the window are reported: the rest have nothing to grade. A
//! session naming no device is not measured.
//!
//! spec: CHK-STF

use super::sync_sessions::stale_window_seconds;
use super::{TamanuCx, query_error_check, row_error_check};
use crate::Stat;
use crate::check::{Check, Instance};

const NAME: &str = "sync_tablet_failures";

/// Failed syncs in the window at which a tablet fails.
const FAIL_FAILURES: i64 = 5;

const WINDOW_HOURS: i64 = 24;

// The two ways central closes a session whose sync did not finish. Tamanu
// appends its reason to the session's errors, so a session can carry it beside
// others.
const CLOSED_FOR_INACTIVITY: &str = "Session marked as completed due to inactivity";
const CLOSED_FOR_RECONNECT: &str = "Session marked as completed due to its device reconnecting";

// A device's facilities are those of its latest failed session, and a facility
// with no record is named by its id. A left-open session whose device never
// connected after starting is as silent as its start.
const SQL: &str = "WITH sessions AS ( \
		SELECT parameters->>'deviceId' AS device_id, \
			CASE WHEN jsonb_typeof(parameters->'facilityIds') = 'array' \
				THEN parameters->'facilityIds' ELSE '[]'::jsonb END AS facility_ids, \
			COALESCE(completed_at, last_connection_time, start_time) AS ended_at, \
			CASE WHEN completed_at IS NOT NULL \
				THEN COALESCE(errors && ARRAY[$2, $3]::text[], false) \
				ELSE COALESCE(last_connection_time, start_time) \
					< now() - make_interval(secs => $4::float8) END AS failed, \
			completed_at IS NOT NULL AND COALESCE(cardinality(errors), 0) = 0 AS succeeded \
		FROM sync_sessions \
		WHERE parameters->>'isMobile' = 'true' \
			AND parameters->>'deviceId' IS NOT NULL \
			AND ( \
				completed_at > now() - ($1::float8 * interval '1 hour') \
				OR (completed_at IS NULL \
					AND COALESCE(last_connection_time, start_time) \
						> now() - ($1::float8 * interval '1 hour')) \
			) \
	), grouped AS ( \
		SELECT device_id, count(*) FILTER (WHERE failed) AS failed_count, \
			bool_or(succeeded) AS synced, \
			(array_agg(facility_ids ORDER BY ended_at DESC) FILTER (WHERE failed))[1] \
				AS facility_ids \
		FROM sessions GROUP BY device_id \
		HAVING bool_or(failed) \
	) \
	SELECT g.device_id, g.failed_count, g.synced, \
		ARRAY(SELECT jsonb_array_elements_text(g.facility_ids))::text[] AS facility_ids, \
		ARRAY(SELECT COALESCE(f.name, i.id) \
			FROM jsonb_array_elements_text(g.facility_ids) AS i(id) \
			LEFT JOIN facilities f ON f.id::text = i.id \
			ORDER BY 1)::text[] AS facility_names \
	FROM grouped g ORDER BY g.failed_count DESC";

struct Tablet {
	id: String,
	failed: i64,
	synced: bool,
	facility_ids: Vec<String>,
	facility_names: Vec<String>,
}

impl Tablet {
	fn from_row(row: &tokio_postgres::Row) -> Result<Self, tokio_postgres::Error> {
		Ok(Self {
			id: row.try_get("device_id")?,
			failed: row.try_get("failed_count")?,
			synced: row.try_get("synced")?,
			facility_ids: row.try_get("facility_ids")?,
			facility_names: row.try_get("facility_names")?,
		})
	}

	fn is_failing(&self) -> bool {
		self.failed >= FAIL_FAILURES && !self.synced
	}

	fn instance(&self) -> Instance {
		let instance = if self.is_failing() {
			Instance::fail(
				&self.id,
				format!(
					"{} failed syncs and none completed in the last {WINDOW_HOURS} hours",
					self.failed
				),
			)
		} else {
			Instance::pass(&self.id)
		}
		.with_detail("failed_count", self.failed)
		.with_detail("synced", self.synced)
		.with_detail("facility_ids", self.facility_ids.clone());

		if self.facility_names.is_empty() {
			instance
		} else {
			instance.with_label(self.facility_names.join(", "))
		}
	}
}

fn report(tablets: &[Tablet]) -> Check {
	let fail_n = tablets.iter().filter(|t| t.is_failing()).count();

	let summary = if fail_n == 0 {
		"no tablet failing its syncs".to_string()
	} else {
		format!(
			"{fail_n} tablet(s) with {FAIL_FAILURES} or more failed syncs and none completed in {WINDOW_HOURS}h"
		)
	};

	Check::instanced(
		NAME,
		summary,
		tablets.iter().map(Tablet::instance).collect(),
	)
	.with_detail("fail_failures", FAIL_FAILURES)
	.with_detail("window_hours", WINDOW_HOURS)
	.with_stat(Stat::gauge("fail", fail_n as f64).help(format!(
		"Tablets with {FAIL_FAILURES} or more failed syncs and none completed in {WINDOW_HOURS} hours"
	)))
}

pub async fn run(ctx: TamanuCx) -> Check {
	let Some(client) = ctx.db().await else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let window_hours = WINDOW_HOURS as f64;
	let stale_window = stale_window_seconds(&ctx) as f64;
	let params: [&(dyn tokio_postgres::types::ToSql + Sync); 4] = [
		&window_hours,
		&CLOSED_FOR_INACTIVITY,
		&CLOSED_FOR_RECONNECT,
		&stale_window,
	];
	let rows = match client.query(SQL, &params).await {
		Ok(rows) => rows,
		Err(err) => {
			if let Some(db) = err.as_db_error()
				&& db.code() == &tokio_postgres::error::SqlState::UNDEFINED_TABLE
			{
				return Check::skip(NAME, "sync_sessions table not present", "table absent");
			}
			return query_error_check(NAME, &err);
		}
	};

	let tablets = match rows
		.iter()
		.map(Tablet::from_row)
		.collect::<Result<Vec<_>, _>>()
	{
		Ok(tablets) => tablets,
		Err(err) => return row_error_check(NAME, &err),
	};

	report(&tablets)
}

#[cfg(test)]
mod tests {
	use super::{Tablet, report};
	use crate::check::CheckStatus;
	use crate::checks::test_support::{central_ctx, facility_ctx};

	fn tablet(id: &str, name: &str, failed: i64) -> Tablet {
		Tablet {
			id: id.into(),
			failed,
			synced: false,
			facility_ids: vec![format!("id-{name}")],
			facility_names: vec![name.into()],
		}
	}

	#[test]
	fn a_tablet_fails_at_five_failed_syncs_and_passes_below() {
		let check = report(&[
			tablet("tab-a", "Apia", 5),
			tablet("tab-b", "Savaii", 4),
			tablet("tab-c", "Tonga", 1),
		]);
		let instances = check.instances.as_ref().expect("instanced");
		let status = |key: &str| &instances.iter().find(|i| i.key == key).unwrap().status;

		assert!(matches!(status("tab-a"), CheckStatus::Fail(_)));
		assert!(matches!(status("tab-b"), CheckStatus::Pass));
		assert!(matches!(status("tab-c"), CheckStatus::Pass));
		assert!(matches!(check.status, CheckStatus::Fail(_)));
	}

	#[test]
	fn a_tablet_that_also_completed_a_sync_passes() {
		let mut t = tablet("tab-a", "Apia", 12);
		t.synced = true;
		let check = report(&[t]);
		assert!(matches!(check.status, CheckStatus::Pass));
		assert_eq!(check.summary, "no tablet failing its syncs");
	}

	#[test]
	fn an_instance_carries_its_label_and_detail() {
		let check = report(&[tablet("tab-a", "Apia", 7)]);
		let a = &check.instances.as_ref().unwrap()[0];
		assert_eq!(a.label.as_deref(), Some("Apia"));
		assert_eq!(a.detail["failed_count"], 7);
		assert_eq!(a.detail["synced"], false);
		assert_eq!(a.detail["facility_ids"], serde_json::json!(["id-Apia"]));
		assert_eq!(check.details["fail_failures"], 5);
		assert_eq!(check.details["window_hours"], 24);
	}

	#[test]
	fn tablets_below_the_threshold_leave_the_check_passing() {
		let check = report(&[tablet("tab-a", "Apia", 4)]);
		assert!(matches!(check.status, CheckStatus::Pass));
		assert_eq!(check.summary, "no tablet failing its syncs");
	}

	#[test]
	fn no_failed_tablet_sync_reports_an_empty_set() {
		let check = report(&[]);
		assert!(matches!(check.status, CheckStatus::Pass));
		assert_eq!(check.instances.as_ref().map(Vec::len), Some(0));
		assert_eq!(check.to_wire()["instances"], serde_json::json!({}));
	}

	#[test]
	fn a_tablet_with_no_named_facility_has_no_label() {
		let mut t = tablet("tab-a", "Apia", 1);
		t.facility_names.clear();
		let check = report(&[t]);
		assert_eq!(check.instances.unwrap()[0].label, None);
	}

	// `runs_against_central` accepts a failed query as a result, so this is what
	// says the SQL is valid against the schema.
	#[tokio::test]
	async fn the_query_prepares_against_the_schema() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let client = ctx.db().await.expect("central connection");
		client.prepare(super::SQL).await.expect("prepare");
	}

	// Temp tables shadow the real ones for this connection only, so the query
	// runs over seeded sessions without touching the database's own.
	#[tokio::test]
	async fn counts_only_a_tablets_failed_syncs_inside_the_window() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let client = ctx.db().await.expect("central connection");
		client
			.batch_execute(
				"CREATE TEMP TABLE facilities (id text, name text); \
				INSERT INTO facilities VALUES ('f1', 'Apia'); \
				CREATE TEMP TABLE sync_sessions (start_time timestamptz, \
					last_connection_time timestamptz, completed_at timestamptz, \
					errors text[], parameters jsonb); \
				INSERT INTO sync_sessions VALUES \
				(now() - '5h'::interval, now() - '4h'::interval, now() - '3h'::interval, \
					ARRAY['Session marked as completed due to inactivity'], \
					'{\"deviceId\":\"A\",\"isMobile\":\"true\",\"facilityIds\":[\"f1\"]}'), \
				(now() - '6h'::interval, now() - '6h'::interval, now() - '5h'::interval, \
					ARRAY['x','Session marked as completed due to its device reconnecting'], \
					'{\"deviceId\":\"A\",\"isMobile\":\"true\",\"facilityIds\":[\"f1\"]}'), \
				(now() - '4h'::interval, now() - '3h'::interval, NULL, NULL, \
					'{\"deviceId\":\"A\",\"isMobile\":\"true\",\"facilityIds\":[\"f1\"]}'), \
				(now() - '30h'::interval, now() - '29h'::interval, now() - '28h'::interval, \
					ARRAY['Session marked as completed due to inactivity'], \
					'{\"deviceId\":\"A\",\"isMobile\":\"true\"}'), \
				(now() - '1h'::interval, now() - '10 minutes'::interval, NULL, NULL, \
					'{\"deviceId\":\"B\",\"isMobile\":\"true\"}'), \
				(now() - '1h'::interval, now() - '30 minutes'::interval, now() - '20 minutes'::interval, \
					NULL, '{\"deviceId\":\"B\",\"isMobile\":\"true\"}'), \
				(now() - '5h'::interval, now() - '4h'::interval, now() - '3h'::interval, \
					ARRAY['Session marked as completed due to inactivity'], \
					'{\"deviceId\":\"S\",\"facilityIds\":[\"f1\"]}'), \
				(now() - '5h'::interval, now() - '4h'::interval, now() - '3h'::interval, \
					ARRAY['Session marked as completed due to inactivity'], '{\"isMobile\":\"true\"}'), \
				(now() - '5h'::interval, now() - '4h'::interval, now() - '3h'::interval, \
					ARRAY['Session marked as completed due to inactivity'], \
					'{\"deviceId\":\"C\",\"isMobile\":\"true\"}'), \
				(now() - '2h'::interval, now() - '2h'::interval, now() - '2h'::interval, \
					'{}', '{\"deviceId\":\"C\",\"isMobile\":\"true\"}'), \
				(now() - '6h'::interval, now() - '6h'::interval, now() - '6h'::interval, \
					ARRAY['could not serialize access due to concurrent update'], \
					'{\"deviceId\":\"A\",\"isMobile\":\"true\"}')",
			)
			.await
			.expect("seed");

		let window_hours = super::WINDOW_HOURS as f64;
		let stale_window = 3600.0_f64;
		let rows = client
			.query(
				super::SQL,
				&[
					&window_hours,
					&super::CLOSED_FOR_INACTIVITY,
					&super::CLOSED_FOR_RECONNECT,
					&stale_window,
				],
			)
			.await
			.expect("query");
		let tablets: Vec<Tablet> = rows.iter().map(|r| Tablet::from_row(r).unwrap()).collect();

		assert_eq!(tablets.len(), 2, "only tablets A and C have a failed sync");
		assert_eq!(tablets[0].id, "A");
		assert_eq!(tablets[0].failed, 3);
		assert!(!tablets[0].synced, "a session with errors is not a success");
		assert_eq!(tablets[0].facility_names, vec!["Apia"]);
		assert_eq!(tablets[1].id, "C");
		assert!(tablets[1].synced);
	}

	#[tokio::test]
	async fn runs_against_central() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, "sync_tablet_failures");
		assert!(matches!(
			check.status,
			CheckStatus::Pass | CheckStatus::Fail(_)
		));
	}

	#[tokio::test]
	async fn skips_without_a_database() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
