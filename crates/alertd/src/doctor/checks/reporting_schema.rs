//! Whether this server has the reporting schema canopy offers it.
//!
//! A reporting schema is the set of views a server's reports read from. Half of
//! it follows from the Tamanu version's schema and half from the group's own
//! configuration, so it is built centrally, against a replica of the group's
//! data at that version, and offered back per group.
//!
//! What a server has is stamped into the schema itself by the SQL that built it.
//! This check reads that stamp, compares it against the version canopy offers
//! for the version the server runs, and reports the stamp as a top-level status
//! fact so the fleet view can show which schema each server is on. Applying the
//! offered schema is the check's heal action, so it happens only in the daemon
//! and only when the drift has graded as a failure.

use std::sync::Arc;

use bestool_canopy::CanopyClient;
use miette::{IntoDiagnostic as _, bail};

use super::{SweepContext, fmt_db_error};
use crate::doctor::{check::Check, heal::HealOutcome};

const NAME: &str = "reporting_schema";

/// The status fact canopy reads to show which schema a server is on.
const VERSION_FACT: &str = "reportingSchemaVersion";

/// The artifact type a reporting schema is published under.
const ARTIFACT_TYPE: &str = "reporting-schema";

/// Reads the version the built schema stamped on itself. The schema is dropped
/// and recreated wholesale by the SQL that builds it, so the stamp goes with it
/// and can never outlive the schema it describes.
const STAMP_SQL: &str = "SELECT obj_description(oid, 'pg_namespace') AS stamp \
	FROM pg_namespace WHERE nspname = 'reporting'";

pub async fn run(ctx: SweepContext) -> Check {
	let Some(tamanu) = ctx.tamanu.as_ref() else {
		return Check::skip(
			NAME,
			"no Tamanu on this host",
			"a reporting schema belongs to a Tamanu server, and this host has none",
		);
	};

	// The `host` registration this check uses runs whatever the context, so
	// the generic `DATABASE_URL` fallback reaches here as a database that is
	// not Tamanu's. Nothing about a reporting schema applies to it, and heal
	// would drop and recreate a schema on it.
	if !tamanu.is_tamanu {
		return Check::skip(
			NAME,
			"no Tamanu on this host",
			"a reporting schema belongs to a Tamanu database, and this one is not",
		);
	}

	let Some(db) = tamanu.db.as_ref() else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let running = match read_stamp(db).await {
		Ok(stamp) => stamp,
		Err(err) => {
			return Check::broken(
				NAME,
				"could not read the reporting schema",
				format!(
					"reading the schema's version stamp failed: {}",
					fmt_db_error(&err)
				),
			);
		}
	};

	let Some(canopy) = ctx.canopy.as_ref() else {
		// Offline: say what the server has and grade nothing. Whether it is the
		// right one is canopy's to answer, and canopy is not reachable.
		return with_version(
			Check::skip(
				NAME,
				"canopy unreachable",
				"cannot tell whether this schema is the one offered without asking canopy",
			),
			&running,
		);
	};

	let offered = match offered_schema(canopy, &tamanu.tamanu_version.to_string()).await {
		Ok(offered) => offered,
		Err(err) if canopy_is_out(&err) => {
			return with_version(
				Check::skip(
					NAME,
					"canopy unreachable",
					format!("cannot tell whether this schema is the one offered: {err}"),
				),
				&running,
			);
		}
		Err(err) => {
			return with_version(
				Check::warning(
					NAME,
					"could not ask canopy what is offered",
					format!("fetching the offered reporting schema failed: {err}"),
				),
				&running,
			);
		}
	};

	let Some(offered) = offered else {
		// Canopy offers none for this version. The pair has not been built yet,
		// which is canopy's own finding to raise, not this server's fault.
		return with_version(
			Check::skip(
				NAME,
				"none offered for this version",
				"canopy has no reporting schema built for the version this server runs",
			),
			&running,
		);
	};

	with_version(grade(&running, &offered.version), &running)
}

