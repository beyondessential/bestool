use std::{
	collections::{BTreeSet, HashSet},
	path::{Path, PathBuf},
};

use bestool_tamanu::{ApiServerKind, config::load_config, detect_kind, roots};
use clap::Parser;
use miette::{IntoDiagnostic, Result, bail, miette};
use node_semver::Version;

use crate::actions::Context;

use super::{
	TamanuArgs,
	artifacts::Platform,
	download::{self, DownloadArgs},
};

const REPORTING_SCHEMA: &str = "reporting";

/// Where a release keeps its migrations, relative to its root, across the layouts
/// Tamanu has shipped.
const MIGRATION_DIRS: &[&str] = &[
	"packages/database/src/migrations",
	"packages/database/dist/migrations",
	"packages/database/dist/cjs/migrations",
	"packages/database/dist/esm/migrations",
];

/// Prepare this server for upgrading Tamanu to a new version.
///
/// Downloads the target release if it isn't installed yet and works out which of
/// its migrations this database hasn't run. If there are any, drops the reporting
/// schema, whose views block migrations that alter the columns they read.
///
/// A reporting schema stamped by alertd is dropped as is: alertd reapplies it once
/// the new version has migrated. Any other reporting schema is only dropped with
/// `--yes`, and has to be reinstalled by hand after the upgrade.
#[derive(Debug, Clone, Parser)]
pub struct PreUpgradeArgs {
	/// Version being upgraded to.
	#[arg(value_name = "VERSION")]
	pub version: String,

	/// Where to download the release to.
	#[arg(long)]
	#[cfg_attr(windows, arg(default_value = "/Tamanu"))]
	#[cfg_attr(not(windows), arg(default_value = "."))]
	pub into: PathBuf,

	/// Also drop a reporting schema that alertd did not apply.
	#[arg(long)]
	pub yes: bool,

	/// Say what would be done without downloading or dropping anything.
	#[arg(long)]
	pub dry_run: bool,
}

pub async fn run(args: PreUpgradeArgs, ctx: Context) -> Result<()> {
	let target = Version::parse(args.version.trim_start_matches('v')).into_diagnostic()?;
	let tamanu = ctx.require::<TamanuArgs>();

	let installed = roots::find_versions()?;
	let current_root = match tamanu.root.clone() {
		Some(root) => root,
		None => installed
			.iter()
			.find(|(version, _)| *version < target)
			.map(|(_, root)| root.clone())
			.ok_or_else(|| miette!("no installed Tamanu older than {target} to upgrade from"))?,
	};

	let config = load_config(&current_root, None)?;
	let client =
		bestool_postgres::pool::connect_one(&config.database_url(), "bestool-tamanu-pre-upgrade")
			.await?;

	let target_root = match installed.iter().find(|(version, _)| *version == target) {
		Some((_, root)) => root.clone(),
		None if args.dry_run => bail!("{target} isn't downloaded yet; run without --dry-run to fetch it"),
		None => {
			let kind = detect_kind(&config, Some(&client)).await;
			download::run(
				DownloadArgs {
					kind: artifact_type(kind).into(),
					version: target.to_string(),
					into: args.into.clone(),
					url_only: false,
					no_extract: false,
					platform: Platform::Host,
				},
				ctx.clone(),
			)
			.await?;
			roots::find_versions()?
				.into_iter()
				.find(|(version, _)| *version == target)
				.map(|(_, root)| root)
				.ok_or_else(|| miette!("downloaded {target} but can't find its release folder"))?
		}
	};

	let applied = client
		.query(r#"SELECT name FROM "SequelizeMeta""#, &[])
		.await
		.into_diagnostic()?
		.iter()
		.map(|row| row.get::<_, String>(0))
		.collect::<Vec<_>>();
	let pending = pending_migrations(&release_migrations(&target_root)?, &applied);

	let Some(first) = pending.first() else {
		println!("{target} has no migrations this database hasn't run, so the reporting schema stays");
		return Ok(());
	};
	println!("{} migration(s) in {target} to run, starting with {first}", pending.len());

	let schema = client
		.query_opt(
			"SELECT obj_description(oid, 'pg_namespace') FROM pg_namespace WHERE nspname = $1",
			&[&REPORTING_SCHEMA],
		)
		.await
		.into_diagnostic()?;
	let Some(row) = schema else {
		println!("no reporting schema to drop");
		return Ok(());
	};
	let managed = is_managed_stamp(row.get::<_, Option<String>>(0).as_deref());

	if !managed && !args.yes {
		bail!(
			"the reporting schema wasn't applied by alertd, so nothing will reinstall it after the \
			 upgrade; rerun with --yes to drop it anyway, then reinstall it by hand once the \
			 upgrade has migrated"
		);
	}

	let aftermath = if managed {
		"alertd reapplies it once the upgrade has migrated"
	} else {
		"nothing reinstalls it automatically, so reinstall it by hand once the upgrade has migrated"
	};
	if args.dry_run {
		println!("would drop the reporting schema; {aftermath}");
		return Ok(());
	}

	client
		.batch_execute(&format!("DROP SCHEMA {REPORTING_SCHEMA} CASCADE"))
		.await
		.into_diagnostic()?;
	println!("dropped the reporting schema; {aftermath}");
	Ok(())
}

fn artifact_type(kind: ApiServerKind) -> &'static str {
	match kind {
		ApiServerKind::Central => "central",
		ApiServerKind::Facility => "facility",
	}
}

