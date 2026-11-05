//! Blob content that no longer matches its hash, or whose bytes are gone.
//!
//! A blob is named by the hash of its content, so any copy can be checked by
//! re-hashing it: `corrupt` is bytes that no longer match, `absent` is a
//! registry entry whose bytes the store does not hold. Both are retained and
//! never served.
//!
//! How urgent a fault is depends on whether the copy was the only durable one.
//! Central's copies are all authoritative and a facility's `outbox` blob has not
//! been acknowledged by central, so a fault in either may be data loss and FAILs
//! on the first one. A `cache` blob is durable on central and refetches on
//! demand, so it WARNs until enough have gone at once to read as the storage
//! failing rather than one bad sector.
//!
//! A store nothing verifies looks healthy right up until someone needs a file,
//! so a scrub that has stamped nothing for hours is a WARN of its own.
//!
//! Dropping a faulty facility cache copy is what makes it self-correcting, and it
//! takes the registry row with it, so those faults are counted in
//! `local_system_facts` rather than left in `blobs`. The counter never resets, so
//! it says how many and how recently, never how many in a window: the gauge is
//! what carries the trend, and the verdict below is a coarse backstop over it.

use tokio_postgres::error::SqlState;

use bestool_tamanu::ApiServerKind;

use super::util::humanise_age;
use super::{TamanuCx, query_error_check};
use crate::Stat;
use crate::check::Check;

const NAME: &str = "blob_integrity";

/// Faulty cache copies that read as the storage under the store failing rather
/// than a single bad sector. The runbook draws the line at "one blob or many"
/// without a number; ten is low enough to catch a failing disk early and high
/// enough that a handful of unlucky sectors stays a warning.
const MANY_AT_ONCE: i64 = 10;

/// How long the store may go unverified before the scrub reads as stopped. It
/// runs hourly on central and on facilities, so this is six missed passes.
const STALE_SCRUB_SECS: i64 = 6 * 60 * 60;

/// How recently a cache drop must have happened to read as still going on. The
/// scrub covers the store over many hourly passes, so a day is wide enough that
/// a genuinely failing disk does not fall between two sweeps of this check.
const RECENT_DROP_SECS: i64 = 24 * 60 * 60;

/// Read separately from the blob registry: a dropped blob's row is gone, and a
/// scrub pass that verified nothing stamps no row. Each column is null where its
/// fact has never been written, including on any Tamanu predating it.
///
/// `blobCacheFaultAt` is zoneless server-local time, so ages come from `updated_at`.
const FACTS_SQL: &str = "\
	SELECT \
	(SELECT value::bigint FROM local_system_facts \
	 WHERE key = 'blobCacheFaults' AND deleted_at IS NULL) AS dropped, \
	(SELECT extract(epoch FROM now() - updated_at)::bigint FROM local_system_facts \
	 WHERE key = 'blobCacheFaults' AND deleted_at IS NULL) AS dropped_since, \
	(SELECT extract(epoch FROM now() - updated_at)::bigint FROM local_system_facts \
	 WHERE key = 'blobScrubCompletedAt' AND deleted_at IS NULL) AS scrub_completed_since";

const SQL: &str = "\
	SELECT count(*) AS blobs, \
	count(*) FILTER (WHERE integrity_state = 'corrupt') AS corrupt, \
	count(*) FILTER (WHERE integrity_state = 'absent') AS absent, \
	count(*) FILTER (WHERE integrity_state IN ('corrupt', 'absent') AND tier = 'outbox') AS outbox_faulty, \
	count(*) FILTER (WHERE integrity_state IN ('corrupt', 'absent') AND tier = 'cache') AS cache_faulty, \
	count(*) FILTER (WHERE last_scrubbed_at IS NULL) AS never_scrubbed, \
	extract(epoch FROM now() - coalesce(max(last_scrubbed_at), min(created_at)))::bigint AS scrub_idle_seconds \
	FROM blobs WHERE deleted_at IS NULL";