/// What the server's `reporting` schema says about itself.
///
/// A schema with no comment on it is not the same as no schema: something built
/// it that was not this pipeline, so the operator is replacing a schema rather
/// than applying a first one, and whatever reports read from it are reading
/// something nobody can name.
#[derive(Debug, PartialEq, Eq)]
enum Stamp {
	NoSchema,
	Unstamped,
	Version(String),
}

/// Longest a stamp may be and still be a version. The comment is arbitrary
/// text that anyone with COMMENT rights on the schema can set, and it is
/// published to canopy as a status fact, so what is not plausibly a version is
/// read as no stamp rather than carried.
const MAX_STAMP_LEN: usize = 64;

/// Read what the server's `reporting` schema stamped on itself.
async fn read_stamp(db: &tokio_postgres::Client) -> Result<Stamp, tokio_postgres::Error> {
	Ok(match db.query_opt(STAMP_SQL, &[]).await? {
		Some(row) => match row.get::<_, Option<String>>("stamp").map(stamp_of) {
			Some(Some(version)) => Stamp::Version(version),
			Some(None) | None => Stamp::Unstamped,
		},
		// No `reporting` schema at all. Not an error: a server that has never
		// had one applied is exactly what this check exists to surface.
		None => Stamp::NoSchema,
	})
}

/// The version a schema comment names, where the comment is one.
fn stamp_of(comment: String) -> Option<String> {
	let trimmed = comment.trim();
	if trimmed.is_empty() || trimmed.len() > MAX_STAMP_LEN {
		return None;
	}

	node_semver::Version::parse(trimmed)
		.is_ok()
		.then(|| trimmed.to_owned())
}

/// What the stamp on the server says against what canopy offers.
///
/// Separated from the sweep because this is the whole judgement the check
/// makes, and it is worth being able to state it without a database.
fn grade(running: &Stamp, offered: &str) -> Check {
	match running {
		Stamp::Version(stamp) if stamp == offered => {
			Check::pass(NAME, format!("reporting schema {stamp}"))
		}
		Stamp::Version(stamp) => Check::fail(
			NAME,
			format!("reporting schema {stamp}, offered {offered}"),
			"the server's reports read from a schema built for a different version",
		),
		Stamp::Unstamped => Check::fail(
			NAME,
			format!("reporting schema unstamped, offered {offered}"),
			"the server has a reporting schema that names no version, so what its \
			 reports read from cannot be told apart from any other build",
		),
		Stamp::NoSchema => Check::fail(
			NAME,
			"no reporting schema",
			"canopy offers one for the version this server runs, and the server has none",
		),
	}
}

/// Carry the stamp as a top-level status fact, so the fleet view can show which
/// schema a server is on without reading into the check's own detail.
fn with_version(check: Check, running: &Stamp) -> Check {
	match running {
		Stamp::Version(version) => {
			check.with_payload_extra(VERSION_FACT, serde_json::Value::from(version.as_str()))
		}
		Stamp::NoSchema | Stamp::Unstamped => check,
	}
}

/// A reporting schema canopy offers, and the version it was built for.
struct Offered {
	version: String,
	id: String,
}

/// Ask canopy which reporting schema this server is offered.
///
/// The call is authenticated, which is what makes it a group-scoped answer:
/// canopy resolves the caller to its machine and its group and offers that
/// group's schema. The same call unauthenticated would only ever see the
/// artifacts that belong to no group.
async fn offered_schema(
	canopy: &Arc<CanopyClient>,
	version: &str,
) -> Result<Option<Offered>, bestool_canopy::Error> {
	let artifacts = match canopy.versions_artifacts(version).await {
		Ok(artifacts) => artifacts,
		Err(err) if offers_nothing(&err) => return Ok(None),
		Err(err) => return Err(err),
	};

	if let Some(range) = artifacts.iter().find_map(range_schema) {
		tracing::warn!(
			%range,
			"canopy offers a reporting schema registered against a range; ignoring it"
		);
	}

	Ok(artifacts
		.into_iter()
		.find(is_exact_schema)
		.map(|a| Offered {
			version: version.to_owned(),
			id: a.id.to_string(),
		}))
}

