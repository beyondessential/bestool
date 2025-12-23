use std::{collections::{BTreeMap, HashSet}, path::PathBuf, sync::Arc, time::Duration};

use bestool_psql::column_extractor::ColumnRef;
use bestool_psql::SnippetLookupProvider;
use clap::{Parser, ValueEnum};
use miette::{IntoDiagnostic as _, Result, WrapErr, bail};
use serde_json::Value;
use tokio::{fs, sync::RwLock, time::timeout};
use tracing::{debug, info, instrument, warn};

use crate::actions::Context;
use crate::download::{DownloadSource, reqwest_client};

use super::{TamanuArgs, config::load_config, connection_url::ConnectionUrlBuilder, find_tamanu};

/// Asynchronous snippet provider that fetches snippets from an API.
///
/// Snippets are cached to disk. On startup, cached snippets are loaded immediately,
/// then the remote API is fetched in the background to update the cache.
struct AsyncSnippetProvider {
	snippets: Arc<RwLock<Option<BTreeMap<String, String>>>>,
}

impl AsyncSnippetProvider {
	fn new() -> Self {
		Self {
			snippets: Arc::new(RwLock::new(None)),
		}
	}

	/// Get the cache file path
	fn cache_path() -> Result<std::path::PathBuf> {
		if let Some(cache_dir) = dirs::cache_dir() {
			let path = cache_dir.join("bestool").join("snippets.json");
			Ok(path)
		} else {
			Err(miette::miette!("Unable to determine cache directory"))
		}
	}

	/// Load snippets from cache file
	fn load_from_cache(&self) -> Result<()> {
		let path = Self::cache_path()?;
		if !path.exists() {
			return Ok(());
		}

		let content = std::fs::read_to_string(&path).into_diagnostic()?;
		let snippets_json: serde_json::Value = serde_json::from_str(&content).into_diagnostic()?;

		let mut snippets = BTreeMap::new();
		if let Some(obj) = snippets_json.as_object() {
			for (name, snippet_data) in obj {
				if let Some(sql) = snippet_data.get("sql").and_then(|v| v.as_str()) {
					snippets.insert(name.clone(), sql.to_string());
				}
			}
		}

		let count = snippets.len();
		let mut cached = self.snippets.blocking_write();
		*cached = Some(snippets);
		info!(count, "loaded snippets from cache file");

		Ok(())
	}

	/// Save snippets to cache file
	async fn save_to_cache(&self, snippets_json: &serde_json::Value) -> Result<()> {
		let path = Self::cache_path()?;
		if let Some(parent) = path.parent() {
			tokio::fs::create_dir_all(parent).await.into_diagnostic()?;
		}

		let count = snippets_json
			.as_object()
			.map(|obj| obj.len())
			.unwrap_or(0);
		let json_str = serde_json::to_string(snippets_json).into_diagnostic()?;
		tokio::fs::write(&path, json_str).await.into_diagnostic()?;
		info!(count, "saved snippets to cache file");

		Ok(())
	}

	/// Start loading snippets asynchronously in the background.
	/// First load from cache file if available, then fetch remote.
	fn load_snippets_background(self: Arc<Self>) {
		if let Err(e) = self.load_from_cache() {
			debug!("failed to load snippets from cache: {e:#}");
		}

		tokio::spawn(async move {
			if let Err(e) = self.fetch_and_update_snippets().await {
				warn!("failed to fetch snippets from remote: {e:#}");
			}
		});
	}

