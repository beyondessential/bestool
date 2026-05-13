//! Doctor healthchecks. One module per check.
//!
//! Each module exposes a `pub async fn run(ctx: &CheckContext) -> Check` (or a
//! sync `fn run(...) -> Check` where async is unnecessary). The `ALL` registry
//! below ties names to runners so the dispatcher can filter by `--check`.

use std::{path::PathBuf, sync::Arc};

use node_semver::Version;
use tokio_postgres::Client as PgClient;

use crate::actions::tamanu::config::TamanuConfig;

use super::check::Check;

pub mod db_connect;
pub mod db_version;
pub mod disk_free;
pub mod fhir_jobs;
pub mod http_errors;
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
/// before the client is available.
#[derive(Clone)]
pub struct CheckContext {
	pub tamanu_version: Version,
	pub tamanu_root: PathBuf,
	pub config: Arc<TamanuConfig>,
	pub database_url: String,
	pub db: Option<Arc<PgClient>>,
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
		entry!("sync_sessions", sync_sessions),
		entry!("fhir_jobs", fhir_jobs),
	]
}