/// Whether an artifact is a reporting schema this server may grade against.
///
/// A schema is published for one exact version, since it follows the
/// migrations that version applies: one built against a patch is not the
/// schema another patch of the same minor describes. Canopy resolves a range
/// artifact for any version it covers, so grading against one would report a
/// server as current on a schema built for something else.
fn is_exact_schema(artifact: &bestool_canopy::schema::Artifact) -> bool {
	artifact.artifact_type == ARTIFACT_TYPE && artifact.version_range_pattern.is_none()
}

/// The range a reporting-schema artifact was registered against, where it was
/// registered against one at all. Worth saying out loud: it means a build
/// published a schema canopy will hand to versions it was not built for.
fn range_schema(artifact: &bestool_canopy::schema::Artifact) -> Option<&str> {
	(artifact.artifact_type == ARTIFACT_TYPE)
		.then_some(artifact.version_range_pattern.as_deref())
		.flatten()
}

/// Whether canopy's answer means it offers nothing for this version, as
/// against the ask itself having failed.
///
/// Canopy answers the artifacts of a version it holds no published, ready
/// release for with a 404, which is the ordinary case for a server on a
/// version canopy has not published. A pair canopy has not built is canopy's
/// own finding to raise rather than this server's fault, so it grades as
/// nothing offered rather than as a warning against the server.
fn offers_nothing(err: &bestool_canopy::Error) -> bool {
	err.status() == Some(bestool_canopy::http::StatusCode::NOT_FOUND)
}

/// Whether the ask failed for canopy's own reasons rather than this server's:
/// the request never landed, or canopy answered with a fault of its own. There
/// is nothing an operator on this server can do about either, and grading them
/// raises the same finding on every server in the fleet for the length of a
/// canopy outage.
fn canopy_is_out(err: &bestool_canopy::Error) -> bool {
	match err {
		// The request never landed.
		bestool_canopy::Error::Transport(_) => true,
		// Anything else that carries no status — an answer that would not
		// decode, above all — is canopy's shape having moved, which has to be
		// visible rather than skipped past.
		other => other
			.status()
			.is_some_and(|status| status.is_server_error()),
	}
}

/// Fetch the bytes of the schema canopy offers.
///
/// Canopy holds a group-scoped artifact itself and serves it only to a caller
/// it is offered to, so the fetch has to carry the device credential the ask
/// carried. An unauthenticated GET of the same URL is answered as a missing
/// artifact, not as a refusal.
async fn fetch_offered(
	canopy: &Arc<CanopyClient>,
	offered: &Offered,
) -> Result<String, miette::Report> {
	let path = download_path(&offered.version, &offered.id);

	let mut response = canopy
		.transport()
		.get(&format!("/public{path}"), &path)
		.await?
		.error_for_status()
		.into_diagnostic()?;

	// A 2xx is not on its own a schema: an HTML page from something between
	// here and canopy would be executed as SQL, and the schema's own SQL drops
	// itself first, so a wrong body destroys what it does not replace.
	let media_type = response
		.headers()
		.get(reqwest::header::CONTENT_TYPE)
		.and_then(|v| v.to_str().ok())
		.map(|v| v.split(';').next().unwrap_or(v).trim().to_ascii_lowercase())
		.unwrap_or_default();
	if !SCHEMA_MEDIA_TYPES.contains(&media_type.as_str()) {
		bail!("the offered reporting schema is {media_type}, not SQL");
	}

	if let Some(len) = response.content_length()
		&& len > MAX_SCHEMA_BYTES as u64
	{
		bail!("the offered reporting schema is larger than {MAX_SCHEMA_BYTES} bytes");
	}

	let mut sql = Vec::new();
	while let Some(chunk) = response.chunk().await.into_diagnostic()? {
		if sql.len() + chunk.len() > MAX_SCHEMA_BYTES {
			bail!("the offered reporting schema is larger than {MAX_SCHEMA_BYTES} bytes");
		}
		sql.extend_from_slice(&chunk);
	}
	if sql.is_empty() {
		bail!("the offered reporting schema is empty");
	}

	String::from_utf8(sql).into_diagnostic()
}

/// What a reporting schema may be served as. Canopy hands back whatever media
/// type the registration named, and a schema is SQL text.
const SCHEMA_MEDIA_TYPES: &[&str] = &["application/sql", "text/plain", "application/octet-stream"];