	async fn fetch_and_update_snippets(&self) -> Result<()> {
		let url = DownloadSource::Meta
			.host()
			.join("bestool/snippets")
			.into_diagnostic()?;

		let response = reqwest_client()
			.await?
			.get(url.to_string())
			.send()
			.await
			.into_diagnostic()?;

		let snippets_json: serde_json::Value = response.json().await.into_diagnostic()?;

		let mut snippets = BTreeMap::new();
		if let Some(obj) = snippets_json.as_object() {
			for (name, snippet_data) in obj {
				if let Some(sql) = snippet_data.get("sql").and_then(|v| v.as_str()) {
					snippets.insert(name.clone(), sql.to_string());
				}
			}
		}

		let count = snippets.len();
		let mut cached = self.snippets.write().await;
		*cached = Some(snippets);
		info!(count, "loaded snippets from remote");

		self.save_to_cache(&snippets_json).await?;

		Ok(())
	}
}

impl Default for AsyncSnippetProvider {
	fn default() -> Self {
		Self::new()
	}
}

impl SnippetLookupProvider for AsyncSnippetProvider {
	fn lookup(&self, name: &str) -> Option<String> {
		if let Ok(snippets) = self.snippets.try_read() {
			snippets.as_ref().and_then(|s| s.get(name).cloned())
		} else {
			None
		}
	}

	fn list_names(&self) -> Vec<String> {
		if let Ok(snippets) = self.snippets.try_read() {
			snippets
				.as_ref()
				.map(|s| s.keys().cloned().collect())
				.unwrap_or_default()
		} else {
			Vec::new()
		}
	}
}

/// SSL mode for PostgreSQL connections
#[derive(Debug, Default, Clone, Copy, ValueEnum)]
pub enum SslMode {
	/// Disable SSL/TLS encryption
	Disable,
	/// Prefer SSL/TLS but allow unencrypted connections
	#[default]
	Prefer,
	/// Require SSL/TLS encryption
	Require,
}

impl SslMode {
	fn as_str(self) -> &'static str {
		match self {
			SslMode::Disable => "disable",
			SslMode::Prefer => "prefer",
			SslMode::Require => "require",
		}
	}
}

/// Connect to Tamanu's database.
///
/// Aliases: p, pg, sql
#[derive(Debug, Clone, Parser)]
pub struct PsqlArgs {
	/// Connect to postgres with a different username.
	///
	/// This may prompt for a password depending on your local settings and pg_hba config.
	#[arg(short = 'U', long, conflicts_with = "url")]
	pub username: Option<String>,

	/// SSL mode for the connection.
	///
	/// Defaults to 'prefer' which attempts SSL but falls back to non-SSL.
	/// Use 'disable' to skip SSL entirely (useful on Windows with certificate issues).
	/// Use 'require' to enforce SSL connections.
	///
	/// Ignored if a database URL is provided and it contains an sslmode parameter.
	#[arg(long, value_enum, default_value_t = SslMode::default())]
	pub ssl: SslMode,

	/// Connect to postgres with a connection URL.
	///
	/// This bypasses the discovery of credentials from Tamanu.
	pub url: Option<String>,

	/// Enable write mode for this psql.
	///
	/// By default we set `TRANSACTION READ ONLY` for the session, which prevents writes. To enable
	/// writes, either pass this flag, or call `\W` within the session.
	///
	/// This also disables autocommit, so you need to issue a COMMIT; command whenever you perform
	/// a write (insert, update, etc), as an extra safety measure.
	///
	/// Additionally, enabling write mode will prompt for an OTS value. This should be the name of
	/// a person supervising the write operation, or a short message describing why you don't need
	/// one, such as "demo" or "emergency".
	#[arg(short = 'W', long)]
	pub write: bool,

	/// Syntax highlighting theme (light, dark, or auto)
	///
	/// Controls the color scheme for SQL syntax highlighting in the input line.
	/// 'auto' attempts to detect terminal background, defaults to 'dark' if detection fails.
	#[arg(long, default_value = "auto")]
	pub theme: bestool_psql::Theme,

	/// Path to audit database directory
	#[arg(long, value_name = "PATH", help = help_audit_path())]
	pub audit_path: Option<PathBuf>,

	/// Don't redact data
	///
	/// This will also skip loading redactions.
	#[arg(long)]
	pub no_redact: bool,
}

