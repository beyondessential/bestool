//! Recent mobile and server sync-session errors, with benign-error exclusions
//! baked into the SQL.
//!
//! The window is a tight `updated_at > now() - interval '1 minute'`; the sweep
//! runs every 60s, so this still catches each error once.
//!
//! That window suits a verdict but not a graph: a scrape reads only the latest
//! sweep, so at munin's five-minute interval four minutes in five would never be
//! seen. The published metric is therefore a running total the daemon
//! accumulates a window at a time, which a scrape derives its own rate from.
//!
//! Errors are reported per instance. Each server device that errored in the
//! window is one instance keyed by its device id, so one failing device is
//! graded and silenced without quieting the rest. Mobile devices are many and
//! come and go, so their errors are one instance between them, keyed `mobile`.
//! A session naming no device is not an instance, though it still counts toward
//! the published total. An instance warns on any error and fails at ten or more
//! in the window.
//!
//! The facility list is read as the `facilityIds` array rather than expanded into
//! a row per facility: a set-returning function in the target list cross-joins,
//! which would count a session spanning several facilities once per facility,
//! and would drop a session whose `parameters` carry no `facilityIds` at all.
//!
//! spec: CHK#instances

use std::sync::atomic::{AtomicU64, Ordering};

use super::TamanuCx;
use crate::Stat;
use crate::check::{Check, Instance};

const NAME: &str = "sync_session_errors";

const FAIL_ERRORS: i64 = 10;

/// Key of the instance holding every mobile device's errors.
const MOBILE_KEY: &str = "mobile";

const MOBILE_FILTER: &str = "parameters->>'isMobile' = 'true' \
	AND errors IS NOT NULL \
	AND errors <> ARRAY['Session marked as completed due to its device reconnecting'] \
	AND errors <> ARRAY['could not serialize access due to concurrent update']";

const SERVER_FILTER: &str = "parameters->>'isMobile' IS DISTINCT FROM 'true' \
	AND errors IS NOT NULL \
	AND errors <> ARRAY['could not serialize access due to concurrent update'] \
	AND NOT (cardinality(errors) = 1 AND errors[1] LIKE '%snapshot-for-pushing%')";

/// One row per group of errored sessions: how many, and the latest of them.
///
/// `key` is the SQL expression sessions are grouped by. A group's facilities are
/// those of its latest session, and a facility with no record is named by its id.
fn query(filter: &str, key: &str) -> String {
	format!(
		"WITH errored AS ( \
			SELECT {key} AS device_id, id, errors, created_at, \
				CASE WHEN jsonb_typeof(parameters->'facilityIds') = 'array' \
					THEN parameters->'facilityIds' ELSE '[]'::jsonb END AS facility_ids \
			FROM sync_sessions \
			WHERE updated_at > now() - interval '1 minute' AND {filter} \
		), grouped AS ( \
			SELECT device_id, count(*)::bigint AS error_count, \
				(array_agg(id::text ORDER BY created_at DESC))[1] AS latest_session, \
				(array_agg(errors::text ORDER BY created_at DESC))[1] AS latest_errors, \
				(array_agg(created_at::text ORDER BY created_at DESC))[1] AS latest_at, \
				(array_agg(facility_ids ORDER BY created_at DESC))[1] AS facility_ids \
			FROM errored GROUP BY device_id \
		) \
		SELECT g.device_id, g.error_count, g.latest_session, g.latest_errors, g.latest_at, \
			ARRAY(SELECT jsonb_array_elements_text(g.facility_ids))::text[] AS facility_ids, \
			ARRAY(SELECT COALESCE(f.name, i.id) \
				FROM jsonb_array_elements_text(g.facility_ids) AS i(id) \
				LEFT JOIN facilities f ON f.id::text = i.id \
				ORDER BY 1)::text[] AS facility_names \
		FROM grouped g ORDER BY g.error_count DESC"
	)
}

fn mobile_sql() -> String {
	query(MOBILE_FILTER, &format!("'{MOBILE_KEY}'::text"))
}

fn server_sql() -> String {
	query(SERVER_FILTER, "parameters->>'deviceId'")
}

/// The errored sessions of one device, or of every mobile device together.
#[derive(Debug, Clone)]
struct Errored {
	/// `None` for sessions that name no device.
	key: Option<String>,
	count: i64,
	latest_session: Option<String>,
	latest_errors: Option<String>,
	latest_at: Option<String>,
	facility_ids: Vec<String>,
	facility_names: Vec<String>,
}

