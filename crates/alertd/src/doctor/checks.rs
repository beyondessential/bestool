//! Doctor healthchecks. One module per check.
//!
//! Each module exposes a `pub async fn run(ctx: &CheckContext) -> Check` (or a
//! sync `fn run(...) -> Check` where async is unnecessary). The `ALL` registry
//! below ties names to runners so the dispatcher can filter by `--check`.

use std::{path::PathBuf, sync::Arc};

use node_semver::Version;
use tokio_postgres::Client as PgClient;

use bestool_tamanu::{ApiServerKind, config::TamanuConfig};

use super::check::Check;

pub mod util;

pub mod caddy_version;
pub mod certificate_notification_errors;
pub mod db_connect;
pub mod db_version;
pub mod disk_free;
pub mod external_users;
pub mod fhir_job_errors;
pub mod fhir_jobs;
pub mod fhir_service_requests_unresolved;
pub mod http_errors;
pub mod ips_errors;
pub mod kopia_backup;
pub mod load;
pub mod memory;
pub mod migrations;
pub mod patient_communication_errors;
pub mod report_errors;
pub mod server_id;
pub mod sync_facility_stale;
pub mod sync_lookup;
pub mod sync_restart_loop;
pub mod sync_session_errors;
pub mod sync_sessions;
pub mod tailscale;
pub mod tamanu_found;
pub mod tamanu_http;
pub mod tamanu_service;
pub mod time_sync;
pub mod uptime;
pub mod version_drift;

/// Shared context handed to every check.
///
/// Each check picks the fields it needs and ignores the rest. The DB client is
/// `Option` because not every check needs the DB, and `db_connect` itself runs
/// before the client is available. The `http_client` is shared across checks
/// and across the daemon's other consumers so TCP/TLS connections stay warm
/// between ticks; HTTP checks apply per-request timeouts via
/// `RequestBuilder::timeout`.
#[derive(Clone)]
pub struct CheckContext {
	pub tamanu_version: Version,
	pub tamanu_root: PathBuf,
	pub config: Arc<TamanuConfig>,
	/// Whether this install is a central or facility server. Determined once
	/// at doctor startup from the most authoritative available signals (DB
	/// `local_system_facts` first, then config), then shared so checks don't
	/// each have to re-decide.
	pub kind: ApiServerKind,
	pub database_url: String,
	pub db: Option<Arc<PgClient>>,
	pub http_client: reqwest::Client,
}

/// `tokio_postgres::Error`'s top-level Display is the unhelpful `"db error"`.
/// The actual SQL message lives in the optional `DbError` underneath; this
/// helper surfaces it where present and falls back to the source chain.
pub fn fmt_db_error(err: &tokio_postgres::Error) -> String {
	if let Some(db) = err.as_db_error() {
		let mut s = format!("{}: {}", db.severity(), db.message());
		if let Some(detail) = db.detail() {
			s.push_str(" — ");
			s.push_str(detail);
		}
		return s;
	}

	fmt_chain(err)
}

/// Build the Check for a query that errored, classified by SQLSTATE.
///
/// Class 42 ("syntax error or access rule violation": dropped or renamed
/// columns, json/jsonb drift, missing functions) means the check's own SQL no
/// longer matches the schema — a fault in the healthcheck, not the deployment
/// — so it reports as BROKEN rather than flagging the server as failing.
/// Everything else stays FAIL.
pub fn query_error_check(name: &'static str, err: &tokio_postgres::Error) -> Check {
	let reason = fmt_db_error(err);
	if err
		.as_db_error()
		.is_some_and(|db| db.code().code().starts_with("42"))
	{
		Check::broken(name, "healthcheck query broken", reason)
	} else {
		Check::fail(name, "query failed", reason)
	}
}

/// Walk a `std::error::Error`'s source chain and join all the messages.
///
/// `reqwest::Error`'s Display is just "error sending request for url (...)";
/// the actual cause (DNS error, connection refused, timed out, proxy failure)
/// is one or two `.source()` calls down. Without walking the chain, doctor
/// `FAIL` rows lose the only diagnostic that matters.
pub fn fmt_chain<E: std::error::Error + ?Sized>(err: &E) -> String {
	use std::error::Error;

	let mut parts = vec![err.to_string()];
	let mut src: Option<&dyn Error> = err.source();
	while let Some(s) = src {
		parts.push(s.to_string());
		src = s.source();
	}
	parts.join(": ")
}

/// One check's name + runner.
pub struct CheckEntry {
	pub name: &'static str,
	/// `false` means the check is rendered to the CLI but NOT included in the
	/// canopy `health[]` wire array (e.g. `tailscale`, which canopy already
	/// tracks elsewhere).
	pub on_wire: bool,
	pub run: fn(CheckContext) -> futures::future::BoxFuture<'static, Check>,
}

macro_rules! entry {
	($name:literal, $module:ident) => {
		CheckEntry {
			name: $name,
			on_wire: true,
			run: |ctx| Box::pin($module::run(ctx)),
		}
	};
	($name:literal, $module:ident, off_wire) => {
		CheckEntry {
			name: $name,
			on_wire: false,
			run: |ctx| Box::pin($module::run(ctx)),
		}
	};
}

