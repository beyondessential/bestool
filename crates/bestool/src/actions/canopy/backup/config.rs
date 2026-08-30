//! Backup definitions read from `/etc/bestool/backups/*.toml`.
//!
//! One file per def (Ansible-friendly): a `type` (the Canopy-facing label),
//! optional `[tags]` and `[[pre]]`/`[[post]]` hooks, and **exactly one** method
//! table (`[simple]` or `[postgresql]`). The dir is overridable via
//! `BESTOOL_BACKUPS_DIR` for tests and ad-hoc relocation.

use std::{
	collections::BTreeMap,
	path::{Path, PathBuf},
};

use miette::{Context as _, IntoDiagnostic as _, Result, bail};
use serde::Deserialize;
use tracing::warn;

use super::method::{Method, PostgresqlConfig, SimpleConfig};

/// Environment variable overriding the backups config directory.
pub const BACKUPS_DIR_ENV: &str = "BESTOOL_BACKUPS_DIR";

/// Environment variable overriding kopia's cache budget, in megabytes.
///
/// An absolute size, so it replaces the share-of-the-volume sizing rather than
/// capping it, for a host whose disk is shared or sized unusually.
pub const CACHE_BUDGET_ENV: &str = "BESTOOL_KOPIA_CACHE_MB";

/// The cache budget override, if the environment carries a usable one.
pub fn cache_budget_override() -> Option<u64> {
	let raw = std::env::var(CACHE_BUDGET_ENV).ok()?;
	let parsed = parse_cache_budget(&raw);
	if parsed.is_none() {
		warn!(
			value = %raw,
			"ignoring {CACHE_BUDGET_ENV}: not a positive whole number of megabytes"
		);
	}
	parsed
}

/// Parse a cache budget in megabytes; `None` for anything not a positive whole
/// number, so a typo falls back to the default sizing rather than to nothing.
fn parse_cache_budget(raw: &str) -> Option<u64> {
	raw.trim().parse::<u64>().ok().filter(|mb| *mb > 0)
}

/// A single pre/post command hook.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Hook {
	/// The command and its arguments (argv-style, no shell).
	pub command: Vec<String>,
}

/// A parsed backup definition.
#[derive(Debug, Clone)]
pub struct BackupDef {
	/// The Canopy backup-type name (label only).
	pub r#type: String,
	/// A type this def follows: after a successful run of that type, this def
	/// runs too, so a pair like database-then-secrets stays ordered.
	pub after: Option<String>,
	/// Extra kopia tags merged with the canopy-* tags.
	pub tags: BTreeMap<String, String>,
	/// Commands run before the method prepares (sequential, fail-fast).
	pub pre: Vec<Hook>,
	/// Commands run after cleanup (best-effort, always).
	pub post: Vec<Hook>,
	/// Commands run before this def's restore lays its data down (sequential,
	/// fail-fast), to quiesce whatever holds the data being replaced.
	pub pre_restore: Vec<Hook>,
	/// Commands run after this def's restore lays its data down (sequential,
	/// fail-fast), for data a service only reads when it starts.
	pub post_restore: Vec<Hook>,
	/// The selected method.
	pub method: Method,
}

/// Raw on-disk shape; the method tables are mutually exclusive options validated
/// after deserialisation.
#[derive(Debug, Deserialize)]
struct RawDef {
	r#type: String,
	#[serde(default)]
	after: Option<String>,
	#[serde(default)]
	tags: BTreeMap<String, String>,
	#[serde(default)]
	pre: Vec<Hook>,
	#[serde(default)]
	post: Vec<Hook>,
	#[serde(default)]
	pre_restore: Vec<Hook>,
	#[serde(default)]
	post_restore: Vec<Hook>,
	#[serde(default)]
	simple: Option<SimpleConfig>,
	#[serde(default)]
	postgresql: Option<PostgresqlConfig>,
}