fn help_audit_path() -> String {
	format!(
		"Path to audit database directory (default: {})",
		bestool_psql::default_audit_dir()
	)
}

pub async fn run(ctx: Context<TamanuArgs, PsqlArgs>) -> Result<()> {
	let PsqlArgs {
		username,
		ssl,
		url,
		write,
		theme,
		audit_path,
		no_redact,
	} = ctx.args_sub;

	let url = if let Some(url) = url {
		let mut url = reqwest::Url::parse(&url).into_diagnostic()?;
		if !url.query_pairs().any(|(key, _)| key == "sslmode") {
			url.query_pairs_mut().append_pair("sslmode", ssl.as_str());
		}
		url.to_string()
	} else {
		let (_, root) = find_tamanu(&ctx.args_top)?;
		let config = load_config(&root, None)?;

		let (username, password) = if let Some(ref user) = username {
			// First, check if this matches a report schema connection
			if let Some(ref report_schemas) = config.db.report_schemas {
				if let Some(connection) = report_schemas.connections.get(user)
					&& !connection.username.is_empty()
				{
					(
						Some(connection.username.clone()),
						Some(connection.password.clone()),
					)
				} else if user == &config.db.username {
					// User matches main db user
					(
						Some(config.db.username.clone()),
						Some(config.db.password.clone()),
					)
				} else {
					// User doesn't match anything, rely on psql password prompt
					(Some(user.clone()), None)
				}
			} else if user == &config.db.username {
				// No report schemas, check if matches main user
				(
					Some(config.db.username.clone()),
					Some(config.db.password.clone()),
				)
			} else {
				// User doesn't match, rely on psql password prompt
				(Some(user.clone()), None)
			}
		} else {
			// No user specified, use main db credentials
			(
				Some(config.db.username.clone()),
				Some(config.db.password.clone()),
			)
		};

		let username = username.unwrap_or_else(|| config.db.username.clone());
		let password = if password.as_ref().is_some_and(|p| p.is_empty()) {
			None
		} else {
			password
		};

		let builder = ConnectionUrlBuilder {
			username,
			password,
			host: config.db.host.clone().unwrap_or_default(),
			port: config.db.port,
			database: config.db.name.clone(),
			ssl_mode: Some(ssl.as_str().to_string()),
		};
		builder.build()
	};

	debug!(url, "creating connection pool");
	let pool = bestool_psql::create_pool(&url).await?;

	// Install a Ctrl-C handler that sets a flag for query cancellation
	bestool_psql::register_sigint_handler()?;

	let version = get_tamanu_version(&pool).await;

	let (redact_mode, redactions) = if let Some(ref version) = version {
		if no_redact {
			debug!("skipping redaction loading");
			(false, HashSet::new())
		} else {
			load_redactions(version).await
		}
	} else {
		debug!("skipping redaction loading");
		(false, HashSet::new())
	};

	let snippet_provider = Arc::new(AsyncSnippetProvider::new());
	snippet_provider.clone().load_snippets_background();

	bestool_psql::run(
		pool,
		#[expect(
			clippy::needless_update,
			reason = "future-proofing for when Config gains new fields"
		)]
		bestool_psql::Config {
			theme: theme.resolve(),
			audit_path,
			write,
			use_colours: ctx.args_top.use_colours,
			redact_mode,
			redactions,
			snippet_lookup: Some(snippet_provider),
			..Default::default()
		},
	)
	.await
}

async fn get_tamanu_version(pool: &bestool_psql::PgPool) -> Option<String> {
	let client = pool.get().await.ok()?;
	let row = client
		.query_one(
			"SELECT value FROM local_system_facts WHERE key = 'currentVersion'",
			&[],
		)
		.await
		.ok()?;
	row.try_get(0).ok()
}