pub async fn run(ctx: TamanuCx) -> Check {
	let Some(client) = ctx.db().await else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let row = match client.query_one(SQL, &[]).await {
		Ok(row) => row,
		Err(err) => {
			if let Some(db) = err.as_db_error()
				&& matches!(
					db.code(),
					&SqlState::UNDEFINED_TABLE | &SqlState::UNDEFINED_COLUMN
				) {
				return Check::skip(
					NAME,
					"no blob store on this Tamanu",
					"the blob registry is not in this deployment's schema",
				);
			}
			return query_error_check(NAME, &err);
		}
	};

	let blobs: i64 = row.try_get("blobs").unwrap_or(0);
	let corrupt: i64 = row.try_get("corrupt").unwrap_or(0);
	let absent: i64 = row.try_get("absent").unwrap_or(0);
	let outbox_faulty: i64 = row.try_get("outbox_faulty").unwrap_or(0);
	let cache_faulty: i64 = row.try_get("cache_faulty").unwrap_or(0);
	let never_scrubbed: i64 = row.try_get("never_scrubbed").unwrap_or(0);
	let registry_idle_secs: Option<i64> = row.try_get("scrub_idle_seconds").unwrap_or(None);

	let (durable_faulty, replica_faulty) = split_faults(
		ctx.server_kind(),
		corrupt,
		absent,
		outbox_faulty,
		cache_faulty,
	);

	// A failure here is not worth losing the registry verdict over: the facts are
	// a supplement to it, and their absence is the normal state.
	let facts = client.query_one(FACTS_SQL, &[]).await.ok();
	let fact =
		|name: &str| -> Option<i64> { facts.as_ref().and_then(|r| r.try_get(name).ok()).flatten() };
	let dropped = fact("dropped");
	let dropped_since = fact("dropped_since");
	let scrub_idle_secs = scrub_idle(fact("scrub_completed_since"), registry_idle_secs);

	let summary = if blobs == 0 {
		"blob store empty".to_string()
	} else if corrupt + absent == 0 {
		format!("{blobs} blobs, all verified")
	} else {
		format!("{blobs} blobs: {corrupt} corrupt, {absent} absent")
	};

	let check = match classify(
		durable_faulty,
		replica_faulty,
		blobs,
		scrub_idle_secs,
		dropped,
		dropped_since,
	) {
		Verdict::Pass => Check::pass(NAME, summary),
		Verdict::Warn(reason) => Check::warning(NAME, summary, reason),
		Verdict::Fail(reason) => Check::fail(NAME, summary, reason),
	};

	let mut check = check
		.with_detail("blobs", blobs)
		.with_detail("corrupt", corrupt)
		.with_detail("absent", absent)
		.with_detail("durable_faulty", durable_faulty)
		.with_detail("replica_faulty", replica_faulty)
		.with_detail("never_scrubbed", never_scrubbed)
		.with_stat(Stat::gauge("blobs", blobs as f64).help("Blobs in this server's registry"))
		.with_stat(
			Stat::gauge("corrupt", corrupt as f64)
				.group("faults")
				.help("Blobs whose stored bytes no longer match their hash"),
		)
		.with_stat(
			Stat::gauge("absent", absent as f64)
				.group("faults")
				.help("Blobs whose content is missing from the store"),
		)
		.with_stat(
			Stat::gauge("never_scrubbed", never_scrubbed as f64)
				.help("Blobs the scrub has not yet verified once"),
		);
	if let Some(total) = dropped {
		check = check.with_detail("cache_blobs_dropped", total).with_stat(
			Stat::gauge("cache_blobs_dropped", total as f64)
				.group("faults")
				.help(
					"Cache blobs dropped for failing verification, lifetime; each refetches on demand",
				),
		);
	}
	if let Some(since) = dropped_since {
		check = check.with_detail("cache_drop_age_seconds", since);
	}
	if let Some(idle) = scrub_idle_secs {
		check = check.with_detail("scrub_idle_seconds", idle).with_stat(
			Stat::gauge("scrub_idle_seconds", idle as f64).help(
				"Seconds since the last completed scrub pass, or on older Tamanu since the scrub last stamped a blob",
			),
		);
	}
	check
}

