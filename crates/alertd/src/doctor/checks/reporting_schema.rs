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

use bestool_canopy::{CanopyClient, reqwest::Url};
use miette::IntoDiagnostic as _;

use super::SweepContext;
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

	let Some(db) = tamanu.db.as_ref() else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let running = match db.query_opt(STAMP_SQL, &[]).await {
		Ok(Some(row)) => row.get::<_, Option<String>>("stamp"),
		// No `reporting` schema at all. Not an error: a server that has never
		// had one applied is exactly what this check exists to surface.
		Ok(None) => None,
		Err(err) => {
			return Check::broken(
				NAME,
				"could not read the reporting schema",
				format!("reading the schema's version stamp failed: {err}"),
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
			running.as_deref(),
		);
	};

	let offered = match offered_schema(canopy, &tamanu.tamanu_version.to_string()).await {
		Ok(offered) => offered,
		Err(err) => {
			return with_version(
				Check::warning(
					NAME,
					"could not ask canopy what is offered",
					format!("fetching the offered reporting schema failed: {err}"),
				),
				running.as_deref(),
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
			running.as_deref(),
		);
	};

	with_version(
		grade(running.as_deref(), &offered.version),
		running.as_deref(),
	)
}

/// What the stamp on the server says against what canopy offers.
///
/// Separated from the sweep because this is the whole judgement the check
/// makes, and it is worth being able to state it without a database.
fn grade(running: Option<&str>, offered: &str) -> Check {
	match running {
		Some(stamp) if stamp == offered => Check::pass(NAME, format!("reporting schema {stamp}")),
		Some(stamp) => Check::fail(
			NAME,
			format!("reporting schema {stamp}, offered {offered}"),
			"the server's reports read from a schema built for a different version",
		),
		None => Check::fail(
			NAME,
			"no reporting schema",
			"canopy offers one for the version this server runs, and the server has none",
		),
	}
}

/// Carry the stamp as a top-level status fact, so the fleet view can show which
/// schema a server is on without reading into the check's own detail.
fn with_version(check: Check, running: Option<&str>) -> Check {
	match running {
		Some(version) => check.with_payload_extra(VERSION_FACT, serde_json::Value::from(version)),
		None => check,
	}
}

/// A reporting schema canopy offers, and the version it was built for.
struct Offered {
	version: String,
	download_url: String,
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
			download_url: a.download_url,
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
		.then(|| artifact.version_range_pattern.as_deref())
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
	let path = offered_path(&offered.download_url)?;

	canopy
		.transport()
		.get(&format!("/public{path}"), &path)
		.await?
		.error_for_status()
		.into_diagnostic()?
		.text()
		.await
		.into_diagnostic()
}

/// The path to ask the transport for, taken from the URL canopy offered.
///
/// The transport addresses canopy by path so that it reaches whichever of the
/// two endpoints holds the credential, and over tailscale the public API is
/// mounted a level down, so the origin canopy names in the offer is dropped.
fn offered_path(download_url: &str) -> Result<String, miette::Report> {
	let url = Url::parse(download_url).into_diagnostic()?;
	Ok(match url.query() {
		Some(query) => format!("{}?{query}", url.path()),
		None => url.path().to_owned(),
	})
}

/// Apply the schema canopy offers.
///
/// Applying is the one thing on this host that writes to Tamanu's database, so
/// it lives here rather than in the check: heal runs only in the daemon, only
/// when the check graded a failure, and behind the shared backoff.
pub async fn heal(ctx: SweepContext) -> HealOutcome {
	let Some(tamanu) = ctx.tamanu.as_ref() else {
		return HealOutcome::Deferred;
	};
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

	let sql = match fetch_offered(canopy, &offered).await {
		Ok(sql) => sql,
		Err(err) => {
			tracing::warn!("fetching the offered reporting schema failed: {err}");
			return HealOutcome::Failed;
		}
	};

	// The schema's own SQL drops and recreates it, so this is not additive and
	// does not need to be made so here.
	match db.batch_execute(&sql).await {
		Ok(()) => {
			tracing::info!(version = %offered.version, "applied reporting schema");
			HealOutcome::Healed
		}
		Err(err) => {
			tracing::warn!(version = %offered.version, "applying the reporting schema failed: {err}");
			HealOutcome::Failed
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::check::CheckStatus;

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
		let check = grade(Some("2.60.0"), "2.60.0");
		assert!(matches!(check.status, CheckStatus::Pass));
	}

	#[test]
	fn a_different_stamp_fails_and_names_both() {
		let check = grade(Some("2.59.0"), "2.60.0");
		assert!(matches!(check.status, CheckStatus::Fail(_)));
		// Both versions belong in the summary: which one the server is on is
		// the thing an operator needs, not just that it is wrong.
		assert!(check.summary.contains("2.59.0"), "{}", check.summary);
		assert!(check.summary.contains("2.60.0"), "{}", check.summary);
	}

	#[test]
	fn no_schema_at_all_fails() {
		let check = grade(None, "2.60.0");
		assert!(matches!(check.status, CheckStatus::Fail(_)));
	}

	#[test]
	fn the_stamp_is_reported_even_when_it_is_wrong() {
		// A server on the wrong schema is exactly when knowing which one it
		// has matters, so the fact rides along with a failure too.
		let check = with_version(grade(Some("2.59.0"), "2.60.0"), Some("2.59.0"));
		assert_eq!(
			check.payload_extras.get(VERSION_FACT),
			Some(&serde_json::Value::from("2.59.0"))
		);
	}

	#[test]
	fn the_offer_s_origin_is_dropped_in_favour_of_the_transport_s() {
		assert_eq!(
			offered_path("https://meta.example/versions/2.60.0/artifacts/abc/download").unwrap(),
			"/versions/2.60.0/artifacts/abc/download"
		);
	}

	#[test]
	fn a_server_with_no_schema_reports_no_version() {
		let check = with_version(grade(None, "2.60.0"), None);
		assert!(!check.payload_extras.contains_key(VERSION_FACT));
	}
}
