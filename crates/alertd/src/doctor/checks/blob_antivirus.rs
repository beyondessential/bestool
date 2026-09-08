//! Malware verdicts over stored blobs, and whether the scanner is being reached.
//!
//! Scanning is off unless a scanner is named, which is the default, and no
//! scanner means no verdicts at all — the normal state, so this SKIPs. The
//! quarantine record propagates from central, so standing quarantines are
//! reported even on a server that drives no scanner of its own.
//!
//! Quarantined content WARNs however new it is: it is a deliberate record that
//! is meant to stand, and the runbook forbids deleting the row, so a FAIL here
//! could never be cleared by anything the runbook allows.
//!
//! What FAILs is the scanner going unreached with content waiting on it, and
//! only under `only-known-good`, where unscanned content is withheld from
//! clinicians. Under the other postures nothing is lost and it WARNs.

use serde_json::Value;
use tokio_postgres::error::SqlState;

use bestool_tamanu::ApiServerKind;

use super::util::humanise_age;
use super::{CheckContext, query_error_check};
use crate::doctor::Stat;
use crate::doctor::check::Check;

const NAME: &str = "blob_antivirus";

/// How long the scanner may record nothing, with content waiting, before it
/// reads as unreachable. The pass runs every fifteen minutes on central and on
/// facilities, so this is eight missed passes.
const STALL_SECS: i64 = 2 * 60 * 60;

/// Blobs above this are never sent to the scanner and stay unscanned by design,
/// so they are kept out of the backlog. Overridden by the deployment's own
/// `blobStorage.antivirus.maxScanMB` where it is set.
const DEFAULT_MAX_SCAN_MB: i64 = 25;

const SCANNER_KEY: &str = "blobStorage.antivirus.scanner";
const SERVE_POLICY_KEY: &str = "blobStorage.antivirus.servePolicy";
const MAX_SCAN_MB_KEY: &str = "blobStorage.antivirus.maxScanMB";

const SCANNER_NONE: &str = "none";
const POLICY_ONLY_KNOWN_GOOD: &str = "only-known-good";

const SETTINGS_SQL: &str = "\
	SELECT key, value, scope FROM settings \
	WHERE (key = 'blobStorage' OR key LIKE 'blobStorage.%') AND deleted_at IS NULL";

const SQL: &str = "\
	SELECT count(*) AS blobs, \
	count(*) FILTER (WHERE scan_verdict IS NULL AND size <= $1) AS unscanned, \
	count(*) FILTER (WHERE scan_verdict IS NULL AND size > $1) AS unscannable, \
	count(*) FILTER (WHERE scan_verdict = 'clean') AS clean, \
	count(*) FILTER (WHERE scan_verdict = 'infected') AS infected, \
	extract(epoch FROM now() - coalesce(max(scanned_at), \
	min(created_at) FILTER (WHERE scan_verdict IS NULL AND size <= $1)))::bigint AS scan_idle_seconds \
	FROM blobs WHERE deleted_at IS NULL AND integrity_state = 'verified'";

const QUARANTINE_SQL: &str = "\
	SELECT count(*) AS quarantined, \
	count(*) FILTER (WHERE created_at > now() - interval '24 hours') AS quarantined_24h \
	FROM blob_quarantines WHERE deleted_at IS NULL";

