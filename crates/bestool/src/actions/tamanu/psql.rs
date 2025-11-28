use std::{collections::HashSet, path::PathBuf, time::Duration};

use bestool_psql::column_extractor::ColumnRef;
use clap::{Parser, ValueEnum};
use miette::{IntoDiagnostic as _, Result};
use tokio::{fs, time::timeout};
use tracing::{debug, instrument, warn};

use crate::actions::Context;

use super::{TamanuArgs, config::load_config, connection_url::ConnectionUrlBuilder, find_tamanu};

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

	bestool_psql::run(
		pool,
		bestool_psql::Config {
			theme: theme.resolve(),
			audit_path,
			write,
			use_colours: ctx.args_top.use_colours,
			redact_mode,
			redactions,
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
		debug!("loaded redactions from cache");
		return Ok(redactions);
	}

	let redactions = fetch_redactions_from_source(version).await?;

	let json = serde_json::to_string_pretty(&redactions).into_diagnostic()?;
	fs::write(&cache_file, json).await.into_diagnostic()?;

	Ok(redactions)
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
	use serde_json::Value;

	let mut redactions = HashSet::new();

	let manifest: Value = match serde_json::from_str(json) {
		Ok(v) => v,
		Err(e) => {
			warn!("failed to parse manifest JSON: {}", e);
			return Ok(redactions);
		}
	};

	let Some(sources) = manifest.get("sources").and_then(|v| v.as_object()) else {
		debug!("manifest missing 'sources' object");
		return Ok(redactions);
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
		assert_eq!(parse_manifest("not json").unwrap().len(), 0);
		assert_eq!(parse_manifest("{}").unwrap().len(), 0);
		assert_eq!(parse_manifest(r#"{"sources": null}"#).unwrap().len(), 0);
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
}
