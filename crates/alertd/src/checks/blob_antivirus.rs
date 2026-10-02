//! Malware verdicts over stored blobs, and whether the scanner is being reached.
//!
//! Scanning is off unless a scanner is named, which is the default, and no
//! scanner means no scan pass, so this SKIPs even over verdicts a scanner since
//! switched off left behind. The
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
use bestool_tamanu::config::TamanuConfig;

use super::util::humanise_age;
use super::{TamanuCx, query_error_check};
use crate::Stat;
use crate::check::Check;

const NAME: &str = "blob_antivirus";

/// How long the scanner may record nothing, with content waiting, before it
/// reads as unreachable. The pass runs every fifteen minutes on central and on
/// facilities, so this is eight missed passes.
const STALL_SECS: i64 = 2 * 60 * 60;

/// Blobs above this are never sent to the scanner and stay unscanned by design,
/// so they are kept out of the backlog. Overridden by the deployment's own
/// `blobStorage.antivirus.maxScanMB` where it is set.
const DEFAULT_MAX_SCAN_MB: f64 = 25.0;

const SCANNER_KEY: &str = "blobStorage.antivirus.scanner";
const SERVE_POLICY_KEY: &str = "blobStorage.antivirus.servePolicy";
const MAX_SCAN_MB_KEY: &str = "blobStorage.antivirus.maxScanMB";

const SCANNER_NONE: &str = "none";
const POLICY_ONLY_KNOWN_GOOD: &str = "only-known-good";

const SETTINGS_SQL: &str = "\
	SELECT key, value, scope, facility_id FROM settings \
	WHERE (key = 'blobStorage' OR key LIKE 'blobStorage.%') AND deleted_at IS NULL";

const SQL: &str = "\
	SELECT count(*) AS blobs, \
	count(*) FILTER (WHERE scan_verdict IS NULL AND size <= $1) AS unscanned, \
	count(*) FILTER (WHERE scan_verdict IS NULL AND size > $1) AS unscannable, \
	count(*) FILTER (WHERE scan_verdict = 'clean') AS clean, \
	count(*) FILTER (WHERE scan_verdict = 'infected') AS infected, \
	extract(epoch FROM now() - greatest(max(scanned_at), \
	min(created_at) FILTER (WHERE scan_verdict IS NULL AND size <= $1)))::bigint AS scan_idle_seconds \
	FROM blobs WHERE deleted_at IS NULL AND integrity_state = 'verified'";

const FACILITY_IDS_FACT_SQL: &str =
	"SELECT value FROM local_system_facts WHERE key = 'facilityIds'";

const QUARANTINE_SQL: &str = "\
	SELECT count(*) AS quarantined, \
	count(*) FILTER (WHERE created_at > now() - interval '24 hours') AS quarantined_24h \
	FROM blob_quarantines WHERE deleted_at IS NULL";

pub async fn run(ctx: TamanuCx) -> Check {
	let Some(client) = ctx.db().await else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let kind = ctx.server_kind();
	let primary = if kind == ApiServerKind::Facility {
		let fact = match client.query_opt(FACILITY_IDS_FACT_SQL, &[]).await {
			Ok(row) => row.and_then(|row| row.try_get::<_, Option<String>>("value").ok().flatten()),
			Err(err) if is_missing_relation(&err) => None,
			Err(err) => return query_error_check(NAME, &err),
		};
		primary_facility(fact.as_deref(), &ctx.config)
	} else {
		None
	};
	let settings = match client.query(SETTINGS_SQL, &[]).await {
		Ok(rows) => blob_settings(
			rows.iter().filter_map(|row| {
				Some(SettingRow {
					key: row.try_get("key").ok()?,
					value: row.try_get("value").ok()?,
					scope: row.try_get("scope").ok().flatten(),
					facility_id: row.try_get("facility_id").ok().flatten(),
				})
			}),
			kind,
			primary.as_deref(),
		),
		Err(err) if is_missing_relation(&err) => Vec::new(),
		Err(err) => return query_error_check(NAME, &err),
	};
	let Posture {
		scanner,
		scanning,
		withholds_unscanned,
		max_scan_bytes,
	} = posture(&settings);

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

	if !scanning && quarantined == 0 {
		return Check::skip(
			NAME,
			"no antivirus scanning here",
			"no scanner is configured on this server, which is the default",
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
				"Seconds since the scanner last recorded a verdict or the oldest unscanned blob was stored, whichever is later",
			));
	}
	check
}

