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
use super::{CheckContext, query_error_check};
use crate::doctor::Stat;
use crate::doctor::check::Check;

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

/// Read separately from the blob registry: the rows these count are gone, which
/// is the whole reason the count exists. Both are null on a server that has never
/// dropped one, and on any Tamanu predating the counter.
const CACHE_DROPS_SQL: &str = "\
	SELECT \
	(SELECT value::bigint FROM local_system_facts \
	 WHERE key = 'blobCacheFaults' AND deleted_at IS NULL) AS dropped, \
	extract(epoch FROM now() - (SELECT value::timestamp FROM local_system_facts \
	 WHERE key = 'blobCacheFaultAt' AND deleted_at IS NULL))::bigint AS dropped_since";

const SQL: &str = "\
	SELECT count(*) AS blobs, \
	count(*) FILTER (WHERE integrity_state = 'corrupt') AS corrupt, \
	count(*) FILTER (WHERE integrity_state = 'absent') AS absent, \
	count(*) FILTER (WHERE integrity_state IN ('corrupt', 'absent') AND tier = 'outbox') AS outbox_faulty, \
	count(*) FILTER (WHERE integrity_state IN ('corrupt', 'absent') AND tier = 'cache') AS cache_faulty, \
	count(*) FILTER (WHERE last_scrubbed_at IS NULL) AS never_scrubbed, \
	extract(epoch FROM now() - coalesce(max(last_scrubbed_at), min(created_at)))::bigint AS scrub_idle_seconds \
	FROM blobs WHERE deleted_at IS NULL";

pub async fn run(ctx: CheckContext) -> Check {
	let Some(client) = ctx.db.as_ref() else {
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
	let scrub_idle_secs: Option<i64> = row.try_get("scrub_idle_seconds").unwrap_or(None);

	let (durable_faulty, replica_faulty) =
		split_faults(ctx.kind, corrupt, absent, outbox_faulty, cache_faulty);

	// A failure here is not worth losing the registry verdict over: the counter is
	// a supplement to it, and its absence is the normal state.
	let drops = client.query_one(CACHE_DROPS_SQL, &[]).await.ok();
	let dropped: Option<i64> = drops
		.as_ref()
		.and_then(|r| r.try_get("dropped").ok())
		.flatten();
	let dropped_since: Option<i64> = drops
		.as_ref()
		.and_then(|r| r.try_get("dropped_since").ok())
		.flatten();

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
				.help("Blobs whose stored bytes no longer match their hash, or are gone"),
		)
		.with_stat(
			Stat::gauge("absent", absent as f64)
				.group("faults")
				.help("Blobs whose stored bytes no longer match their hash, or are gone"),
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
				"Seconds since the scrub last stamped a blob, or since the oldest blob was stored",
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

/// Grade the store on what it has lost and whether anything is still checking.
///
/// `durable_faulty` counts copies that must be durably present on this server,
/// so one is enough to escalate; `replica_faulty` counts copies central still
/// holds, which refetch on demand.
///
/// `scrub_idle_secs` is the age of the newest scrub stamp, falling back to the
/// age of the oldest blob where nothing has been stamped at all, so a store
/// filled minutes ago does not read as unscrubbed before its first pass is due.
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
			"{total} cache blobs dropped for failing verification, the last one recently; each refetched, but a run of them reads as the storage failing"
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
	use crate::doctor::check::CheckStatus;
	use crate::doctor::checks::test_support::{central_ctx, facility_ctx};

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
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, "blob_integrity");
		assert!(
			!matches!(check.status, CheckStatus::Broken(_)),
			"a Tamanu without a blob store should skip, not break: {:?}",
			check.to_wire()["result"]
		);
	}

	/// Seed a corrupt blob and check the whole path grades it. Runs inside a
	/// transaction that is always rolled back, on this test's own connection, so
	/// it leaves the database as it found it.
	#[tokio::test]
	async fn grades_a_seeded_corrupt_blob_against_central() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let client = ctx.db.clone().expect("central_ctx carries a connection");
		if !blob_store_present(&client).await {
			return;
		}

		client
			.batch_execute(
				"BEGIN; \
				 INSERT INTO blobs (hash, size, integrity_state) \
				 VALUES ('sha256:0000000000000000000000000000000000000000000000000000000000000001', \
				 4096, 'corrupt');",
			)
			.await
			.expect("seeding a corrupt blob should succeed");

		let check = super::run(ctx).await;
		let rolled_back = client.batch_execute("ROLLBACK").await;

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

		rolled_back.expect("rollback should succeed");
	}

	async fn blob_store_present(client: &tokio_postgres::Client) -> bool {
		client
			.query_one("SELECT to_regclass('blobs') IS NOT NULL AS present", &[])
			.await
			.and_then(|row| row.try_get("present"))
			.unwrap_or(false)
	}

	#[tokio::test]
	async fn skips_without_a_database() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