/// Registry of every check the doctor knows how to run.
///
/// Order here is the order they appear in the CLI render.
pub fn all() -> Vec<CheckEntry> {
	vec![
		entry!("tamanu_found", tamanu_found),
		entry!("db_connect", db_connect),
		entry!("db_version", db_version),
		entry!("server_id", server_id),
		entry!("migrations", migrations),
		entry!("disk_free", disk_free),
		entry!("memory", memory),
		entry!("load", load),
		entry!("uptime", uptime),
		entry!("time_sync", time_sync),
		entry!("tamanu_http", tamanu_http),
		entry!("caddy_version", caddy_version),
		entry!("http_errors", http_errors),
		entry!("tailscale", tailscale, off_wire),
		entry!("tamanu_service", tamanu_service),
		entry!("version_drift", version_drift),
		entry!("external_users", external_users),
		entry!("sync_sessions", sync_sessions),
		entry!("fhir_jobs", fhir_jobs),
		entry!("kopia_backup", kopia_backup),
		entry!(
			"certificate_notification_errors",
			certificate_notification_errors
		),
		entry!("ips_errors", ips_errors),
		entry!("patient_communication_errors", patient_communication_errors),
		entry!("report_errors", report_errors),
		entry!("fhir_job_errors", fhir_job_errors),
		entry!("sync_session_errors", sync_session_errors),
		entry!("sync_facility_stale", sync_facility_stale),
		entry!("sync_lookup", sync_lookup),
		entry!("sync_restart_loop", sync_restart_loop),
		entry!(
			"fhir_service_requests_unresolved",
			fhir_service_requests_unresolved
		),
	]
}

#[cfg(test)]
pub mod test_support {
	//! Helpers for DB-backed check tests.
	//!
	//! Each check is central-only and DB-backed, so its tests need a
	//! [`CheckContext`] wired to one of the local `tamanu-central` /
	//! `tamanu-facility` databases. These connect lazily and return `None` when
	//! the DB is unavailable so the suite degrades gracefully off-CI.

	use std::sync::Arc;

	use node_semver::Version;

	use bestool_tamanu::{ApiServerKind, config::TamanuConfig};

	use super::CheckContext;

	fn central_config() -> TamanuConfig {
		serde_json::from_value(serde_json::json!({
			"db": { "name": "tamanu-central", "username": "u", "password": "p" },
		}))
		.expect("central test config should parse")
	}

	fn facility_config() -> TamanuConfig {
		serde_json::from_value(serde_json::json!({
			"db": { "name": "tamanu-facility", "username": "u", "password": "p" },
			"serverFacilityIds": ["facility-1"],
		}))
		.expect("facility test config should parse")
	}

	async fn connect(db_name: &str) -> Option<Arc<tokio_postgres::Client>> {
		let url = format!("postgresql://localhost/{db_name}");
		match bestool_postgres::pool::connect_one(&url, "bestool-alertd-test").await {
			Ok(client) => Some(Arc::new(client)),
			Err(_) => None,
		}
	}

	/// A central [`CheckContext`] backed by `tamanu-central`, or `None` if that
	/// DB can't be reached.
	pub async fn central_ctx() -> Option<CheckContext> {
		let db = connect("tamanu-central").await?;
		Some(CheckContext {
			tamanu_version: Version::parse("0.0.0").unwrap(),
			tamanu_root: std::path::PathBuf::from("/nonexistent"),
			config: Arc::new(central_config()),
			kind: ApiServerKind::Central,
			database_url: "postgresql://localhost/tamanu-central".into(),
			db: Some(db),
			http_client: reqwest::Client::new(),
		})
	}

	/// A facility [`CheckContext`] with no DB; central-only checks skip on it
	/// before ever touching the database.
	pub fn facility_ctx() -> CheckContext {
		CheckContext {
			tamanu_version: Version::parse("0.0.0").unwrap(),
			tamanu_root: std::path::PathBuf::from("/nonexistent"),
			config: Arc::new(facility_config()),
			kind: ApiServerKind::Facility,
			database_url: "postgresql://localhost/tamanu-facility".into(),
			db: None,
			http_client: reqwest::Client::new(),
		}
	}
}

#[cfg(test)]
mod tests {
	use serde_json::Value;

	use super::{query_error_check, test_support::central_ctx};
	use crate::doctor::check::CheckStatus;

	async fn query_err(sql: &str) -> Option<tokio_postgres::Error> {
		let ctx = central_ctx().await?;
		let client = ctx.db.expect("central_ctx always has a client");
		Some(
			client
				.query(sql, &[])
				.await
				.expect_err("query should error"),
		)
	}

	#[tokio::test]
	async fn schema_drift_is_broken() {
		// 42P01 undefined_table — the shape a dropped/renamed relation takes.
		let Some(err) = query_err("SELECT nope FROM no_such_table_bestool_test").await else {
			return;
		};
		let check = query_error_check("x", &err);
		assert!(matches!(check.status, CheckStatus::Broken(_)));
		assert_eq!(check.to_wire()["result"], Value::from("broken"));
	}

	#[tokio::test]
	async fn syntax_error_is_broken() {
		// 42601 syntax_error.
		let Some(err) = query_err("SELECT FROM WHERE").await else {
			return;
		};
		let check = query_error_check("x", &err);
		assert!(matches!(check.status, CheckStatus::Broken(_)));
	}

	#[tokio::test]
	async fn runtime_db_error_still_fails() {
		// 22012 division_by_zero — not class 42, so the deployment is blamed.
		let Some(err) = query_err("SELECT 1/0").await else {
			return;
		};
		let check = query_error_check("x", &err);
		assert!(check.status.is_fatal());
		assert_eq!(check.to_wire()["result"], Value::from("failed"));
	}
}
