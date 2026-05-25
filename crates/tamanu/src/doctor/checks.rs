//! Doctor healthchecks. One module per check.
//!
//! Each module exposes a `pub async fn run(ctx: &CheckContext) -> Check` (or a
//! sync `fn run(...) -> Check` where async is unnecessary). The `ALL` registry
//! below ties names to runners so the dispatcher can filter by `--check`.

use std::{path::PathBuf, sync::Arc};

use node_semver::Version;
use tokio_postgres::Client as PgClient;

use crate::{ApiServerKind, config::TamanuConfig};

use super::check::Check;

pub mod db_connect;
pub mod db_version;
pub mod disk_free;
pub mod external_users;
pub mod fhir_jobs;
pub mod http_errors;
pub mod kopia_backup;
pub mod load;
pub mod memory;
pub mod migrations;
pub mod server_id;
pub mod sync_sessions;
pub mod tailscale;
pub mod tamanu_found;
pub mod tamanu_http;
pub mod tamanu_service;
pub mod time_sync;
pub mod uptime;

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
	use std::error::Error;

	if let Some(db) = err.as_db_error() {
		let mut s = format!("{}: {}", db.severity(), db.message());
		if let Some(detail) = db.detail() {
			s.push_str(" — ");
			s.push_str(detail);
		}
		return s;
	}

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
		entry!("http_errors", http_errors),
		entry!("tailscale", tailscale, off_wire),
		entry!("tamanu_service", tamanu_service),
		entry!("external_users", external_users),
		entry!("sync_sessions", sync_sessions),
		entry!("fhir_jobs", fhir_jobs),
		entry!("kopia_backup", kopia_backup),
	]
}
