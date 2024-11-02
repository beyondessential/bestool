use std::{collections::HashMap, env::temp_dir, fs, path::PathBuf};

use clap::{Parser, ValueHint};
use dunce::canonicalize;
use miette::{Context as _, IntoDiagnostic, Result};
use serde_yml::Value;
use tracing::{debug, info, instrument, warn};
use walkdir::WalkDir;

use crate::actions::{tamanu::find_postgres_bin, Context};

use super::{config::load_config, find_package, find_tamanu, ApiServerKind, TamanuArgs};

/// Generate a Greenmask config file.
#[derive(Debug, Clone, Parser)]
pub struct GreenmaskConfigArgs {
	/// Package to load config from.
	///
	/// By default, this command looks for the most recent installed version of Tamanu and tries to
	/// look for an appropriate config. If both central and facility servers are present and
	/// configured, it will pick one arbitrarily.
	#[arg(long)]
	pub kind: Option<ApiServerKind>,

	/// Folders containing table masking definitions.
	///
	/// Can be specified multiple times, entries will be merged.
	///
	/// By default, it will look in the `greenmask/config` folder in the Tamanu root, and the
	/// `greenmask` folder in the Tamanu release folder. Non-existant folders are ignored.
	#[arg(value_hint = ValueHint::DirPath)]
	pub folders: Vec<PathBuf>,

	/// Folder where dumps are stored.
	///
	/// By default, this is the `greenmask/dumps` folder in the Tamanu root.
	///
	/// If the folder does not exist, it will be created.
	#[arg(long, value_hint = ValueHint::DirPath)]
	pub storage_dir: Option<PathBuf>,
}

#[derive(serde::Deserialize, Debug)]
struct TamanuConfig {
	db: Db,
}

fn default_host() -> String {
	"localhost".into()
}

#[derive(serde::Deserialize, Debug)]
struct Db {
	#[serde(default = "default_host")]
	host: String,
	name: String,
	username: String,
	password: String,
}

#[derive(serde::Serialize, Debug)]
struct GreenmaskConfig {
	common: GreenmaskCommon,
	storage: GreenmaskStorageWrap,
	dump: GreenmaskDump,
}

#[derive(serde::Serialize, Debug)]
struct GreenmaskCommon {
	pg_bin_path: PathBuf,
	tmp_dir: PathBuf,
}

#[derive(serde::Serialize, Debug)]
struct GreenmaskStorageWrap {
	#[serde(rename = "type")]
	kind: GreenmaskStorageName,
	#[serde(flatten)]
	storage: GreenmaskStorage,
}

#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "lowercase")]
enum GreenmaskStorageName {
	Directory,
}

#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "lowercase")]
enum GreenmaskStorage {
	Directory(GreenmaskStorageDirectory),
}

impl From<GreenmaskStorage> for GreenmaskStorageWrap {
	fn from(storage: GreenmaskStorage) -> Self {
		match storage {
			GreenmaskStorage::Directory(dir) => GreenmaskStorageWrap {
				kind: GreenmaskStorageName::Directory,
				storage: GreenmaskStorage::Directory(dir),
			},
		}
	}
}

#[derive(serde::Serialize, Debug)]
struct GreenmaskStorageDirectory {
	path: PathBuf,
}

#[derive(serde::Serialize, Debug)]
struct GreenmaskDump {
	pg_dump_options: GreenmaskDumpOptions,
	transformation: Vec<GreenmaskTransformation>,
}

#[derive(serde::Serialize, Debug)]
struct GreenmaskDumpOptions {
	dbname: String,
	schema: String,
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
struct GreenmaskTransformation {
	schema: String,
	#[serde(rename = "name")]
	table: String,