fn release_migrations(root: &Path) -> Result<Vec<String>> {
	let mut names = Vec::new();
	for dir in MIGRATION_DIRS.iter().map(|dir| root.join(dir)) {
		let Ok(entries) = std::fs::read_dir(&dir) else {
			continue;
		};
		for entry in entries {
			names.push(entry.into_diagnostic()?.file_name().to_string_lossy().into_owned());
		}
	}

	if names.is_empty() {
		bail!(
			"found no migrations in {} (looked in {})",
			root.display(),
			MIGRATION_DIRS.join(", ")
		);
	}
	Ok(names)
}

/// Migrations named in `release` that `applied` doesn't record, oldest first.
///
/// Compared without extensions: a release may ship `.ts` sources while the
/// database records the compiled `.js` name.
fn pending_migrations(release: &[String], applied: &[String]) -> Vec<String> {
	let applied = applied
		.iter()
		.filter_map(|name| migration_stem(name))
		.collect::<HashSet<_>>();
	release
		.iter()
		.filter_map(|name| migration_stem(name))
		.filter(|stem| !applied.contains(stem))
		.map(str::to_owned)
		.collect::<BTreeSet<_>>()
		.into_iter()
		.collect()
}

/// `1782783331515-foo` from `1782783331515-foo.ts`, for script files named with a
/// numeric timestamp prefix. Anything else in a migrations folder isn't a
/// migration.
fn migration_stem(name: &str) -> Option<&str> {
	let (stem, ext) = name.rsplit_once('.')?;
	let (prefix, rest) = stem.split_once('-')?;
	let is_script = matches!(ext, "js" | "ts" | "cjs" | "mjs");
	let is_timestamped = !prefix.is_empty() && prefix.bytes().all(|b| b.is_ascii_digit());
	(is_script && is_timestamped && !rest.is_empty()).then_some(stem)
}

/// Whether a reporting schema's comment is the `<version> sha256-<digest>` stamp
/// alertd leaves on a schema it applied.
fn is_managed_stamp(comment: Option<&str>) -> bool {
	let Some((version, digest)) = comment.and_then(|c| c.split_once(' ')) else {
		return false;
	};
	let Some(b64) = digest.strip_prefix("sha256-") else {
		return false;
	};
	!version.is_empty()
		&& !version.contains(char::is_whitespace)
		&& b64.len() == 44
		&& b64.ends_with('=')
		&& b64[..43]
			.bytes()
			.all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

#[cfg(test)]
mod tests {
	use super::*;

	fn strings(names: &[&str]) -> Vec<String> {
		names.iter().map(|n| n.to_string()).collect()
	}

	#[test]
	fn pending_compares_across_source_and_compiled_extensions() {
		let release = strings(&[
			"1782783331515-makeFkeyDeferrable.ts",
			"1784601600000-addIndexes.ts",
			"000_baseline.sql",
			"notes.json",
		]);
		let applied = strings(&["1782783331515-makeFkeyDeferrable.js"]);

		assert_eq!(
			pending_migrations(&release, &applied),
			vec!["1784601600000-addIndexes"]
		);
	}

	#[test]
	fn nothing_pending_when_every_migration_has_run() {
		let release = strings(&["1782783331515-a.ts", "1784601600000-b.ts"]);
		let applied = strings(&["1782783331515-a.js", "1784601600000-b.js", "1700000000000-older.js"]);

		assert!(pending_migrations(&release, &applied).is_empty());
	}

	#[test]
	fn pending_is_oldest_first_and_deduplicated_across_folders() {
		let release = strings(&["1784601600000-b.js", "1782783331515-a.ts", "1784601600000-b.ts"]);

		assert_eq!(
			pending_migrations(&release, &[]),
			vec!["1782783331515-a", "1784601600000-b"]
		);
	}

	#[test]
	fn alertd_stamp_is_managed() {
		let digest = format!("sha256-{}=", "A".repeat(43));
		assert!(is_managed_stamp(Some(&format!("2.64.2 {digest}"))));
	}

	#[test]
	fn anything_else_is_not_managed() {
		assert!(!is_managed_stamp(None));
		assert!(!is_managed_stamp(Some("2.64.2")));
		assert!(!is_managed_stamp(Some("built by hand")));
		assert!(!is_managed_stamp(Some("2.64.2 sha256-short=")));
	}
}