pub async fn run(ctx: CheckContext) -> Check {
	let Some(client) = ctx.db.as_ref() else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let settings = match client.query(SETTINGS_SQL, &[]).await {
		Ok(rows) => blob_settings(&rows, ctx.kind),
		Err(err) if is_missing_relation(&err) => Vec::new(),
		Err(err) => return query_error_check(NAME, &err),
	};
	let scanner = setting(&settings, SCANNER_KEY)
		.and_then(Value::as_str)
		.unwrap_or(SCANNER_NONE)
		.to_string();
	let withholds_unscanned = setting(&settings, SERVE_POLICY_KEY)
		.and_then(Value::as_str)
		.is_some_and(|policy| policy == POLICY_ONLY_KNOWN_GOOD);
	let max_scan_bytes = setting(&settings, MAX_SCAN_MB_KEY)
		.and_then(Value::as_i64)
		.unwrap_or(DEFAULT_MAX_SCAN_MB)
		* 1024 * 1024;

	let row = match client.query_one(SQL, &[&max_scan_bytes]).await {
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
	let unscanned: i64 = row.try_get("unscanned").unwrap_or(0);
	let unscannable: i64 = row.try_get("unscannable").unwrap_or(0);
	let clean: i64 = row.try_get("clean").unwrap_or(0);
	let infected: i64 = row.try_get("infected").unwrap_or(0);
	let scan_idle_secs: Option<i64> = row.try_get("scan_idle_seconds").unwrap_or(None);

	let (quarantined, quarantined_24h) = match client.query_one(QUARANTINE_SQL, &[]).await {
		Ok(row) => (
			row.try_get("quarantined").unwrap_or(0),
			row.try_get("quarantined_24h").unwrap_or(0),
		),
		Err(err) if is_missing_relation(&err) => (0, 0),
		Err(err) => return query_error_check(NAME, &err),
	};

	// A scanner turned off leaves its verdicts behind, and they still grade.
	let scanning = scanner != SCANNER_NONE || clean + infected > 0;
	if !scanning && quarantined == 0 {
		return Check::skip(
			NAME,
			"no antivirus scanning here",
			"no scanner is configured on this server and no verdict is recorded, which is the default",
		);
	}

	let summary = if !scanning {
		format!("no scanner here, {quarantined} hash(es) quarantined")
	} else if quarantined == 0 {
		format!("{blobs} blobs: {clean} clean, {unscanned} unscanned")
	} else {
		format!("{blobs} blobs: {clean} clean, {unscanned} unscanned, {quarantined} quarantined")
	};

	let check = match classify(
		scanning,
		unscanned,
		scan_idle_secs,
		withholds_unscanned,
		quarantined,
	) {
		Verdict::Pass => Check::pass(NAME, summary),
		Verdict::Warn(reason) => Check::warning(NAME, summary, reason),
		Verdict::Fail(reason) => Check::fail(NAME, summary, reason),
	};

	let mut check = check
		.with_detail("scanner", scanner)
		.with_detail("blobs", blobs)
		.with_detail("clean", clean)
		.with_detail("infected", infected)
		.with_detail("unscanned", unscanned)
		.with_detail("unscannable", unscannable)
		.with_detail("quarantined", quarantined)
		.with_detail("quarantined_24h", quarantined_24h)
		.with_detail("withholds_unscanned", withholds_unscanned)
		.with_stat(
			Stat::gauge("unscanned", unscanned as f64)
				.group("coverage")
				.help("Blobs by what this server's scanner has found in them"),
		)
		.with_stat(
			Stat::gauge("clean", clean as f64)
				.group("coverage")
				.help("Blobs by what this server's scanner has found in them"),
		)
		.with_stat(
			Stat::gauge("infected", infected as f64)
				.group("coverage")
				.help("Blobs by what this server's scanner has found in them"),
		)
		.with_stat(
			Stat::gauge("quarantined", quarantined as f64)
				.help("Hashes the deployment knows to be malware"),
		);
	if let Some(idle) = scan_idle_secs {
		check = check
			.with_detail("scan_idle_seconds", idle)
			.with_stat(Stat::gauge("scan_idle_seconds", idle as f64).help(
				"Seconds since the scanner last recorded a verdict, or since the oldest unscanned blob was stored",
			));
	}
	check
}

enum Verdict {
	Pass,
	Warn(String),
	Fail(String),
}

/// Grade what the scanner has found and whether it is still being reached.
///
/// `scan_idle_secs` is the age of the newest verdict, falling back to the age of
/// the oldest blob still waiting for one, so a scanner just switched on is not
/// read as stalled before its first pass is due. A store with no backlog records
/// no new verdicts either, which is why the idle time is only graded alongside
/// content waiting on it.
fn classify(
	scanning: bool,
	unscanned: i64,
	scan_idle_secs: Option<i64>,
	withholds_unscanned: bool,
	quarantined: i64,
) -> Verdict {
	let stalled = scanning && unscanned > 0 && scan_idle_secs.is_some_and(|secs| secs > STALL_SECS);
	let idle = humanise_age(scan_idle_secs.unwrap_or(0));

	if stalled && withholds_unscanned {
		Verdict::Fail(format!(
			"no verdict recorded for {idle} with {unscanned} blob(s) waiting, and the serve policy withholds unscanned content"
		))
	} else if quarantined > 0 {
		Verdict::Warn(format!(
			"{quarantined} hash(es) quarantined as malware, retained and never served"
		))
	} else if stalled {
		Verdict::Warn(format!(
			"no verdict recorded for {idle} with {unscanned} blob(s) waiting, so the scanner is not being reached"
		))
	} else {
		Verdict::Pass
	}
}

/// The `blobStorage` settings that apply to this server, as key/value pairs.
///
/// Central and facility carry the same setting names under their own scope, and
/// central holds every facility's settings alongside its own, so the other
/// kind's scope is dropped rather than allowed to answer for this server.
fn blob_settings(rows: &[tokio_postgres::Row], kind: ApiServerKind) -> Vec<(String, Value)> {
	let foreign_scope = if kind == ApiServerKind::Central {
		"facility"
	} else {
		"central"
	};
	rows.iter()
		.filter(|row| {
			!row.try_get::<_, String>("scope")
				.is_ok_and(|scope| scope == foreign_scope)
		})
		.filter_map(|row| {
			Some((
				row.try_get::<_, String>("key").ok()?,
				row.try_get::<_, Value>("value").ok()?,
			))
		})
		.collect()
}

/// Read one dotted setting path out of the stored rows.
///
/// Settings are written one row per leaf, but a whole object can also be stored
/// under a parent key, so a row is a match when its key is the path or a prefix
/// of it whose value carries the rest.
fn setting<'a>(rows: &'a [(String, Value)], path: &str) -> Option<&'a Value> {
	rows.iter().find_map(|(key, value)| {
		if key == path {
			return Some(value);
		}
		let rest = path.strip_prefix(key.as_str())?.strip_prefix('.')?;
		rest.split('.')
			.try_fold(value, |value, segment| value.get(segment))
	})
}