/// Ceiling on a schema, matching what canopy will hold for one.
const MAX_SCHEMA_BYTES: usize = 32 * 1024 * 1024;

/// The path to ask the transport for.
///
/// The transport addresses canopy by path so that it reaches whichever of the
/// two endpoints holds the credential, and over tailscale the public API is
/// mounted a level down. The path is built from the artifact's id rather than
/// taken from the offer's `download_url`: a path is resolved against the
/// transport's own base, so one carrying an authority of its own would present
/// the device credential to whatever host named it.
fn download_path(version: &str, id: &str) -> String {
	format!("/versions/{version}/artifacts/{id}/download")
}

/// Apply the schema canopy offers.
///
/// Applying is the one thing on this host that writes to Tamanu's database, so
/// it lives here rather than in the check: heal runs only in the daemon, only
/// when the check graded a failure, and behind the shared backoff.
pub async fn heal(ctx: SweepContext) -> HealOutcome {
	// A heal that never returns holds the attempt slot for the life of the
	// process, so self-heal stops for this check with nothing to say so.
	match tokio::time::timeout(HEAL_DEADLINE, apply_offered(ctx)).await {
		Ok(outcome) => outcome,
		Err(_) => {
			tracing::warn!("applying the reporting schema did not finish; giving up the attempt");
			HealOutcome::Failed
		}
	}
}

/// Longest one apply may take before the attempt is abandoned.
const HEAL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10 * 60);

async fn apply_offered(ctx: SweepContext) -> HealOutcome {
	let Some(tamanu) = ctx.tamanu.as_ref() else {
		return HealOutcome::Deferred;
	};
	if !tamanu.is_tamanu {
		return HealOutcome::Deferred;
	}
	let (Some(db), Some(canopy)) = (tamanu.db.as_ref(), ctx.canopy.as_ref()) else {
		return HealOutcome::Deferred;
	};

	let offered = match offered_schema(canopy, &tamanu.tamanu_version.to_string()).await {
		Ok(Some(offered)) => offered,
		Ok(None) => return HealOutcome::Deferred,
		Err(err) => {
			tracing::warn!("asking canopy for the reporting schema failed: {err}");
			return HealOutcome::Failed;
		}
	};

	// Applying drops the schema before it recreates it, so an artifact that has
	// already been applied and did not leave the stamp it should is not applied
	// again. Retrying it rebuilds the schema on every backoff step, forever,
	// with reports broken through each rebuild.
	if applied_without_stamping(&offered.id) {
		tracing::warn!(
			artifact = %offered.id,
			"the offered reporting schema has already been applied without stamping its version"
		);
		return HealOutcome::Deferred;
	}

	let sql = match fetch_offered(canopy, &offered).await {
		Ok(sql) => sql,
		Err(err) => {
			tracing::warn!("fetching the offered reporting schema failed: {err}");
			return HealOutcome::Failed;
		}
	};

	let apply = match apply_connection(&tamanu.database_url).await {
		Ok(apply) => apply,
		Err(err) => {
			tracing::warn!("opening a connection to apply the reporting schema failed: {err}");
			return HealOutcome::Failed;
		}
	};

	// The schema's own SQL drops and recreates it, so this is not additive and
	// does not need to be made so here.
	if let Err(err) = apply.batch_execute(&sql).await {
		tracing::warn!(version = %offered.version, "applying the reporting schema failed: {err}");
		return HealOutcome::Failed;
	}

	// A heal reported as healed clears the backoff, so an apply that leaves
	// the schema stamped as anything else has to report a failure: otherwise
	// the schema is dropped and rebuilt on every interval, forever.
	match read_stamp(db).await {
		Ok(Stamp::Version(stamp)) if stamp == offered.version => {
			tracing::info!(version = %offered.version, "applied reporting schema");
			HealOutcome::Healed
		}
		Ok(stamp) => {
			tracing::warn!(
				?stamp,
				offered = %offered.version,
				"the applied reporting schema did not stamp the offered version"
			);
			note_unstamped(&offered.id);
			HealOutcome::Failed
		}
		Err(err) => {
			tracing::warn!(
				"reading back the applied reporting schema's stamp failed: {}",
				fmt_db_error(&err)
			);
			HealOutcome::Failed
		}
	}
}