impl Errored {
	fn from_row(row: &tokio_postgres::Row) -> Self {
		Self {
			key: row.try_get("device_id").ok().flatten(),
			count: row.try_get("error_count").unwrap_or(0),
			latest_session: row.try_get("latest_session").ok().flatten(),
			latest_errors: row.try_get("latest_errors").ok().flatten(),
			latest_at: row.try_get("latest_at").ok().flatten(),
			facility_ids: row.try_get("facility_ids").unwrap_or_default(),
			facility_names: row.try_get("facility_names").unwrap_or_default(),
		}
	}

	/// The instance for this group, or `None` for sessions naming no device.
	fn instance(&self) -> Option<Instance> {
		let key = self.key.as_deref()?;
		let reason = format!("{} sync session error(s) in the last minute", self.count);
		let instance = if self.count >= FAIL_ERRORS {
			Instance::fail(key, reason)
		} else {
			Instance::warning(key, reason)
		}
		.with_detail("error_count", self.count)
		.with_detail("latest_session", self.latest_session.clone())
		.with_detail("latest_errors", self.latest_errors.clone())
		.with_detail("latest_at", self.latest_at.clone())
		.with_detail("facility_ids", self.facility_ids.clone());

		Some(if self.facility_names.is_empty() {
			instance
		} else {
			instance.with_label(self.facility_names.join(", "))
		})
	}
}

/// Errors seen since this process started, one stream each.
///
/// Postgres can only be asked what happened in a window; there is no cheap
/// cumulative total to read, since no index covers `errors IS NOT NULL` across
/// the whole of `sync_sessions`. So the daemon does the accumulating: each
/// sweep's window is added to a running total that a scrape derives its own rate
/// from.
static MOBILE_SEEN: AtomicU64 = AtomicU64::new(0);
static SERVER_SEEN: AtomicU64 = AtomicU64::new(0);

/// Add this sweep's count to a running total and read the total back.
fn accumulate(seen: &AtomicU64, found: u64) -> u64 {
	seen.fetch_add(found, Ordering::Relaxed) + found
}

/// Attach the running totals, which stand independent of the verdict: a sweep
/// that finds nothing still reports every error counted before it.
fn with_error_counters(check: Check, mobile_seen: u64, server_seen: u64) -> Check {
	check
		.with_stat(
			Stat::counter("errors_total", mobile_seen as f64)
				.label("stream", "mobile")
				.group("errors")
				.help("Sync-session errors seen"),
		)
		.with_stat(
			Stat::counter("errors_total", server_seen as f64)
				.label("stream", "server")
				.group("errors")
				.help("Sync-session errors seen"),
		)
}

pub async fn run(ctx: TamanuCx) -> Check {
	let Some(client) = ctx.db().await else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let mobile = match client.query(&mobile_sql(), &[]).await {
		Ok(rows) => rows.iter().map(Errored::from_row).collect::<Vec<_>>(),
		Err(err) => return super::query_error_check(NAME, &err),
	};
	let server = match client.query(&server_sql(), &[]).await {
		Ok(rows) => rows.iter().map(Errored::from_row).collect::<Vec<_>>(),
		Err(err) => return super::query_error_check(NAME, &err),
	};

	// Both queries are done; the rest is arithmetic, so hand the connection
	// back rather than holding a slot through it.
	drop(client);

	// Every errored session counts toward the totals, whether or not it names a
	// device to report an instance for.
	let mobile_seen = accumulate(&MOBILE_SEEN, total(&mobile));
	let server_seen = accumulate(&SERVER_SEEN, total(&server));

	with_error_counters(grade(&mobile, &server), mobile_seen, server_seen)
}

fn total(groups: &[Errored]) -> u64 {
	groups.iter().map(|g| g.count.max(0) as u64).sum()
}