#[instrument(level = "debug")]
async fn load_redactions(version: &str) -> (bool, HashSet<ColumnRef>) {
	match timeout(Duration::from_secs(2), fetch_and_cache_redactions(version)).await {
		Ok(Ok(redactions)) => {
			debug!("loaded {} redaction rules", redactions.len());
			(!redactions.is_empty(), redactions)
		}
		Ok(Err(e)) => {
			warn!("failed to load redactions: {}", e);
			(false, HashSet::new())
		}
		Err(_) => {
			warn!("failed to load redactions: timed out");
			(false, HashSet::new())
		}
	}
}

async fn fetch_and_cache_redactions(version: &str) -> Result<HashSet<ColumnRef>> {
	let cache_dir = if let Some(dir) = dirs::cache_dir() {
		dir.join("bestool").join("redactions")
	} else {
		std::env::temp_dir().join("bestool").join("redactions")
	};

	fs::create_dir_all(&cache_dir).await.into_diagnostic()?;

	let cache_file = cache_dir.join(format!("redactions-{version}.json"));

	if let Ok(contents) = fs::read_to_string(&cache_file).await
		&& let Ok(redactions) = serde_json::from_str(&contents)
	{
		debug!("loaded redactions from cache for {}", version);
		return Ok(redactions);
	}

	match fetch_redactions_from_source(version).await {
		Ok(redactions) => {
			let json = serde_json::to_string_pretty(&redactions).into_diagnostic()?;
			fs::write(&cache_file, json).await.into_diagnostic()?;
			Ok(redactions)
		}
		Err(e) => {
			if let Some(base_version) = get_base_version(version)
				&& base_version != version
			{
				debug!(
					"failed to fetch redactions for {}, trying {}: {}",
					version, base_version, e
				);

				let base_cache_file = cache_dir.join(format!("redactions-{base_version}.json"));

				if let Ok(contents) = fs::read_to_string(&base_cache_file).await
					&& let Ok(redactions) = serde_json::from_str(&contents)
				{
					debug!(
						"loaded redactions from cache for base version {}",
						base_version
					);
					return Ok(redactions);
				}

				let redactions = fetch_redactions_from_source(&base_version).await?;

				let json = serde_json::to_string_pretty(&redactions).into_diagnostic()?;
				fs::write(&base_cache_file, json).await.into_diagnostic()?;

				Ok(redactions)
			} else {
				Err(e)
			}
		}
	}
}

fn get_base_version(version: &str) -> Option<String> {
	let parts: Vec<&str> = version.split('.').collect();
	if parts.len() != 3 {
		return None;
	}

	if parts[1].parse::<u32>().is_err() || parts[2].parse::<u32>().is_err() {
		return None;
	}

	if parts[2] == "0" {
		None
	} else {
		Some(format!("{}.{}.0", parts[0], parts[1]))
	}
}

#[instrument(level = "debug")]
async fn fetch_redactions_from_source(version: &str) -> Result<HashSet<ColumnRef>> {
	let url = format!("https://docs.data.bes.au/tamanu/v{version}/manifest.json");
	debug!("fetching redactions from {}", url);

	let response = reqwest::get(&url).await.into_diagnostic()?;
	let text = response.text().await.into_diagnostic()?;

	parse_manifest(&text)
}

fn parse_manifest(json: &str) -> Result<HashSet<ColumnRef>> {
	let mut redactions = HashSet::new();

	let manifest: Value = serde_json::from_str(json)
		.into_diagnostic()
		.wrap_err("failed to parse dbt manifest")?;

	let Some(sources) = manifest.get("sources").and_then(|v| v.as_object()) else {
		bail!("manifest missing 'sources' object");
	};

	for (source_name, source_def) in sources {
		if let Some((schema, table)) = parse_source_name(source_name)
			&& let Some(columns) = source_def.get("columns").and_then(|v| v.as_object())
		{
			for (column_name, column_def) in columns {
				if has_masking(column_def) {
					redactions.insert(ColumnRef {
						schema: schema.to_string(),
						table: table.to_string(),
						column: column_name.clone(),
					});
				}
			}
		}
	}

	debug!("parsed {} redactions from manifest", redactions.len());
	Ok(redactions)
}

