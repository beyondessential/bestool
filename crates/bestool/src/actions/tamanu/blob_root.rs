use std::path::{Path, PathBuf};

use clap::Parser;
use miette::{Context as _, IntoDiagnostic as _, Result, bail};

use bestool_tamanu::{ApiServerKind, config::TamanuConfig, config::load_config, detect_kind};

use crate::actions::{
	Context,
	tamanu::{TamanuArgs, find_tamanu},
};

/// The schema default when no settings row exists, from
/// `packages/settings/src/schema/{central,facility}.ts` in Tamanu.
const DEFAULT_ROOT: &str = "data/blobs";

/// Print the Tamanu blob store root.
///
/// The root is Tamanu's `blobStorage.root` setting (database-backed and
/// editable in the admin panel, so no config file carries it), resolved
/// against the server package directory when not absolute. A blob store
/// backup def names this command as its `path_command`, so every capture and
/// restore follows the live setting instead of a hardcoded path going stale.
#[derive(Debug, Clone, Parser)]
pub struct BlobRootArgs {
	/// Package to read the setting for (central-server or facility-server).
	///
	/// Detected from the config and database when not given.
	#[arg(short, long)]
	pub package: Option<String>,
}

pub async fn run(args: BlobRootArgs, ctx: Context) -> Result<()> {
	let (_, root) = find_tamanu(ctx.require::<TamanuArgs>()).await?;
	let config = load_config(&root, args.package.as_deref())?;
	let client =
		bestool_postgres::pool::connect_one(&config.database_url(), "bestool-tamanu-blob-root")
			.await?;
	let kind = match args.package.as_deref().and_then(ApiServerKind::from_str_ci) {
		Some(kind) => kind,
		None => detect_kind(&config, Some(&client)).await,
	};

	let rows = client
		.query(
			"SELECT value, scope, facility_id FROM settings \
			 WHERE key = 'blobStorage.root' AND deleted_at IS NULL \
			 ORDER BY facility_id NULLS LAST",
			&[],
		)
		.await
		.into_diagnostic()
		.wrap_err("querying the blobStorage.root setting")?;
	let rows: Vec<SettingRow> = rows
		.into_iter()
		.map(|row| SettingRow {
			value: row.get(0),
			scope: row.get(1),
			facility_id: row.get(2),
		})
		.collect();

	let setting = pick_root(&rows, kind, first_facility_id(&config))
		.unwrap_or_else(|| DEFAULT_ROOT.to_owned());
	println!("{}", resolve_root(&setting, &root, kind)?.display());
	Ok(())
}

/// One live `settings` row for the key: a JSONB value, its scope, and the
/// facility it applies to (facility-scoped rows only).
struct SettingRow {
	value: serde_json::Value,
	scope: String,
	facility_id: Option<String>,
}

/// The facility whose settings the server uses: Tamanu's tasks convention is
/// the first configured facility on a multi-facility server.
fn first_facility_id(config: &TamanuConfig) -> Option<&str> {
	config
		.server_facility_ids
		.as_ref()
		.and_then(|ids| ids.first())
		.or(config.server_facility_id.as_ref())
		.map(String::as_str)
		.filter(|id| !id.is_empty())
}

/// Pick the stored value the server would use, or `None` when the schema
/// default applies. On a facility server that's the first configured
/// facility's row (any facility row when the config doesn't say which); on
/// central, the central-scoped row.
fn pick_root(rows: &[SettingRow], kind: ApiServerKind, facility_id: Option<&str>) -> Option<String> {
	let row = match kind {
		ApiServerKind::Facility => match facility_id {
			Some(id) => rows.iter().find(|r| r.facility_id.as_deref() == Some(id)),
			None => rows.iter().find(|r| r.facility_id.is_some()),
		},
		ApiServerKind::Central => rows
			.iter()
			.find(|r| r.scope == "central" && r.facility_id.is_none()),
	};
	row.and_then(|r| r.value.as_str().map(str::to_owned))
}

/// Resolve a relative root the way the server does: against its working
/// directory, the server package directory under the install root. An
/// absolute root passes through. A relative root with no package directory to
/// resolve against (e.g. a containerised install, whose in-container path
/// means nothing on the host) is an error: such a deployment should set an
/// absolute root.
fn resolve_root(setting: &str, root: &Path, kind: ApiServerKind) -> Result<PathBuf> {
	let path = Path::new(setting);
	if path.is_absolute() {
		return Ok(path.to_path_buf());
	}
	let base = root.join("packages").join(kind.package_name());
	if !base.is_dir() {
		bail!(
			"blobStorage.root is relative ({setting}) and there is no {} to resolve it against; \
			 set the setting to an absolute path",
			base.display()
		);
	}
	Ok(base.join(path))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn row(value: &str, scope: &str, facility_id: Option<&str>) -> SettingRow {
		SettingRow {
			value: serde_json::Value::String(value.to_owned()),
			scope: scope.to_owned(),
			facility_id: facility_id.map(str::to_owned),
		}
	}

	#[test]
	fn picks_the_configured_facility_row() {
		let rows = vec![
			row("/a", "facility", Some("facility-a")),
			row("/b", "facility", Some("facility-b")),
		];
		assert_eq!(
			pick_root(&rows, ApiServerKind::Facility, Some("facility-b")),
			Some("/b".to_owned())
		);
		// The configured facility has no row: the schema default applies, even
		// though another facility's row exists.
		assert_eq!(pick_root(&rows, ApiServerKind::Facility, Some("facility-c")), None);
		// Unknown facility: any facility row (rows arrive sorted by facility id).
		assert_eq!(
			pick_root(&rows, ApiServerKind::Facility, None),
			Some("/a".to_owned())
		);
	}

	#[test]
	fn picks_the_central_row_only_on_central() {
		let rows = vec![row("/c", "central", None)];
		assert_eq!(
			pick_root(&rows, ApiServerKind::Central, None),
			Some("/c".to_owned())
		);
		assert_eq!(pick_root(&rows, ApiServerKind::Facility, None), None);
		assert_eq!(pick_root(&[], ApiServerKind::Central, None), None);
	}

	#[test]
	fn absolute_root_passes_through() {
		let root = if cfg!(windows) { r"C:\Tamanu\blobs" } else { "/var/lib/tamanu/blobs" };
		assert_eq!(
			resolve_root(root, Path::new("/nonexistent"), ApiServerKind::Central).unwrap(),
			PathBuf::from(root)
		);
	}

	#[test]
	fn relative_root_resolves_against_the_package_dir() {
		let tmp = tempfile::tempdir().unwrap();
		let package_dir = tmp.path().join("packages").join("facility-server");
		std::fs::create_dir_all(&package_dir).unwrap();
		assert_eq!(
			resolve_root("data/blobs", tmp.path(), ApiServerKind::Facility).unwrap(),
			package_dir.join("data/blobs")
		);
		// No package dir (e.g. containerised install): refuse rather than guess.
		assert!(resolve_root("data/blobs", tmp.path(), ApiServerKind::Central).is_err());
	}
}