	#[serde(flatten)]
	rest: Value,
}

pub async fn run(ctx: Context<TamanuArgs, GreenmaskConfigArgs>) -> Result<()> {
	let (_, tamanu_folder) = find_tamanu(&ctx.args_top)?;
	let root = tamanu_folder.parent().unwrap();

	let kind = match ctx.args_sub.kind {
		Some(kind) => kind,
		None => find_package(&tamanu_folder)?,
	};
	info!(?kind, "using");

	let config_value = load_config(&tamanu_folder, kind.package_name())?;

	let tamanu_config: TamanuConfig = serde_json::from_value(config_value)
		.into_diagnostic()
		.wrap_err("parsing of Tamanu config failed")?;

	let pg_bin_path = find_postgres_bin("psql").wrap_err("failed to find psql executable")?;
	let tmp_dir = temp_dir();

	let mut transforms_dirs = ctx.args_sub.folders;
	if transforms_dirs.is_empty() {
		transforms_dirs.push(root.join("greenmask").join("config"));
		transforms_dirs.push(tamanu_folder.join("greenmask"));
	}

	let mut transforms = HashMap::new();
	for transforms_dir in &transforms_dirs {
		info!(path=?transforms_dir, "loading transformations");
		if !transforms_dir.exists() {
			warn!(path=?transforms_dir, "directory does not exist");
			continue;
		}

		for entry in WalkDir::new(transforms_dir).follow_links(true) {
			let path = match entry {
				Ok(entry) => entry.path().to_owned(),
				Err(err) => {
					warn!(?err, "failed to read entry");
					continue;
				}
			};

			match path.extension().and_then(|ext| ext.to_str()) {
				Some("yml" | "yaml") => (),
				_ => continue,
			}

			let content = fs::read_to_string(&path).into_diagnostic()?;
			let value: GreenmaskTransformation = serde_yml::from_str(&content).into_diagnostic()?;

			debug!(path=%path.display(), "loading transformation");
			transforms
				.entry((value.schema.clone(), value.table.clone()))
				.and_modify(|entry: &mut GreenmaskTransformation| {
					debug!(
						?entry,
						"duplicate entry for {}.{}, merging {}",
						value.schema,
						value.table,
						path.display()
					);
					entry.rest = merge_yaml(entry.rest.clone(), value.rest.clone());
				})
				.or_insert(value);
		}
	}

	let storage_dir = {
		let dir = ctx
			.args_sub
			.storage_dir
			.unwrap_or_else(|| root.join("greenmask").join("dumps"));
		fs::create_dir_all(&dir).into_diagnostic()?;
		canonicalize(dir).into_diagnostic()?
	};

	let greenmask_config = GreenmaskConfig {
		common: GreenmaskCommon {
			pg_bin_path,
			tmp_dir,
		},
		storage: GreenmaskStorage::Directory(GreenmaskStorageDirectory { path: storage_dir })
			.into(),
		dump: GreenmaskDump {
			pg_dump_options: GreenmaskDumpOptions {
				dbname: format!(
					"host='{}' user='{}' password='{}' dbname='{}'",
					tamanu_config.db.host,
					tamanu_config.db.username,
					tamanu_config.db.password,
					tamanu_config.db.name
				),
				schema: "public".into(),
			},
			transformation: transforms.into_values().collect(),
		},
	};

	println!(
		"{}",
		serde_yml::to_string(&greenmask_config)
			.into_diagnostic()
			.wrap_err("failed to serialize Greenmask config")?
	);

	Ok(())
}

#[instrument(level = "trace")]
fn merge_yaml(mut base: serde_yml::Value, mut overlay: serde_yml::Value) -> serde_yml::Value {
	if let (Some(base), Some(overlay)) = (base.as_mapping_mut(), overlay.as_mapping_mut()) {
		for (key, value) in overlay {
			if let Some(base_value) = base.get_mut(key) {
				*base_value = merge_yaml(base_value.clone(), value.clone());
			} else {
				base.insert(key.clone(), value.clone());
			}
		}
	} else if let (Some(base), Some(overlay)) = (base.as_sequence_mut(), overlay.as_sequence_mut())
	{
		for item in overlay {
			base.push(item.clone());
		}
	} else {
		// If either or both of `base` and `overlay` are scalar values, it must be safe to simply overwrite the base.
		base = overlay
	}
	base
}