fn parse_source_name(source_name: &str) -> Option<(&str, &str)> {
	let parts: Vec<&str> = source_name.split('.').collect();
	if parts.len() != 4 || parts[0] != "source" || parts[1] != "tamanu" {
		return None;
	}

	let schema_part = parts[2];
	let table = parts[3];

	let schema = if schema_part == "tamanu" {
		"public"
	} else if let Some(stripped) = schema_part.strip_suffix("__tamanu") {
		stripped
	} else {
		return None;
	};

	Some((schema, table))
}

fn has_masking(column_def: &serde_json::Value) -> bool {
	column_def
		.get("config")
		.and_then(|v| v.get("meta"))
		.and_then(|v| v.get("masking"))
		.is_some()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_parse_source_name_public_schema() {
		assert_eq!(
			parse_source_name("source.tamanu.tamanu.users"),
			Some(("public", "users"))
		);
	}

	#[test]
	fn test_parse_source_name_custom_schema() {
		assert_eq!(
			parse_source_name("source.tamanu.fhir__tamanu.patient"),
			Some(("fhir", "patient"))
		);
	}

	#[test]
	fn test_parse_source_name_invalid() {
		assert_eq!(parse_source_name("invalid.format"), None);
		assert_eq!(parse_source_name("source.wrong.tamanu.users"), None);
		assert_eq!(parse_source_name("source.tamanu.invalid.users"), None);
	}

	#[test]
	fn test_parse_manifest_with_masking() {
		let json = r#"{
			"sources": {
				"source.tamanu.tamanu.users": {
					"columns": {
						"email": {
							"config": {
								"meta": {
									"masking": "email"
								}
							}
						},
						"name": {}
					}
				},
				"source.tamanu.fhir__tamanu.patient": {
					"columns": {
						"ssn": {
							"config": {
								"meta": {
									"masking": {
										"type": "hash"
									}
								}
							}
						}
					}
				}
			}
		}"#;

		let result = parse_manifest(json).unwrap();
		assert_eq!(result.len(), 2);
		assert!(result.contains(&ColumnRef {
			schema: "public".to_string(),
			table: "users".to_string(),
			column: "email".to_string(),
		}));
		assert!(result.contains(&ColumnRef {
			schema: "fhir".to_string(),
			table: "patient".to_string(),
			column: "ssn".to_string(),
		}));
	}

	#[test]
	fn test_parse_manifest_malformed() {
		assert!(parse_manifest("not json").is_err());
		assert!(parse_manifest("{}").is_err());
		assert!(parse_manifest(r#"{"sources": null}"#).is_err());
	}

	#[test]
	fn test_has_masking() {
		use serde_json::json;

		assert!(has_masking(&json!({
			"config": {
				"meta": {
					"masking": "email"
				}
			}
		})));

		assert!(has_masking(&json!({
			"config": {
				"meta": {
					"masking": {"type": "hash"}
				}
			}
		})));

		assert!(!has_masking(&json!({
			"config": {
				"meta": {}
			}
		})));

		assert!(!has_masking(&json!({})));
	}

	#[test]
	fn test_get_base_version() {
		assert_eq!(get_base_version("2.38.7"), Some("2.38.0".to_string()));
		assert_eq!(get_base_version("1.2.3"), Some("1.2.0".to_string()));
		assert_eq!(get_base_version("2.38.0"), None);
		assert_eq!(get_base_version("1.0.0"), None);
		assert_eq!(get_base_version("invalid"), None);
		assert_eq!(get_base_version("2.38"), None);
		assert_eq!(get_base_version("2.38.7.1"), None);
	}
}