fn is_missing_relation(err: &tokio_postgres::Error) -> bool {
	err.as_db_error()
		.is_some_and(|db| db.code() == &SqlState::UNDEFINED_TABLE)
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;
	use crate::doctor::check::CheckStatus;
	use crate::doctor::checks::test_support::{central_ctx, facility_ctx};

	fn grade(
		scanning: bool,
		unscanned: i64,
		idle: Option<i64>,
		withholds: bool,
		quarantined: i64,
	) -> &'static str {
		match classify(scanning, unscanned, idle, withholds, quarantined) {
			Verdict::Pass => "pass",
			Verdict::Warn(_) => "warn",
			Verdict::Fail(_) => "fail",
		}
	}

	#[test]
	fn a_scanned_store_passes() {
		assert_eq!(grade(true, 0, Some(STALL_SECS * 10), false, 0), "pass");
	}

	#[test]
	fn a_backlog_the_pass_is_working_through_passes() {
		assert_eq!(grade(true, 500, Some(STALL_SECS), false, 0), "pass");
	}

	#[test]
	fn a_stalled_scanner_warns() {
		assert_eq!(grade(true, 1, Some(STALL_SECS + 1), false, 0), "warn");
	}

	#[test]
	fn a_stalled_scanner_withholding_content_fails() {
		assert_eq!(grade(true, 1, Some(STALL_SECS + 1), true, 0), "fail");
	}

	#[test]
	fn a_server_that_does_not_scan_is_never_stalled() {
		// Every blob is unscanned by design where no scanner is configured.
		assert_eq!(grade(false, 5_000, Some(STALL_SECS * 100), true, 0), "pass");
	}

	#[test]
	fn quarantined_content_warns_however_long_it_stands() {
		assert_eq!(grade(true, 0, Some(0), false, 1), "warn");
		assert_eq!(grade(false, 0, None, false, 3), "warn");
	}

	#[test]
	fn withheld_content_outranks_a_standing_quarantine() {
		assert_eq!(grade(true, 1, Some(STALL_SECS + 1), true, 1), "fail");
	}

	#[test]
	fn a_leaf_setting_is_read() {
		let rows = vec![(SCANNER_KEY.to_string(), json!("clamd"))];
		assert_eq!(setting(&rows, SCANNER_KEY).unwrap(), &json!("clamd"));
	}

	#[test]
	fn a_setting_stored_under_a_parent_is_read() {
		let rows = vec![(
			"blobStorage".to_string(),
			json!({ "antivirus": { "scanner": "clamd", "maxScanMB": 40 } }),
		)];
		assert_eq!(setting(&rows, SCANNER_KEY).unwrap(), &json!("clamd"));
		assert_eq!(
			setting(&rows, MAX_SCAN_MB_KEY).and_then(Value::as_i64),
			Some(40)
		);
		assert!(setting(&rows, SERVE_POLICY_KEY).is_none());
	}

	#[test]
	fn an_unrelated_key_does_not_answer_for_the_path() {
		let rows = vec![("blobStorageRoot".to_string(), json!("data/blobs"))];
		assert!(setting(&rows, SCANNER_KEY).is_none());
	}

	#[tokio::test]
	async fn runs_against_central() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, "blob_antivirus");
		assert!(
			!matches!(check.status, CheckStatus::Broken(_)),
			"a Tamanu without a blob store should skip, not break: {:?}",
			check.to_wire()["result"]
		);
	}

	/// A store with no scanner named and no verdict recorded is the default, and
	/// says nothing about the deployment's health.
	#[tokio::test]
	async fn a_store_without_a_scanner_skips() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		if !blob_store_present(ctx.db.as_ref().expect("central_ctx carries a connection")).await {
			return;
		}
		let check = super::run(ctx).await;
		assert!(
			check.status.is_skip(),
			"no scanner and no verdicts should skip: {:?} — {}",
			check.status,
			check.summary
		);
	}

	/// Seed a quarantine and check the whole path grades it, including on a
	/// server that drives no scanner of its own. Runs inside a transaction that
	/// is always rolled back, on this test's own connection, so it leaves the
	/// database as it found it.
	#[tokio::test]
	async fn grades_a_seeded_quarantine_against_central() {
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
				 INSERT INTO blob_quarantines (hash, scanner_version, signature_version) \
				 VALUES ('sha256:0000000000000000000000000000000000000000000000000000000000000002', \
				 'probe-1', 'probe-sig-1');",
			)
			.await
			.expect("seeding a quarantine should succeed");

		let check = super::run(ctx).await;
		let rolled_back = client.batch_execute("ROLLBACK").await;

		assert!(
			matches!(check.status, CheckStatus::Warning(_)),
			"a standing quarantine should warn: {:?} — {}",
			check.status,
			check.summary
		);
		assert!(
			check.details["quarantined"].as_i64().unwrap_or(0) >= 1,
			"the seeded quarantine should be counted: {:?}",
			check.details
		);

		rolled_back.expect("rollback should succeed");
	}

	/// Seed a scanner with content waiting on it and no verdict for hours, which
	/// is the scanner not being reached.
	#[tokio::test]
	async fn grades_a_seeded_stall_against_central() {
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
				 INSERT INTO settings (key, value) \
				 VALUES ('blobStorage.antivirus.scanner', '\"clamd\"'); \
				 INSERT INTO blobs (hash, size, created_at) \
				 VALUES ('sha256:0000000000000000000000000000000000000000000000000000000000000003', \
				 4096, now() - interval '6 hours');",
			)
			.await
			.expect("seeding an unscanned blob should succeed");

		let check = super::run(ctx).await;
		let rolled_back = client.batch_execute("ROLLBACK").await;

		assert!(
			matches!(check.status, CheckStatus::Warning(_)),
			"a scanner that has recorded nothing for hours should warn: {:?} — {}",
			check.status,
			check.summary
		);
		assert!(
			check.details["scan_idle_seconds"].as_i64().unwrap_or(0) >= 6 * 60 * 60,
			"the wait should be measured from the oldest unscanned blob: {:?}",
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