enum Verdict {
	Pass,
	Warn(String),
	Fail(String),
}

/// Split the faults into copies that must be durably present on this server and
/// copies central still holds, which is what decides how urgent they are.
///
/// Central does not consult the tier: every copy it holds is authoritative, and
/// a row there carries the default tier whatever it is.
fn split_faults(
	kind: ApiServerKind,
	corrupt: i64,
	absent: i64,
	outbox_faulty: i64,
	cache_faulty: i64,
) -> (i64, i64) {
	if kind == ApiServerKind::Central {
		(corrupt + absent, 0)
	} else {
		(outbox_faulty, cache_faulty)
	}
}

/// The age of the last completed scrub pass, where Tamanu records one.
///
/// The registry's newest stamp is only a fallback: admitting a blob stamps it too,
/// so a store that keeps taking uploads never reads as idle by it. It falls back
/// in turn to the age of the oldest blob where nothing has been stamped at all, so
/// a store filled minutes ago does not read as unscrubbed before its first pass.
fn scrub_idle(pass_completed_secs: Option<i64>, registry_idle_secs: Option<i64>) -> Option<i64> {
	pass_completed_secs.or(registry_idle_secs)
}

/// Grade the store on what it has lost and whether anything is still checking.
///
/// `durable_faulty` counts copies that must be durably present on this server,
/// so one is enough to escalate; `replica_faulty` counts copies central still
/// holds, which refetch on demand.
///
/// `dropped` is the facility's lifetime count of cache copies dropped for failing
/// verification and `dropped_since` how long ago the last one went. Both are
/// needed: the count alone would warn forever about a bad sector from a year ago,
/// and recency alone would warn for a day about a single one.
fn classify(
	durable_faulty: i64,
	replica_faulty: i64,
	blobs: i64,
	scrub_idle_secs: Option<i64>,
	dropped: Option<i64>,
	dropped_since: Option<i64>,
) -> Verdict {
	let dropping = dropped.unwrap_or(0) >= MANY_AT_ONCE
		&& dropped_since.is_some_and(|secs| secs <= RECENT_DROP_SECS);

	if durable_faulty > 0 {
		Verdict::Fail(format!(
			"{durable_faulty} blob(s) that must be durably present here are corrupt or absent"
		))
	} else if replica_faulty >= MANY_AT_ONCE {
		Verdict::Fail(format!(
			"{replica_faulty} cache blobs faulty at once, which reads as the storage failing rather than one bad write"
		))
	} else if replica_faulty > 0 {
		Verdict::Warn(format!(
			"{replica_faulty} faulty cache blob(s), which should clear by refetching from central"
		))
	} else if dropping {
		let total = dropped.unwrap_or(0);
		Verdict::Warn(format!(
			"{total} cache blobs dropped for failing verification, the last one recently; each refetches on its next read, but a run of them reads as the storage failing"
		))
	} else if blobs > 0 && scrub_idle_secs.is_some_and(|secs| secs > STALE_SCRUB_SECS) {
		let idle = humanise_age(scrub_idle_secs.unwrap_or(0));
		Verdict::Warn(format!(
			"the scrub has verified nothing for {idle}, so corruption would go unnoticed"
		))
	} else {
		Verdict::Pass
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::check::CheckStatus;
	use crate::checks::test_support::{BLOB_STORE, ScratchDb, facility_ctx, scratch_db};

	fn verdict(durable: i64, replica: i64) -> &'static str {
		grade(durable, replica, 100, Some(0))
	}

	fn grade(durable: i64, replica: i64, blobs: i64, idle: Option<i64>) -> &'static str {
		grade_drops(durable, replica, blobs, idle, None, None)
	}

	fn grade_drops(
		durable: i64,
		replica: i64,
		blobs: i64,
		idle: Option<i64>,
		dropped: Option<i64>,
		dropped_since: Option<i64>,
	) -> &'static str {
		match classify(durable, replica, blobs, idle, dropped, dropped_since) {
			Verdict::Pass => "pass",
			Verdict::Warn(_) => "warn",
			Verdict::Fail(_) => "fail",
		}
	}

	#[test]
	fn a_verified_store_passes() {
		assert_eq!(verdict(0, 0), "pass");
	}

	#[test]
	fn one_durable_fault_fails() {
		assert_eq!(verdict(1, 0), "fail");
	}

	#[test]
	fn a_faulty_replica_warns() {
		assert_eq!(verdict(0, 1), "warn");
		assert_eq!(verdict(0, 9), "warn");
	}

	#[test]
	fn many_faulty_replicas_at_once_fail() {
		assert_eq!(verdict(0, 10), "fail");
	}

	#[test]
	fn a_run_of_recent_cache_drops_warns() {
		assert_eq!(grade_drops(0, 0, 100, Some(0), Some(10), Some(60)), "warn");
	}

	#[test]
	fn cache_drops_need_both_a_run_and_recency() {
		// One bad sector the refetch already corrected.
		assert_eq!(grade_drops(0, 0, 100, Some(0), Some(1), Some(60)), "pass");
		// A run, but nothing since; the disk was replaced or the run was one-off.
		assert_eq!(
			grade_drops(0, 0, 100, Some(0), Some(40), Some(RECENT_DROP_SECS + 1)),
			"pass"
		);
	}

	#[test]
	fn nine_recent_cache_drops_pass() {
		assert_eq!(grade_drops(0, 0, 100, Some(0), Some(9), Some(60)), "pass");
	}

	#[test]
	fn a_drop_exactly_a_day_old_still_counts_as_recent() {
		assert_eq!(
			grade_drops(0, 0, 100, Some(0), Some(10), Some(RECENT_DROP_SECS)),
			"warn"
		);
	}

	#[test]
	fn a_drop_count_without_recency_passes() {
		assert_eq!(grade_drops(0, 0, 100, Some(0), Some(50), None), "pass");
	}

	fn reason(verdict: Verdict) -> String {
		match verdict {
			Verdict::Pass => panic!("expected a reason, got a pass"),
			Verdict::Warn(r) | Verdict::Fail(r) => r,
		}
	}

	#[test]
	fn a_faulty_cache_blob_is_reported_over_a_drop_run() {
		assert_eq!(
			reason(classify(0, 1, 100, Some(0), Some(50), Some(60))),
			"1 faulty cache blob(s), which should clear by refetching from central"
		);
	}

	#[test]
	fn a_faulty_cache_blob_is_reported_over_a_stale_scrub() {
		assert_eq!(
			reason(classify(0, 1, 100, Some(STALE_SCRUB_SECS + 1), None, None)),
			"1 faulty cache blob(s), which should clear by refetching from central"
		);
	}

	#[test]
	fn a_store_that_has_never_dropped_one_passes() {
		assert_eq!(grade_drops(0, 0, 100, Some(0), None, None), "pass");
	}

	#[test]
	fn a_durable_fault_outranks_a_cache_drop_run() {
		assert_eq!(grade_drops(1, 0, 100, Some(0), Some(50), Some(60)), "fail");
	}

	#[test]
	fn a_durable_fault_outranks_a_replica_one() {
		assert_eq!(verdict(1, 1), "fail");
	}

	#[test]
	fn a_stalled_scrub_warns() {
		assert_eq!(grade(0, 0, 100, Some(STALE_SCRUB_SECS + 1)), "warn");
		assert_eq!(grade(0, 0, 100, Some(STALE_SCRUB_SECS)), "pass");
	}

	#[test]
	fn a_completed_pass_outranks_the_registry_stamps() {
		assert_eq!(scrub_idle(Some(60), Some(30 * 24 * 60 * 60)), Some(60));
		assert_eq!(scrub_idle(Some(7 * 60 * 60), Some(0)), Some(7 * 60 * 60));
	}

	#[test]
	fn without_a_pass_record_the_registry_stamps_decide() {
		assert_eq!(scrub_idle(None, Some(90)), Some(90));
		assert_eq!(scrub_idle(None, None), None);
	}

	#[test]
	fn an_empty_store_never_reads_as_unscrubbed() {
		assert_eq!(grade(0, 0, 0, None), "pass");
		assert_eq!(grade(0, 0, 0, Some(STALE_SCRUB_SECS * 10)), "pass");
	}

	#[test]
	fn central_holds_every_copy_it_has() {
		assert_eq!(
			split_faults(ApiServerKind::Central, 2, 1, 0, 3),
			(3, 0),
			"the tier a central row carries says nothing about its durability"
		);
	}

	#[test]
	fn a_facility_is_split_by_tier() {
		assert_eq!(split_faults(ApiServerKind::Facility, 4, 1, 2, 3), (2, 3));
	}

	#[tokio::test]
	async fn runs_against_central() {
		let Some(db) = scratch_db(BLOB_STORE).await else {
			return;
		};
		let check = super::run(db.central.clone()).await;
		assert_eq!(check.name, "blob_integrity");
		assert!(
			!matches!(check.status, CheckStatus::Broken(_)),
			"a Tamanu without a blob store should skip, not break: {:?}",
			check.to_wire()["result"]
		);
	}

	#[tokio::test]
	async fn skips_without_a_blob_store() {
		let Some(db) = scratch_db("").await else {
			return;
		};
		let check = super::run(db.central.clone()).await;
		assert!(check.status.is_skip(), "{:?}", check.status);
		assert_eq!(check.summary, "no blob store on this Tamanu");
	}

	#[tokio::test]
	async fn grades_a_seeded_corrupt_blob_against_central() {
		let Some(db) = scratch_db(BLOB_STORE).await else {
			return;
		};
		db.central
			.db()
			.await
			.expect("a scratch database carries a connection")
			.batch_execute(
				"INSERT INTO blobs (hash, size, integrity_state) \
				 VALUES ('sha256:0000000000000000000000000000000000000000000000000000000000000001', \
				 4096, 'corrupt');",
			)
			.await
			.expect("seeding a corrupt blob should succeed");

		let check = super::run(db.central.clone()).await;
		assert!(
			matches!(check.status, CheckStatus::Fail(_)),
			"an authoritative corrupt copy should fail: {:?} — {}",
			check.status,
			check.summary
		);
		assert!(
			check.details["corrupt"].as_i64().unwrap_or(0) >= 1,
			"the seeded blob should be counted: {:?}",
			check.details
		);
	}

	#[tokio::test]
	async fn skips_without_a_database() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}

	async fn seeded(sql: &str) -> Option<ScratchDb> {
		let db = scratch_db(BLOB_STORE).await?;
		db.central
			.db()
			.await
			.expect("a scratch database carries a connection")
			.batch_execute(sql)
			.await
			.expect("seeding should succeed");
		Some(db)
	}

	fn assert_status(check: &Check, status: &str, reason: Option<&str>) {
		assert_eq!(
			check.status.wire_result(),
			status,
			"{:?}: {}",
			check.status,
			check.summary
		);
		assert_eq!(check.status.reason(), reason, "{}", check.summary);
	}

	const DURABLE_ONE: &str = "1 blob(s) that must be durably present here are corrupt or absent";
	const CACHE_ONE: &str = "1 faulty cache blob(s), which should clear by refetching from central";
	const DROP_RUN_12: &str = "12 cache blobs dropped for failing verification, the last one recently; each refetches on its next read, but a run of them reads as the storage failing";

	#[tokio::test]
	async fn an_absent_blob_on_central_fails() {
		let Some(db) = seeded(
			"INSERT INTO blobs (hash, size, integrity_state) VALUES ('sha256:a', 1, 'absent');",
		)
		.await
		else {
			return;
		};
		let check = super::run(db.central.clone()).await;
		assert_status(&check, "failed", Some(DURABLE_ONE));
		assert_eq!(check.summary, "1 blobs: 0 corrupt, 1 absent");
		assert_eq!(check.details["absent"], 1);
		assert_eq!(check.details["durable_faulty"], 1);
	}

	#[tokio::test]
	async fn a_corrupt_outbox_blob_on_a_facility_fails() {
		let Some(db) = seeded(
			"INSERT INTO blobs (hash, size, integrity_state, tier) \
			 VALUES ('sha256:a', 1, 'corrupt', 'outbox');",
		)
		.await
		else {
			return;
		};
		let check = super::run(db.facility.clone()).await;
		assert_status(&check, "failed", Some(DURABLE_ONE));
		assert_eq!(check.summary, "1 blobs: 1 corrupt, 0 absent");
		assert_eq!(check.details["durable_faulty"], 1);
		assert_eq!(check.details["replica_faulty"], 0);
	}

	#[tokio::test]
	async fn an_absent_outbox_blob_on_a_facility_fails() {
		let Some(db) = seeded(
			"INSERT INTO blobs (hash, size, integrity_state, tier) \
			 VALUES ('sha256:a', 1, 'absent', 'outbox');",
		)
		.await
		else {
			return;
		};
		let check = super::run(db.facility.clone()).await;
		assert_status(&check, "failed", Some(DURABLE_ONE));
		assert_eq!(check.summary, "1 blobs: 0 corrupt, 1 absent");
		assert_eq!(check.details["durable_faulty"], 1);
	}

	#[tokio::test]
	async fn one_corrupt_cache_blob_on_a_facility_warns() {
		let Some(db) = seeded(
			"INSERT INTO blobs (hash, size, integrity_state, tier) \
			 VALUES ('sha256:a', 1, 'corrupt', 'cache');",
		)
		.await
		else {
			return;
		};
		let check = super::run(db.facility.clone()).await;
		assert_status(&check, "warning", Some(CACHE_ONE));
		assert_eq!(check.details["durable_faulty"], 0);
		assert_eq!(check.details["replica_faulty"], 1);
	}

	#[tokio::test]
	async fn ten_corrupt_cache_blobs_on_a_facility_fail() {
		let Some(db) = seeded(
			"INSERT INTO blobs (hash, size, integrity_state, tier) \
			 SELECT 'sha256:' || g, 1, 'corrupt', 'cache' FROM generate_series(1, 10) g;",
		)
		.await
		else {
			return;
		};
		let check = super::run(db.facility.clone()).await;
		assert_status(
			&check,
			"failed",
			Some(
				"10 cache blobs faulty at once, which reads as the storage failing rather than one bad write",
			),
		);
		assert_eq!(check.summary, "10 blobs: 10 corrupt, 0 absent");
	}

	#[tokio::test]
	async fn soft_deleted_faulty_blobs_are_ignored() {
		let Some(db) = seeded(
			"INSERT INTO blobs (hash, size, integrity_state, tier, deleted_at) VALUES \
			 ('sha256:a', 1, 'corrupt', 'outbox', now()), \
			 ('sha256:b', 1, 'absent', 'cache', now()); \
			 INSERT INTO blobs (hash, size) VALUES ('sha256:c', 1);",
		)
		.await
		else {
			return;
		};
		let check = super::run(db.facility.clone()).await;
		assert_status(&check, "passed", None);
		assert_eq!(check.summary, "1 blobs, all verified");
	}

	#[tokio::test]
	async fn a_recent_run_of_cache_drops_warns() {
		let Some(db) = seeded(
			"INSERT INTO local_system_facts (key, value, updated_at) \
			 VALUES ('blobCacheFaults', '12', now() - interval '1 hour');",
		)
		.await
		else {
			return;
		};
		let check = super::run(db.facility.clone()).await;
		assert_status(&check, "warning", Some(DROP_RUN_12));
		assert_eq!(check.details["cache_blobs_dropped"], 12);
		let age = check.details["cache_drop_age_seconds"].as_i64().unwrap();
		assert!((3590..=3700).contains(&age), "{age}");
	}

	#[tokio::test]
	async fn an_old_run_of_cache_drops_passes() {
		let Some(db) = seeded(
			"INSERT INTO local_system_facts (key, value, updated_at) \
			 VALUES ('blobCacheFaults', '12', now() - interval '2 days');",
		)
		.await
		else {
			return;
		};
		let check = super::run(db.facility.clone()).await;
		assert_status(&check, "passed", None);
		assert_eq!(check.details["cache_blobs_dropped"], 12);
	}

	#[tokio::test]
	async fn drop_recency_comes_from_the_counter_row_not_the_fault_time() {
		let Some(db) = seeded(
			"INSERT INTO local_system_facts (key, value, updated_at) VALUES \
			 ('blobCacheFaults', '12', now() - interval '1 hour'), \
			 ('blobCacheFaultAt', (now() - interval '3 days')::timestamp::text, now());",
		)
		.await
		else {
			return;
		};
		let check = super::run(db.facility.clone()).await;
		assert_status(&check, "warning", Some(DROP_RUN_12));
	}

	#[tokio::test]
	async fn a_stale_scrub_pass_warns_despite_fresh_blob_stamps() {
		let Some(db) = seeded(
			"INSERT INTO local_system_facts (key, value, updated_at) \
			 VALUES ('blobScrubCompletedAt', 'x', now() - interval '7 hours'); \
			 INSERT INTO blobs (hash, size, last_scrubbed_at) VALUES ('sha256:a', 1, now());",
		)
		.await
		else {
			return;
		};
		let check = super::run(db.central.clone()).await;
		assert_eq!(check.status.wire_result(), "warning", "{:?}", check.status);
		assert!(
			check
				.status
				.reason()
				.unwrap()
				.starts_with("the scrub has verified nothing for "),
			"{:?}",
			check.status
		);
		let idle = check.details["scrub_idle_seconds"].as_i64().unwrap();
		assert!((7 * 3600 - 10..=7 * 3600 + 100).contains(&idle), "{idle}");
	}

	#[tokio::test]
	async fn a_fresh_scrub_pass_passes_despite_old_blob_stamps() {
		let Some(db) = seeded(
			"INSERT INTO local_system_facts (key, value, updated_at) \
			 VALUES ('blobScrubCompletedAt', 'x', now() - interval '5 minutes'); \
			 INSERT INTO blobs (hash, size, last_scrubbed_at) \
			 VALUES ('sha256:a', 1, now() - interval '3 days');",
		)
		.await
		else {
			return;
		};
		let check = super::run(db.central.clone()).await;
		assert_status(&check, "passed", None);
		let idle = check.details["scrub_idle_seconds"].as_i64().unwrap();
		assert!((290..=400).contains(&idle), "{idle}");
	}

	#[tokio::test]
	async fn without_a_scrub_pass_the_blob_stamps_decide() {
		let Some(db) = seeded(
			"INSERT INTO blobs (hash, size, last_scrubbed_at) \
			 VALUES ('sha256:a', 1, now() - interval '7 hours');",
		)
		.await
		else {
			return;
		};
		let check = super::run(db.central.clone()).await;
		assert_eq!(check.status.wire_result(), "warning", "{:?}", check.status);
		assert!(
			check
				.status
				.reason()
				.unwrap()
				.starts_with("the scrub has verified nothing for "),
			"{:?}",
			check.status
		);
	}

	#[tokio::test]
	async fn an_empty_store_without_a_scrub_pass_passes() {
		let Some(db) = scratch_db(BLOB_STORE).await else {
			return;
		};
		let check = super::run(db.central.clone()).await;
		assert_status(&check, "passed", None);
		assert_eq!(check.summary, "blob store empty");
		assert_eq!(check.details["blobs"], 0);
	}
}