struct Posture {
	scanner: String,
	scanning: bool,
	withholds_unscanned: bool,
	max_scan_bytes: i64,
}

/// How this server is set to scan. With no scanner Tamanu runs no scan pass and
/// withholds nothing, whatever the serve policy says.
fn posture(settings: &[(String, Value)]) -> Posture {
	let scanner = setting(settings, SCANNER_KEY)
		.and_then(Value::as_str)
		.unwrap_or(SCANNER_NONE)
		.to_string();
	let scanning = scanner != SCANNER_NONE;
	let withholds_unscanned = scanning
		&& setting(settings, SERVE_POLICY_KEY)
			.and_then(Value::as_str)
			.is_some_and(|policy| policy == POLICY_ONLY_KNOWN_GOOD);
	let max_scan_mb = setting(settings, MAX_SCAN_MB_KEY)
		.and_then(Value::as_f64)
		.unwrap_or(DEFAULT_MAX_SCAN_MB);
	let max_scan_bytes = (max_scan_mb * 1024.0 * 1024.0).floor() as i64;
	Posture {
		scanner,
		scanning,
		withholds_unscanned,
		max_scan_bytes,
	}
}

enum Verdict {
	Pass,
	Warn(String),
	Fail(String),
}

/// Grade what the scanner has found and whether it is still being reached.
///
/// `scan_idle_secs` is the younger of the newest verdict and the oldest blob
/// still waiting for one, so a stall needs both the scanner silent and content
/// left waiting past a pass it was due, and a quiet store that has just received
/// an upload is not read as stalled. A store with no backlog records no new
/// verdicts either, which is why the idle time is only graded alongside content
/// waiting on it.
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

struct SettingRow {
	key: String,
	value: Value,
	scope: Option<String>,
	facility_id: Option<String>,
}

/// The facility a facility server reads its antivirus settings for: the first
/// of its facility ids, from the recorded fact ahead of the config file.
fn primary_facility(fact: Option<&str>, config: &TamanuConfig) -> Option<String> {
	match fact.and_then(|fact| serde_json::from_str::<Vec<String>>(fact).ok()) {
		Some(ids) => ids.into_iter().next(),
		None => config.server_facility_id.clone().or_else(|| {
			config
				.server_facility_ids
				.as_ref()
				.and_then(|ids| ids.first().cloned())
		}),
	}
}