/// The check for one window's errored sessions: an instance per server device
/// and one for mobile, none when nothing errored.
fn grade(mobile: &[Errored], server: &[Errored]) -> Check {
	let instances: Vec<Instance> = server
		.iter()
		.chain(mobile)
		.filter_map(Errored::instance)
		.collect();
	if instances.is_empty() {
		return Check::instanced(NAME, "no recent sync session errors", instances);
	}

	let count = |groups: &[Errored]| -> i64 {
		groups
			.iter()
			.filter(|g| g.key.is_some())
			.map(|g| g.count)
			.sum()
	};
	let summary = format!(
		"sync session errors: {} mobile, {} server",
		count(mobile),
		count(server)
	);
	Check::instanced(NAME, summary, instances)
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::AtomicU64;

	use super::{Errored, MOBILE_KEY, accumulate, grade, mobile_sql, server_sql, total};
	use crate::check::CheckStatus;
	use crate::checks::test_support::{central_ctx, facility_ctx};

	#[test]
	fn sessions_are_counted_before_their_facilities_are_expanded() {
		// Expanding facilityIds while sessions are still being counted
		// cross-joins, so a session would be counted once per facility and a
		// session carrying no facilityIds would not appear at all. Expansion is
		// confined to the per-group scalar subqueries after the grouping.
		for sql in [mobile_sql(), server_sql()] {
			let counting = sql.split("grouped AS").next().unwrap();
			assert!(!counting.contains("jsonb_array_elements"), "{counting}");
			assert!(counting.contains("parameters->'facilityIds'"));
		}
	}

	fn errored(key: Option<&str>, count: i64) -> Errored {
		Errored {
			key: key.map(str::to_string),
			count,
			latest_session: Some("s1".into()),
			latest_errors: Some("{boom}".into()),
			latest_at: Some("2026-01-01 00:00:00+00".into()),
			facility_ids: vec!["f1".into()],
			facility_names: vec!["Apia".into()],
		}
	}

	#[test]
	fn each_erroring_server_device_is_an_instance_graded_on_its_own_count() {
		let check = grade(
			&[],
			&[
				errored(Some("dev-a"), 12),
				errored(Some("dev-b"), 1),
				errored(Some("dev-c"), 9),
			],
		);
		let instances = check.instances.as_ref().expect("instanced");
		let status = |key: &str| &instances.iter().find(|i| i.key == key).unwrap().status;

		assert!(matches!(status("dev-a"), CheckStatus::Fail(_)));
		assert!(matches!(status("dev-b"), CheckStatus::Warning(_)));
		// Nine on its own is under the failing line, however many its neighbours
		// add to the total.
		assert!(matches!(status("dev-c"), CheckStatus::Warning(_)));
		assert!(matches!(check.status, CheckStatus::Fail(_)));
		assert_eq!(check.summary, "sync session errors: 0 mobile, 22 server");

		let a = instances.iter().find(|i| i.key == "dev-a").unwrap();
		assert_eq!(a.label.as_deref(), Some("Apia"));
		assert_eq!(a.detail["error_count"], 12);
		assert_eq!(a.detail["latest_session"], "s1");
	}

	#[test]
	fn mobile_errors_are_one_instance_between_every_mobile_device() {
		let check = grade(&[errored(Some(MOBILE_KEY), 3)], &[]);
		let instances = check.instances.as_ref().unwrap();
		assert_eq!(instances.len(), 1);
		assert_eq!(instances[0].key, "mobile");
		assert!(matches!(instances[0].status, CheckStatus::Warning(_)));
	}

	#[test]
	fn sessions_naming_no_device_are_counted_but_are_no_instance() {
		let groups = [errored(None, 4), errored(Some("dev-a"), 1)];
		assert_eq!(total(&groups), 5);

		let check = grade(&[], &groups);
		let instances = check.instances.as_ref().unwrap();
		assert_eq!(instances.len(), 1);
		assert_eq!(instances[0].key, "dev-a");
		assert_eq!(check.summary, "sync session errors: 0 mobile, 1 server");

		let only_unnamed = grade(&[], &[errored(None, 4)]);
		assert!(matches!(only_unnamed.status, CheckStatus::Pass));
	}

	// `runs_against_central` accepts a failed query as a result, so this is what
	// says the SQL is valid against the schema.
	#[tokio::test]
	async fn the_queries_prepare_against_the_schema() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let client = ctx.db().await.expect("central connection");
		for sql in [mobile_sql(), server_sql()] {
			client.prepare(&sql).await.expect("prepare");
		}
	}

	#[test]
	fn a_quiet_window_reports_an_empty_passing_set() {
		let check = grade(&[], &[]);
		assert!(matches!(check.status, CheckStatus::Pass));
		assert_eq!(check.to_wire()["instances"], serde_json::json!({}));
	}

	#[test]
	fn accumulate_sums_successive_windows() {
		let seen = AtomicU64::new(0);
		assert_eq!(accumulate(&seen, 3), 3);
		assert_eq!(accumulate(&seen, 4), 7);
		// a quiet window leaves the total where it was
		assert_eq!(accumulate(&seen, 0), 7);
	}

	#[tokio::test]
	async fn runs_against_central() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, "sync_session_errors");
	}

	#[tokio::test]
	async fn skips_on_facility() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
