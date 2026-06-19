use std::collections::HashMap;

use miette::{IntoDiagnostic, Result, WrapErr, miette};
use url::Url;

/// Environment variable that overrides the database connection otherwise read
/// from the Tamanu config. Holds a full `postgresql://…` URL.
///
/// Honoured by every bestool command that connects to or emits the Tamanu
/// database connection (alertd, doctor, logs, lifecycle, psql, db_url, backup,
/// greenmask). When set, the command does not need the config's `db` block —
/// and alertd does not need a Tamanu install at all.
pub const DATABASE_URL_ENV: &str = "TAMANU_DATABASE_URL";

/// The [`DATABASE_URL_ENV`] override, if set to a non-empty value.
pub fn database_url_override() -> Option<String> {
	std::env::var(DATABASE_URL_ENV)
		.ok()
		.filter(|s| !s.is_empty())
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TamanuConfig {
	pub canonical_host_name: Option<Url>,
	pub canonical_url: Option<Url>,
	/// Current (multi-facility) form. Newer installs only.
	pub server_facility_ids: Option<Vec<String>>,
	/// Legacy single-facility form. Still in use on older facility installs.
	pub server_facility_id: Option<String>,
	pub db: Database,
	pub mailgun: Option<Mailgun>,
	pub primary_time_zone: Option<String>,
	pub country_time_zone: Option<String>,
	#[serde(default)]
	pub integrations: Integrations,
	/// `sync` block. Only configured on facility servers — central servers have
	/// no sync target, so its presence is a reliable "is this a facility" signal.
	pub sync: Option<Sync>,
}

impl TamanuConfig {
	pub fn canonical_url(&self) -> Option<&Url> {
		self.canonical_host_name
			.as_ref()
			.or(self.canonical_url.as_ref())
	}

	/// Identify the server as a facility from any one of three independent
	/// config signals.
	///
	/// Central servers have *none* of these populated; any one being present is
	/// taken as facility. We check all three because real-world facility
	/// installs vary by Tamanu vintage:
	///
	///   * `serverFacilityIds` (plural) — current multi-facility form
	///   * `serverFacilityId` (singular) — legacy single-facility form, still
	///     in active use on older facility installs
	///   * `sync.host` — only configured on facilities (centrals have no
	///     upstream to sync to)
	pub fn is_facility(&self) -> bool {
		self.server_facility_ids
			.as_ref()
			.is_some_and(|ids| !ids.is_empty())
			|| self
				.server_facility_id
				.as_ref()
				.is_some_and(|id| !id.is_empty())
			|| self.sync.as_ref().is_some_and(|s| s.host.is_some())
	}

	/// Mirrors `getPrimaryTimeZone()` in Tamanu: prefers `primaryTimeZone`,
	/// falls back to `countryTimeZone`.
	pub fn primary_time_zone(&self) -> Option<&str> {
		self.primary_time_zone
			.as_deref()
			.or(self.country_time_zone.as_deref())
	}

	pub fn fhir_worker_enabled(&self) -> bool {
		self.integrations.fhir.worker.enabled
	}

	/// Build the base URL for the facility sync sub-process's API
	/// (`sync.syncApiConnection`). Returns `None` when the config has no
	/// `sync` block at all (i.e. central server) — callers should check
	/// [`Self::is_facility`] first to give a better error.
	///
	/// The returned URL has no path; routes are `sync/run` and `sync/status`.
	pub fn sync_api_url(&self) -> Option<Url> {
		let conn = self
			.sync
			.as_ref()?
			.sync_api_connection
			.clone()
			.unwrap_or_default();
		let raw = format!("{}:{}", conn.host().trim_end_matches('/'), conn.port());
		Url::parse(&raw).ok()
	}

	/// Build the `postgresql://` URL from `db` for the canonical
	/// username/password the local Tamanu install uses. Hosts default to
	/// `localhost`, port defers to the URL's libpq default when unset.
	///
	/// When [`DATABASE_URL_ENV`] is set, its value is returned verbatim
	/// instead — preserving any query parameters (e.g. `sslmode`) the operator
	/// included.
	///
	/// Subcommands that need a different role (e.g. psql's report-schema
	/// connections) build their own `ConnectionUrlBuilder` instead.
	pub fn database_url(&self) -> String {
		if let Some(url) = database_url_override() {
			return url;
		}
		crate::connection_url::ConnectionUrlBuilder {
			username: self.db.username.clone(),
			password: Some(self.db.password.clone()),
			host: self
				.db
				.host
				.clone()
				.unwrap_or_else(|| "localhost".to_string()),
			port: self.db.port,
			database: self.db.name.clone(),
			ssl_mode: None,
		}
		.build()
	}

	/// The effective database connection details: parsed from
	/// [`DATABASE_URL_ENV`] when set, otherwise the config's own `db` block.
	///
	/// Used by subcommands that need the individual fields (host, user,
	/// password, name) rather than a URL — e.g. backup's `pg_dump`, greenmask,
	/// and psql/db_url's report-schema handling.
	pub fn database(&self) -> Result<Database> {
		match database_url_override() {
			Some(url) => Database::from_url(&url),
			None => Ok(self.db.clone()),
		}
	}

	/// A config carrying only a database section, for callers that have a
	/// database URL but no Tamanu install to read (see [`DATABASE_URL_ENV`]).
	pub fn from_database(db: Database) -> Self {
		Self {
			canonical_host_name: None,
			canonical_url: None,
			server_facility_ids: None,
			server_facility_id: None,
			db,
			mailgun: None,
			primary_time_zone: None,
			country_time_zone: None,
			integrations: Integrations::default(),
			sync: None,
		}
	}
}

/// The `integrations` block of the Tamanu config.
///
/// Tamanu groups all third-party / optional subsystem toggles here. The only
/// field we currently care about is `fhir` (so we know whether the FHIR worker
/// services should be Up or Down), but the struct exists as its own type so
/// future integration probes have an obvious home.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Integrations {
	pub fhir: Fhir,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sync {
	/// URL of the central server this facility syncs against. Only set on
	/// facility servers.
	pub host: Option<Url>,
	/// Facility-only: connection details for the sync sub-process's
	/// `POST /sync/run` / `GET /sync/status` API. Bound to localhost by
	/// default and not authed, so callers must be local to the box.
	pub sync_api_connection: Option<SyncApiConnection>,
}

/// Facility sync sub-process address (`sync.syncApiConnection`).
///
/// Mirrors `tamanu/packages/facility-server/config/default.json5` —
/// `host` defaults to `http://localhost` and `port` to `4100` when the
/// block is absent or only partly filled.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncApiConnection {
	pub host: Option<String>,
	pub port: Option<u16>,
}