/// The `blobStorage` settings that apply to this server, as key/value pairs.
///
/// Central and facility carry the same setting names under their own scope, and
/// central holds every facility's settings alongside its own, so the other
/// kind's scope is dropped rather than allowed to answer for this server. A
/// facility server reads only its primary facility's rows.
///
/// Ordered so [`setting`] meets them in Tamanu's precedence: the server's own
/// scope ahead of global, and within a scope the deeper key ahead of a parent
/// object holding the same path.
fn blob_settings(
	rows: impl IntoIterator<Item = SettingRow>,
	kind: ApiServerKind,
	primary_facility: Option<&str>,
) -> Vec<(String, Value)> {
	let own_scope = if kind == ApiServerKind::Central {
		"central"
	} else {
		"facility"
	};
	let mut rows: Vec<SettingRow> = rows
		.into_iter()
		.filter(|row| match row.scope.as_deref() {
			Some("facility") => {
				primary_facility.is_some_and(|id| row.facility_id.as_deref() == Some(id))
			}
			Some("central") => kind == ApiServerKind::Central,
			_ => true,
		})
		.collect();
	rows.sort_by_key(|row| {
		(
			row.scope.as_deref() != Some(own_scope),
			std::cmp::Reverse(row.key.len()),
		)
	});
	rows.into_iter().map(|row| (row.key, row.value)).collect()
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
	use crate::check::CheckStatus;
	use crate::checks::test_support::{BLOB_STORE, facility_ctx, scratch_db};

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
	fn no_scanner_means_no_scanning_and_nothing_withheld() {
		let rows = vec![(SERVE_POLICY_KEY.to_string(), json!(POLICY_ONLY_KNOWN_GOOD))];
		let posture = posture(&rows);
		assert!(!posture.scanning);
		assert!(!posture.withholds_unscanned);
	}

	#[test]
	fn a_named_scanner_under_only_known_good_withholds() {
		let rows = vec![
			(SCANNER_KEY.to_string(), json!("clamd")),
			(SERVE_POLICY_KEY.to_string(), json!(POLICY_ONLY_KNOWN_GOOD)),
		];
		let posture = posture(&rows);
		assert!(posture.scanning);
		assert!(posture.withholds_unscanned);
	}

	#[test]
	fn a_fractional_scan_cap_is_kept() {
		let rows = vec![(MAX_SCAN_MB_KEY.to_string(), json!(0.3))];
		assert_eq!(posture(&rows).max_scan_bytes, 314_572);
	}

	#[test]
	fn the_scan_cap_defaults_to_25_mb() {
		assert_eq!(posture(&[]).max_scan_bytes, 25 * 1024 * 1024);
	}

	fn row(scope: &str, facility_id: Option<&str>, scanner: &str) -> SettingRow {
		SettingRow {
			key: SCANNER_KEY.to_string(),
			value: json!(scanner),
			scope: Some(scope.to_string()),
			facility_id: facility_id.map(str::to_string),
		}
	}

	fn scanner_for(rows: Vec<SettingRow>, kind: ApiServerKind, primary: Option<&str>) -> String {
		let settings = blob_settings(rows, kind, primary);
		posture(&settings).scanner
	}

	#[test]
	fn a_facility_server_reads_its_primary_facility_only() {
		let rows = vec![
			row("facility", Some("facility-2"), "other"),
			row("facility", Some("facility-1"), "clamd"),
		];
		assert_eq!(
			scanner_for(rows, ApiServerKind::Facility, Some("facility-1")),
			"clamd"
		);
	}

	#[test]
	fn a_facility_server_with_no_facility_reads_no_facility_rows() {
		let rows = vec![row("facility", Some("facility-1"), "clamd")];
		assert_eq!(
			scanner_for(rows, ApiServerKind::Facility, None),
			SCANNER_NONE
		);
	}

	#[test]
	fn central_reads_its_own_and_global_scope_only() {
		let rows = vec![
			row("facility", Some("facility-1"), "other"),
			row("central", None, "clamd"),
		];
		assert_eq!(scanner_for(rows, ApiServerKind::Central, None), "clamd");
		let rows = vec![
			row("facility", Some("facility-1"), "other"),
			row("global", None, "clamd"),
		];
		assert_eq!(scanner_for(rows, ApiServerKind::Central, None), "clamd");
	}

	#[test]
	fn a_facility_server_ignores_central_scope() {
		let rows = vec![row("central", None, "other"), row("global", None, "clamd")];
		assert_eq!(
			scanner_for(rows, ApiServerKind::Facility, Some("facility-1")),
			"clamd"
		);
	}

	fn parent(scope: &str, facility_id: Option<&str>, antivirus: Value) -> SettingRow {
		SettingRow {
			key: "blobStorage".to_string(),
			value: json!({ "antivirus": antivirus }),
			scope: Some(scope.to_string()),
			facility_id: facility_id.map(str::to_string),
		}
	}

	#[test]
	fn own_scope_outranks_global_whatever_the_row_order() {
		let rows = vec![
			row("global", None, "other"),
			row("facility", Some("facility-1"), "clamd"),
		];
		assert_eq!(
			scanner_for(rows, ApiServerKind::Facility, Some("facility-1")),
			"clamd"
		);
		let rows = vec![row("global", None, "other"), row("central", None, "clamd")];
		assert_eq!(scanner_for(rows, ApiServerKind::Central, None), "clamd");
	}

	#[test]
	fn an_own_scope_parent_object_outranks_a_global_leaf() {
		let rows = vec![
			row("global", None, "other"),
			parent("central", None, json!({ "scanner": "clamd" })),
		];
		assert_eq!(scanner_for(rows, ApiServerKind::Central, None), "clamd");
	}

	#[test]
	fn a_leaf_outranks_its_parent_object_in_the_same_scope() {
		let rows = vec![
			parent("central", None, json!({ "scanner": "other" })),
			row("central", None, "clamd"),
		];
		assert_eq!(scanner_for(rows, ApiServerKind::Central, None), "clamd");
	}

	#[test]
	fn an_own_scope_parent_without_the_path_leaves_it_to_global() {
		let rows = vec![
			parent("central", None, json!({ "address": "localhost:3310" })),
			row("global", None, "clamd"),
		];
		assert_eq!(scanner_for(rows, ApiServerKind::Central, None), "clamd");
	}

	fn config(extra: Value) -> TamanuConfig {
		let mut json =
			json!({ "db": { "name": "tamanu-facility", "username": "u", "password": "p" } });
		json.as_object_mut()
			.unwrap()
			.extend(extra.as_object().unwrap().clone());
		serde_json::from_value(json).unwrap()
	}

	#[test]
	fn the_primary_facility_is_the_first_configured() {
		let plural = config(json!({ "serverFacilityIds": ["facility-1", "facility-2"] }));
		assert_eq!(
			primary_facility(None, &plural).as_deref(),
			Some("facility-1")
		);
		let singular = config(json!({ "serverFacilityId": "facility-3" }));
		assert_eq!(
			primary_facility(None, &singular).as_deref(),
			Some("facility-3")
		);
		assert_eq!(primary_facility(None, &config(json!({}))), None);
	}

	#[test]
	fn the_recorded_facility_ids_outrank_the_config_file() {
		let plural = config(json!({ "serverFacilityIds": ["facility-1"] }));
		assert_eq!(
			primary_facility(Some(r#"["facility-9","facility-1"]"#), &plural).as_deref(),
			Some("facility-9")
		);
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
		let Some(db) = scratch_db(BLOB_STORE).await else {
			return;
		};
		let check = super::run(db.central.clone()).await;
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
		let Some(db) = scratch_db(BLOB_STORE).await else {
			return;
		};
		let check = super::run(db.central.clone()).await;
		assert!(
			check.status.is_skip(),
			"no scanner and no verdicts should skip: {:?} — {}",
			check.status,
			check.summary
		);
	}

	/// Seed a quarantine and check the whole path grades it, including on a
	/// server that drives no scanner of its own.
	#[tokio::test]
	async fn grades_a_seeded_quarantine_against_central() {
		let Some(db) = scratch_db(BLOB_STORE).await else {
			return;
		};

		db.central
			.db()
			.await
			.expect("a scratch database carries a connection")
			.batch_execute(
				"INSERT INTO blob_quarantines (hash, scanner_version, signature_version) \
				 VALUES ('sha256:0000000000000000000000000000000000000000000000000000000000000002', \
				 'probe-1', 'probe-sig-1');",
			)
			.await
			.expect("seeding a quarantine should succeed");

		let check = super::run(db.central.clone()).await;
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
	}

	/// Seed a scanner with content waiting on it and no verdict for hours, which
	/// is the scanner not being reached.
	#[tokio::test]
	async fn grades_a_seeded_stall_against_central() {
		let Some(db) = scratch_db(BLOB_STORE).await else {
			return;
		};

		db.central
			.db()
			.await
			.expect("a scratch database carries a connection")
			.batch_execute(
				"INSERT INTO settings (key, value) \
				 VALUES ('blobStorage.antivirus.scanner', '\"clamd\"'); \
				 INSERT INTO blobs (hash, size, created_at) \
				 VALUES ('sha256:0000000000000000000000000000000000000000000000000000000000000003', \
				 4096, now() - interval '6 hours');",
			)
			.await
			.expect("seeding an unscanned blob should succeed");

		let check = super::run(db.central.clone()).await;
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
	async fn skips_without_a_database() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