impl RawDef {
	fn into_def(self) -> Result<BackupDef> {
		let method = match (self.simple, self.postgresql) {
			(Some(simple), None) => Method::Simple(simple),
			(None, Some(postgresql)) => Method::Postgresql(postgresql),
			(None, None) => bail!(
				"backup def '{}' has no method table; add exactly one of [simple] or [postgresql]",
				self.r#type
			),
			(Some(_), Some(_)) => bail!(
				"backup def '{}' has both [simple] and [postgresql]; exactly one is allowed",
				self.r#type
			),
		};
		if self.after.as_deref() == Some(self.r#type.as_str()) {
			bail!("backup def '{}' declares itself as its own `after`", self.r#type);
		}
		Ok(BackupDef {
			r#type: self.r#type,
			after: self.after,
			tags: self.tags,
			pre: self.pre,
			post: self.post,
			pre_restore: self.pre_restore,
			post_restore: self.post_restore,
			method,
		})
	}
}

/// Parse a single def from TOML text.
pub fn parse_def(text: &str) -> Result<BackupDef> {
	let raw: RawDef = toml::from_str(text)
		.into_diagnostic()
		.wrap_err("parsing backup definition TOML")?;
	raw.into_def()
}

/// The directory backup defs are read from.
pub fn backups_dir() -> PathBuf {
	if let Some(dir) = std::env::var_os(BACKUPS_DIR_ENV) {
		return PathBuf::from(dir);
	}
	default_backups_dir()
}

#[cfg(unix)]
fn default_backups_dir() -> PathBuf {
	PathBuf::from("/etc/bestool/backups")
}

#[cfg(windows)]
fn default_backups_dir() -> PathBuf {
	let base = std::env::var_os("ProgramData")
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
	base.join("bestool").join("backups")
}

/// Load every `*.toml` def from a directory.
///
/// A missing directory is not an error — it just yields no defs (a host with no
/// backups configured). Each file's stem is only informational; the canonical
/// identity is the `type` field inside.
pub async fn load_dir(dir: &Path) -> Result<Vec<BackupDef>> {
	let mut entries = match tokio::fs::read_dir(dir).await {
		Ok(entries) => entries,
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
		Err(err) => {
			return Err(err)
				.into_diagnostic()
				.wrap_err_with(|| format!("reading backups dir {}", dir.display()));
		}
	};

	let mut defs = Vec::new();
	while let Some(entry) = entries
		.next_entry()
		.await
		.into_diagnostic()
		.wrap_err_with(|| format!("listing backups dir {}", dir.display()))?
	{
		let path = entry.path();
		if path.extension().and_then(|e| e.to_str()) != Some("toml") {
			continue;
		}
		let text = tokio::fs::read_to_string(&path)
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("reading backup def {}", path.display()))?;
		let def = parse_def(&text).wrap_err_with(|| format!("in {}", path.display()))?;
		defs.push(def);
	}
	defs.sort_by(|a, b| a.r#type.cmp(&b.r#type));
	Ok(defs)
}

/// Find a def by its `type`, or `None` if there's no such def.
pub async fn find_def(dir: &Path, backup_type: &str) -> Result<Option<BackupDef>> {
	Ok(load_dir(dir)
		.await?
		.into_iter()
		.find(|d| d.r#type == backup_type))
}

/// The defs that declare `after = backup_type`, in type order.
///
/// spec: BAK#follower-backups
pub async fn followers_of(dir: &Path, backup_type: &str) -> Result<Vec<BackupDef>> {
	Ok(load_dir(dir)
		.await?
		.into_iter()
		.filter(|d| d.after.as_deref() == Some(backup_type))
		.collect())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_simple_def_with_hooks_and_tags() {
		let def = parse_def(
			r#"
			type = "something-custom"
			[tags]
			app = "tamanu"
			[[pre]]
			command = ["/usr/bin/touch", "something"]
			[[post]]
			command = ["/usr/bin/rm", "something"]
			[simple]
			path = "/somewhere/custom"
			"#,
		)
		.unwrap();

		assert_eq!(def.r#type, "something-custom");
		assert_eq!(def.tags.get("app").map(String::as_str), Some("tamanu"));
		assert_eq!(
			def.pre,
			vec![Hook {
				command: vec!["/usr/bin/touch".into(), "something".into()],
			}]
		);
		assert_eq!(
			def.post,
			vec![Hook {
				command: vec!["/usr/bin/rm".into(), "something".into()],
			}]
		);
		assert_eq!(def.method.name(), "simple");
	}

	#[test]
	fn parses_postgresql_def() {
		let def = parse_def(
			r#"
			type = "tamanu-postgres"
			[postgresql]
			cluster = "main"
			"#,
		)
		.unwrap();
		assert_eq!(def.r#type, "tamanu-postgres");
		assert_eq!(def.method.name(), "postgresql");
		assert!(def.pre.is_empty());
		assert!(def.tags.is_empty());
	}

	#[test]
	fn parses_after() {
		let def = parse_def(
			r#"
			type = "tamanu-secrets"
			after = "tamanu-postgres"
			[simple]
			path = "/var/lib/containers/storage/secrets"
			"#,
		)
		.unwrap();
		assert_eq!(def.r#type, "tamanu-secrets");
		assert_eq!(def.after.as_deref(), Some("tamanu-postgres"));
		assert_eq!(def.method.name(), "simple");
	}

	#[test]
	fn parses_restore_hooks() {
		let def = parse_def(
			r#"
			type = "tamanu-secrets"
			[simple]
			path = "/var/lib/containers/storage/secrets"
			[[pre_restore]]
			command = ["/usr/bin/systemctl", "stop", "tamanu-central"]
			[[post_restore]]
			command = ["/usr/bin/systemctl", "start", "tamanu-central"]
			"#,
		)
		.unwrap();
		assert_eq!(
			def.pre_restore
				.iter()
				.map(|h| h.command.clone())
				.collect::<Vec<_>>(),
			vec![vec!["/usr/bin/systemctl", "stop", "tamanu-central"]]
		);
		assert_eq!(
			def.post_restore
				.iter()
				.map(|h| h.command.clone())
				.collect::<Vec<_>>(),
			vec![vec!["/usr/bin/systemctl", "start", "tamanu-central"]]
		);
	}

	#[test]
	fn restore_hooks_default_to_empty() {
		let def = parse_def(
			r#"
			type = "tamanu-postgres"
			[postgresql]
			cluster = "main"
			"#,
		)
		.unwrap();
		assert!(def.pre_restore.is_empty());
		assert!(def.post_restore.is_empty());
	}

	#[test]
	fn after_defaults_to_none() {
		let def = parse_def(
			r#"
			type = "tamanu-postgres"
			[postgresql]
			cluster = "main"
			"#,
		)
		.unwrap();
		assert_eq!(def.after, None);
	}

	#[test]
	fn rejects_self_after() {
		let err = parse_def(
			r#"
			type = "loop"
			after = "loop"
			[simple]
			path = "/a"
			"#,
		)
		.unwrap_err();
		assert!(format!("{err}").contains("its own"));
	}

	#[tokio::test]
	async fn followers_of_selects_by_after_in_type_order() {
		let dir = std::env::temp_dir().join(format!("bestool-followers-{}", std::process::id()));
		tokio::fs::create_dir_all(&dir).await.unwrap();
		tokio::fs::write(
			dir.join("pg.toml"),
			"type = \"tamanu-postgres\"\n[postgresql]\ncluster = \"main\"\n",
		)
		.await
		.unwrap();
		tokio::fs::write(
			dir.join("secrets.toml"),
			"type = \"tamanu-secrets\"\nafter = \"tamanu-postgres\"\n[simple]\npath = \"/srv/secrets\"\n",
		)
		.await
		.unwrap();
		tokio::fs::write(
			dir.join("assets.toml"),
			"type = \"assets\"\nafter = \"tamanu-postgres\"\n[simple]\npath = \"/srv/assets\"\n",
		)
		.await
		.unwrap();

		let followers = followers_of(&dir, "tamanu-postgres").await.unwrap();
		let types: Vec<&str> = followers.iter().map(|d| d.r#type.as_str()).collect();
		assert_eq!(types, vec!["assets", "tamanu-secrets"]);
		assert!(followers_of(&dir, "tamanu-secrets").await.unwrap().is_empty());

		tokio::fs::remove_dir_all(&dir).await.ok();
	}

	#[test]
	fn rejects_two_method_tables() {
		let err = parse_def(
			r#"
			type = "bad"
			[simple]
			path = "/a"
			[postgresql]
			cluster = "main"
			"#,
		)
		.unwrap_err();
		assert!(format!("{err}").contains("exactly one"));
	}

	#[test]
	fn rejects_no_method_table() {
		let err = parse_def(
			r#"
			type = "bad"
			"#,
		)
		.unwrap_err();
		assert!(format!("{err}").contains("no method table"));
	}

	#[tokio::test]
	async fn load_dir_missing_is_empty() {
		let dir = std::env::temp_dir().join(format!("bestool-backups-missing-{}", std::process::id()));
		let defs = load_dir(&dir).await.unwrap();
		assert!(defs.is_empty());
	}

	#[tokio::test]
	async fn load_dir_reads_toml_only_and_finds_by_type() {
		let dir = std::env::temp_dir().join(format!("bestool-backups-{}", std::process::id()));
		tokio::fs::create_dir_all(&dir).await.unwrap();
		tokio::fs::write(
			dir.join("pg.toml"),
			"type = \"tamanu-postgres\"\n[postgresql]\ncluster = \"main\"\n",
		)
		.await
		.unwrap();
		tokio::fs::write(
			dir.join("files.toml"),
			"type = \"files\"\n[simple]\npath = \"/srv/files\"\n",
		)
		.await
		.unwrap();
		// A non-toml file is ignored.
		tokio::fs::write(dir.join("README.md"), "ignore me").await.unwrap();

		let defs = load_dir(&dir).await.unwrap();
		assert_eq!(defs.len(), 2);
		// Sorted by type.
		assert_eq!(defs[0].r#type, "files");
		assert_eq!(defs[1].r#type, "tamanu-postgres");

		let found = find_def(&dir, "files").await.unwrap().unwrap();
		assert_eq!(found.method.name(), "simple");
		assert!(find_def(&dir, "nope").await.unwrap().is_none());

		tokio::fs::remove_dir_all(&dir).await.ok();
	}
}

#[cfg(test)]
mod cache_budget_tests {
	use super::*;

	#[test]
	fn parses_a_plain_megabyte_count() {
		assert_eq!(parse_cache_budget("2048"), Some(2048));
		assert_eq!(parse_cache_budget(" 2048 "), Some(2048));
	}

	#[test]
	fn rejects_values_that_would_disable_the_cache_or_confuse_kopia() {
		assert_eq!(parse_cache_budget("0"), None);
		assert_eq!(parse_cache_budget("-1"), None);
		assert_eq!(parse_cache_budget("2GB"), None);
		assert_eq!(parse_cache_budget(""), None);
	}
}