/// Artifacts this process has applied that did not leave the stamp they should.
fn unstamped() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
	static IDS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
		std::sync::OnceLock::new();
	IDS.get_or_init(Default::default)
}

fn applied_without_stamping(artifact: &str) -> bool {
	unstamped()
		.lock()
		.expect("unstamped registry poisoned")
		.contains(artifact)
}

fn note_unstamped(artifact: &str) {
	unstamped()
		.lock()
		.expect("unstamped registry poisoned")
		.insert(artifact.to_owned());
}

/// A connection of the apply's own, with ceilings on it.
///
/// The sweep's client is shared by every database-backed check and
/// tokio-postgres serialises what is queued on a connection, so a whole-schema
/// DDL batch on it holds up every other check for as long as the apply runs.
/// Opened through `connect_one` like every other database open in the project,
/// which is what selects TLS for a URL that asks for it. The timeouts bound a
/// batch that cannot get its locks.
async fn apply_connection(database_url: &str) -> Result<tokio_postgres::Client, miette::Report> {
	let client =
		bestool_postgres::pool::connect_one(database_url, "bestool-alertd-reporting-schema")
			.await?;
	client
		.batch_execute("SET statement_timeout = '5min'; SET lock_timeout = '30s'")
		.await
		.into_diagnostic()?;
	Ok(client)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::{
		check::CheckStatus,
		checks::test_support::{central_ctx, db_only_ctx, no_tamanu_ctx},
	};

	/// A host with no Tamanu on it has no reporting schema to be wrong about.
	#[tokio::test]
	async fn a_host_with_no_tamanu_skips() {
		let check = run(no_tamanu_ctx()).await;
		assert!(matches!(check.status, CheckStatus::Skip(_)));
	}

	/// The stamp is read from the database, so without one there is nothing to
	/// compare against canopy's offer. It skips rather than failing: a server
	/// whose database this sweep cannot reach is `db_connect`'s finding.
	#[tokio::test]
	async fn no_database_connection_skips() {
		let check = run(db_only_ctx()).await;
		assert!(matches!(check.status, CheckStatus::Skip(_)));
		assert!(!check.payload_extras.contains_key(VERSION_FACT));
	}

	/// What [`STAMP_SQL`] reads back, against a real Postgres: no `reporting`
	/// schema at all, one with no comment on it, and a stamped one. The three
	/// readings are what the check branches on, and a query that compiles can
	/// still return the wrong one of them.
	///
	/// Runs inside a transaction that is rolled back, so the database it
	/// borrows keeps whatever it had.
	#[tokio::test]
	async fn the_stamp_query_reads_the_schema_comment() {
		let Some(tamanu) = central_ctx().await else {
			return;
		};
		let db = tamanu.db.as_ref().expect("central_ctx carries a db");

		async fn stamp(db: &tokio_postgres::Client) -> Option<Option<String>> {
			db.query_opt(STAMP_SQL, &[])
				.await
				.expect("the stamp query runs")
				.map(|row| row.get::<_, Option<String>>("stamp"))
		}

		db.batch_execute("BEGIN; DROP SCHEMA IF EXISTS reporting CASCADE")
			.await
			.expect("start the transaction");
		let absent = stamp(db).await;

		db.batch_execute("CREATE SCHEMA reporting")
			.await
			.expect("create the schema");
		let unstamped = stamp(db).await;

		db.batch_execute("COMMENT ON SCHEMA reporting IS '2.60.0'")
			.await
			.expect("stamp the schema");
		let stamped = stamp(db).await;

		db.batch_execute("ROLLBACK").await.expect("roll back");

		assert_eq!(absent, None, "no reporting schema is no row");
		assert_eq!(
			unstamped,
			Some(None),
			"a schema with no comment is unstamped"
		);
		assert_eq!(stamped, Some(Some("2.60.0".to_owned())));
	}

	/// Whether the schema a server has is the offered one is canopy's to
	/// answer, so an unreachable canopy grades nothing rather than grading the
	/// server against a stamp it cannot check.
	#[tokio::test]
	async fn an_unreachable_canopy_grades_nothing() {
		let Some(tamanu) = central_ctx().await else {
			return;
		};
		let check = run(SweepContext::builder()
			.tamanu(tamanu)
			.http_client(reqwest::Client::new())
			.build())
		.await;

		assert!(
			matches!(check.status, CheckStatus::Skip(_)),
			"got {:?}",
			check.to_wire()["result"]
		);
		assert!(check.summary.contains("canopy"), "{}", check.summary);
	}

	fn http_error(status: u16) -> bestool_canopy::Error {
		bestool_canopy::Error::Http(bestool_canopy::CanopyHttpError {
			status: bestool_canopy::http::StatusCode::from_u16(status).unwrap(),
			path: "/versions/2.60.0/artifacts".to_owned(),
			body: bestool_canopy::bytes::Bytes::new(),
		})
	}

	fn artifact(kind: &str, range: Option<&str>) -> bestool_canopy::schema::Artifact {
		let mut value = serde_json::json!({
			"artifact_type": kind,
			"download_url": "https://canopy.example/s.sql",
			"id": "00000000-0000-0000-0000-000000000000",
			"platform": "any",
		});
		if let Some(range) = range {
			value["version_range_pattern"] = serde_json::Value::from(range);
		}
		serde_json::from_value(value).expect("an artifact")
	}

	/// A schema follows the migrations one version applies, so canopy publishes
	/// it for that version alone. A range artifact is resolved for every
	/// version it covers, so grading against one would call a server current on
	/// a schema built for something else.
	#[test]
	fn only_an_exact_schema_is_graded_against() {
		assert!(is_exact_schema(&artifact("reporting-schema", None)));
		assert!(!is_exact_schema(&artifact(
			"reporting-schema",
			Some("2.60.x")
		)));
	}

	/// Other artifact types share the version listing, and an installer is not
	/// a schema however it was registered.
	#[test]
	fn another_artifact_type_is_not_a_schema() {
		assert!(!is_exact_schema(&artifact("installer", None)));
		assert!(range_schema(&artifact("installer", Some("2.60.x"))).is_none());
	}

	/// A range-registered schema is worth naming: it means a build published
	/// one canopy will hand to versions it was not built for.
	#[test]
	fn a_range_registered_schema_is_named() {
		assert_eq!(
			range_schema(&artifact("reporting-schema", Some("^2.60.0"))),
			Some("^2.60.0")
		);
		assert_eq!(range_schema(&artifact("reporting-schema", None)), None);
	}

	/// A version canopy has not published has no artifacts of any kind, which
	/// it answers with a 404. That is canopy owing a build, not this server
	/// being wrong, so it must not land as a finding against the server.
	#[test]
	fn a_version_canopy_has_not_published_offers_nothing() {
		assert!(offers_nothing(&http_error(404)));
	}

	/// Anything else is the ask failing, which the server does want to hear
	/// about. Collapsing these into "nothing offered" would hide a canopy that
	/// is refusing or broken behind a silent skip.
	#[test]
	fn a_canopy_that_answered_badly_is_not_an_absent_offer() {
		for status in [401, 403, 500, 502, 503] {
			assert!(
				!offers_nothing(&http_error(status)),
				"{status} is the ask failing"
			);
		}
	}

	#[test]
	fn a_matching_stamp_passes() {
		let check = grade(&Stamp::Version("2.60.0".into()), "2.60.0");
		assert!(matches!(check.status, CheckStatus::Pass));
	}

	#[test]
	fn a_different_stamp_fails_and_names_both() {
		let check = grade(&Stamp::Version("2.59.0".into()), "2.60.0");
		assert!(matches!(check.status, CheckStatus::Fail(_)));
		// Both versions belong in the summary: which one the server is on is
		// the thing an operator needs, not just that it is wrong.
		assert!(check.summary.contains("2.59.0"), "{}", check.summary);
		assert!(check.summary.contains("2.60.0"), "{}", check.summary);
	}

	#[test]
	fn no_schema_at_all_fails() {
		let check = grade(&Stamp::NoSchema, "2.60.0");
		assert!(matches!(check.status, CheckStatus::Fail(_)));
	}

	/// A `reporting` schema with no comment on it is a different finding from
	/// having none: something built it that was not this pipeline, and the
	/// summary has to say so or an operator reads "no reporting schema" against
	/// a server whose reports are reading from one.
	#[test]
	fn an_unstamped_schema_is_not_an_absent_one() {
		let unstamped = grade(&Stamp::Unstamped, "2.60.0");
		let absent = grade(&Stamp::NoSchema, "2.60.0");

		assert!(matches!(unstamped.status, CheckStatus::Fail(_)));
		assert_ne!(unstamped.summary, absent.summary);
		assert!(
			unstamped.summary.contains("2.60.0"),
			"{}",
			unstamped.summary
		);

		// Neither reports a version: there is none to report, and a stale fact
		// would read as a server sitting on a schema it no longer has.
		assert!(
			!with_version(unstamped, &Stamp::Unstamped)
				.payload_extras
				.contains_key(VERSION_FACT)
		);
	}

	#[test]
	fn the_stamp_is_reported_even_when_it_is_wrong() {
		// A server on the wrong schema is exactly when knowing which one it
		// has matters, so the fact rides along with a failure too.
		let check = with_version(
			grade(&Stamp::Version("2.59.0".into()), "2.60.0"),
			&Stamp::Version("2.59.0".into()),
		);
		assert_eq!(
			check.payload_extras.get(VERSION_FACT),
			Some(&serde_json::Value::from("2.59.0"))
		);
	}

	/// The offer's `download_url` is not what is asked for. A path is resolved
	/// against the transport's own base, so one canopy names could carry an
	/// authority and take the device credential with it.
	#[test]
	fn the_download_path_is_built_from_the_artifact_s_id() {
		assert_eq!(
			download_path("2.60.0", "00000000-0000-0000-0000-000000000000"),
			"/versions/2.60.0/artifacts/00000000-0000-0000-0000-000000000000/download"
		);
	}

	/// The comment is arbitrary text anyone with COMMENT rights on the schema
	/// can set, and it rides to canopy as a status fact, so what is not
	/// plausibly a version reads as no stamp rather than being carried.
	#[test]
	fn a_comment_that_is_not_a_version_is_not_a_stamp() {
		assert_eq!(stamp_of("  2.60.0 ".to_owned()), Some("2.60.0".to_owned()));
		assert_eq!(stamp_of("   ".to_owned()), None);
		assert_eq!(stamp_of("x".repeat(MAX_STAMP_LEN + 1)), None);
		assert_eq!(stamp_of("built by hand".to_owned()), None);
		assert_eq!(
			stamp_of("2.60.0\n<script>alert(1)</script>".to_owned()),
			None
		);
	}

	/// An answer that would not decode has no status either, and reading it as
	/// an unreachable canopy stops the grading fleet-wide with nothing to say
	/// the check stopped working.
	#[test]
	fn an_answer_that_would_not_decode_is_not_an_unreachable_canopy() {
		assert!(!canopy_is_out(&bestool_canopy::Error::Decode {
			path: "/versions/2.60.0/artifacts".to_owned(),
			source: serde_json::from_str::<u8>("[]").expect_err("a decode error"),
		}));
		assert!(canopy_is_out(&bestool_canopy::Error::transport(
			std::io::Error::other("no route to host")
		)));
	}

	/// A canopy that never answered, or answered with a fault of its own, is
	/// not a finding against this server: it would raise the same one on every
	/// server in the fleet.
	#[test]
	fn a_canopy_fault_is_not_graded_against_the_server() {
		for status in [500, 502, 503] {
			assert!(canopy_is_out(&http_error(status)), "{status} is canopy's");
		}
		for status in [401, 403, 404] {
			assert!(!canopy_is_out(&http_error(status)), "{status} is an answer");
		}
	}

	#[test]
	fn a_server_with_no_schema_reports_no_version() {
		let check = with_version(grade(&Stamp::NoSchema, "2.60.0"), &Stamp::NoSchema);
		assert!(!check.payload_extras.contains_key(VERSION_FACT));
	}
}