impl SyncApiConnection {
	pub const DEFAULT_HOST: &'static str = "http://localhost";
	pub const DEFAULT_PORT: u16 = 4100;

	pub fn host(&self) -> &str {
		self.host.as_deref().unwrap_or(Self::DEFAULT_HOST)
	}

	pub fn port(&self) -> u16 {
		self.port.unwrap_or(Self::DEFAULT_PORT)
	}
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Fhir {
	pub worker: FhirWorker,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FhirWorker {
	pub enabled: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Database {
	pub host: Option<String>,
	pub port: Option<u16>,
	pub name: String,
	pub username: String,
	pub password: String,
	pub report_schemas: Option<ReportSchemas>,
}

impl Database {
	/// Parse a `postgresql://…` URL (or libpq key/value string) into the
	/// database fields, via tokio-postgres's own connection-string parser so
	/// every form it accepts at connect time — including Unix sockets and
	/// percent-encoded credentials — parses the same way here.
	///
	/// Report-schema credentials are config-only and have no place in a URL, so
	/// the result always has `report_schemas: None`.
	pub fn from_url(url: &str) -> Result<Self> {
		let parsed: tokio_postgres::Config = url
			.parse()
			.into_diagnostic()
			.wrap_err_with(|| format!("parsing {DATABASE_URL_ENV}"))?;

		// tokio-postgres allows multiple hosts/ports for failover; bestool only
		// ever connects to one, so take the first of each.
		let host = parsed.get_hosts().first().map(|h| match h {
			tokio_postgres::config::Host::Tcp(h) => h.clone(),
			#[cfg(unix)]
			tokio_postgres::config::Host::Unix(p) => p.display().to_string(),
		});
		let port = parsed.get_ports().first().copied();
		let name = parsed
			.get_dbname()
			.filter(|n| !n.is_empty())
			.ok_or_else(|| miette!("{DATABASE_URL_ENV} must include a database name"))?
			.to_owned();

		Ok(Self {
			host,
			port,
			name,
			username: parsed.get_user().unwrap_or_default().to_owned(),
			password: parsed
				.get_password()
				.map(|p| String::from_utf8_lossy(p).into_owned())
				.unwrap_or_default(),
			report_schemas: None,
		})
	}
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReportSchemas {
	pub connections: HashMap<String, ReportConnection>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ReportConnection {
	pub username: String,
	pub password: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mailgun {
	pub domain: String,
	pub api_key: String,

	#[serde(rename = "from")]
	pub sender: String,
}

#[cfg(test)]
mod tests {
	use super::*;

	fn parse(json: serde_json::Value) -> TamanuConfig {
		serde_json::from_value(json).expect("test config should parse")
	}

	fn base() -> serde_json::Value {
		serde_json::json!({
			"db": { "name": "x", "username": "u", "password": "p" },
		})
	}

	#[test]
	fn central_has_no_facility_signals() {
		assert!(!parse(base()).is_facility());
	}

	#[test]
	fn server_facility_ids_plural_marks_facility() {
		let mut json = base();
		json["serverFacilityIds"] = serde_json::json!(["facility-x"]);
		assert!(parse(json).is_facility());
	}

	#[test]
	fn empty_server_facility_ids_does_not_mark_facility() {
		let mut json = base();
		json["serverFacilityIds"] = serde_json::json!([]);
		assert!(!parse(json).is_facility());
	}

	#[test]
	fn legacy_singular_server_facility_id_marks_facility() {
		// Older facility installs use the singular form. This was the case
		// that misclassified a real facility as central in production.
		let mut json = base();
		json["serverFacilityId"] = serde_json::json!("facility-x");
		assert!(parse(json).is_facility());
	}

	#[test]
	fn empty_singular_server_facility_id_does_not_mark_facility() {
		let mut json = base();
		json["serverFacilityId"] = serde_json::json!("");
		assert!(!parse(json).is_facility());
	}

	#[test]
	fn sync_host_marks_facility() {
		// Central servers have no upstream to sync to; presence of `sync.host`
		// is a positive facility signal independent of the serverFacilityId(s)
		// fields.
		let mut json = base();
		json["sync"] = serde_json::json!({ "host": "https://central.example.org" });
		assert!(parse(json).is_facility());
	}

	#[test]
	fn empty_sync_block_does_not_mark_facility() {
		let mut json = base();
		json["sync"] = serde_json::json!({});
		assert!(!parse(json).is_facility());
	}

	#[test]
	fn fhir_worker_enabled_reads_integrations_path() {
		let mut json = base();
		json["integrations"] = serde_json::json!({ "fhir": { "worker": { "enabled": true } } });
		assert!(parse(json).fhir_worker_enabled());
	}

	#[test]
	fn fhir_worker_disabled_when_integrations_says_false() {
		let mut json = base();
		json["integrations"] = serde_json::json!({ "fhir": { "worker": { "enabled": false } } });
		assert!(!parse(json).fhir_worker_enabled());
	}

	#[test]
	fn fhir_worker_disabled_when_integrations_missing() {
		assert!(!parse(base()).fhir_worker_enabled());
	}

	#[test]
	fn sync_api_url_missing_block_is_none() {
		assert!(parse(base()).sync_api_url().is_none());
	}

	#[test]
	fn sync_api_url_defaults_when_block_empty() {
		let mut json = base();
		json["sync"] = serde_json::json!({});
		assert_eq!(
			parse(json).sync_api_url().unwrap().as_str(),
			"http://localhost:4100/"
		);
	}

	#[test]
	fn sync_api_url_uses_configured_port() {
		let mut json = base();
		json["sync"] = serde_json::json!({
			"syncApiConnection": { "port": 4200 },
		});
		assert_eq!(
			parse(json).sync_api_url().unwrap().as_str(),
			"http://localhost:4200/"
		);
	}

	#[test]
	fn sync_api_url_uses_configured_host_and_port() {
		let mut json = base();
		json["sync"] = serde_json::json!({
			"syncApiConnection": { "host": "http://127.0.0.1", "port": 9999 },
		});
		assert_eq!(
			parse(json).sync_api_url().unwrap().as_str(),
			"http://127.0.0.1:9999/"
		);
	}

	#[test]
	fn sync_api_url_strips_trailing_slash_in_host() {
		// `host` in the default config is "http://localhost" (no trailing
		// slash), but operators occasionally paste in a URL with one. The
		// facility-server itself strips trailing slashes before concatenating
		// the port, so we do the same.
		let mut json = base();
		json["sync"] = serde_json::json!({
			"syncApiConnection": { "host": "http://localhost/" },
		});
		assert_eq!(
			parse(json).sync_api_url().unwrap().as_str(),
			"http://localhost:4100/"
		);
	}

	#[test]
	fn legacy_top_level_fhir_field_is_ignored() {
		// The old (wrong) location. We don't honour it — the actual schema
		// puts this under `integrations.fhir.worker.enabled`, and pretending
		// the top-level key still works would hide real misconfigurations.
		let mut json = base();
		json["fhir"] = serde_json::json!({ "worker": { "enabled": true } });
		assert!(!parse(json).fhir_worker_enabled());
	}

	#[test]
	fn database_from_url_parses_tcp() {
		let db = Database::from_url("postgresql://user:pass@db.example:5544/tamanu").unwrap();
		assert_eq!(db.host.as_deref(), Some("db.example"));
		assert_eq!(db.port, Some(5544));
		assert_eq!(db.name, "tamanu");
		assert_eq!(db.username, "user");
		assert_eq!(db.password, "pass");
		assert!(db.report_schemas.is_none());
	}

	#[test]
	fn database_from_url_decodes_userinfo() {
		let db = Database::from_url("postgresql://u%40d:p%40ss%2Fword@localhost/tamanu").unwrap();
		assert_eq!(db.username, "u@d");
		assert_eq!(db.password, "p@ss/word");
	}

	#[test]
	fn database_from_url_unix_socket_host_query() {
		let db =
			Database::from_url("postgresql://user:pass@/tamanu?host=/var/run/postgresql").unwrap();
		assert_eq!(db.host.as_deref(), Some("/var/run/postgresql"));
		assert_eq!(db.name, "tamanu");
	}

	#[test]
	fn database_from_url_defaults_port_to_libpq_default() {
		// tokio-postgres fills the libpq default (5432) when the URL omits a
		// port. Making it explicit is harmless — it's the port a portless URL
		// would have connected to anyway.
		let db = Database::from_url("postgresql://user:pass@localhost/tamanu").unwrap();
		assert_eq!(db.port, Some(5432));
	}

	#[test]
	fn database_from_url_rejects_wrong_scheme() {
		assert!(Database::from_url("mysql://user@localhost/tamanu").is_err());
	}

	#[test]
	fn database_from_url_requires_database_name() {
		assert!(Database::from_url("postgresql://user:pass@localhost/").is_err());
	}
}
