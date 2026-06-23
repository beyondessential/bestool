//! Built-in backup methods.
//!
//! A backup def selects exactly one method. The driver runs the def's `pre`
//! hooks, calls [`Method::prepare`] to get a kopia source path (plus any
//! method-supplied tags), snapshots it, then calls [`Method::cleanup`] and the
//! `post` hooks. `type` is just the Canopy-facing label; the method is what
//! decides *how* to produce a consistent source.

use std::{collections::BTreeMap, path::PathBuf};

use miette::Result;
use serde::Deserialize;

/// A source ready for kopia to snapshot, produced by [`Method::prepare`].
#[derive(Debug)]
pub struct Prepared {
	/// The path kopia should snapshot.
	pub path: PathBuf,
	/// Extra tags the method contributes (merged with the canopy-* tags and the
	/// def's own `[tags]`).
	pub extra_tags: BTreeMap<String, String>,
}

/// `[simple]` method: snapshot a path verbatim.
#[derive(Debug, Clone, Deserialize)]
pub struct SimpleConfig {
	/// The path kopia snapshots.
	pub path: PathBuf,
}

/// `[postgresql]` method: physical, crash-consistent cluster snapshot.
///
/// Driven entirely by this table — generic postgres, no Tamanu coupling.
#[derive(Debug, Clone, Deserialize)]
#[expect(
	dead_code,
	reason = "fields are read by the postgresql snapshot machinery, implemented next"
)]
pub struct PostgresqlConfig {
	/// The cluster name (e.g. `main`); resolves the data dir / connection.
	pub cluster: String,
	/// Override the resolved data directory.
	#[serde(default)]
	pub data_dir: Option<PathBuf>,
	/// Override the resolved major version.
	#[serde(default)]
	pub version: Option<String>,
	/// Override the port used to connect for `CHECKPOINT`.
	#[serde(default)]
	pub port: Option<u16>,
	/// Override the unix socket directory used to connect.
	#[serde(default)]
	pub socket: Option<PathBuf>,
	/// Force a snapshot strategy instead of auto-detecting (for testing).
	#[serde(default)]
	pub strategy: Option<String>,
}

/// A built-in backup method, selected by the def's single method table.
#[derive(Debug, Clone)]
pub enum Method {
	Simple(SimpleConfig),
	Postgresql(PostgresqlConfig),
}

impl Method {
	/// The method's name, used in diagnostics.
	pub fn name(&self) -> &'static str {
		match self {
			Method::Simple(_) => "simple",
			Method::Postgresql(_) => "postgresql",
		}
	}

	/// Produce the source kopia will snapshot.
	pub async fn prepare(&self) -> Result<Prepared> {
		match self {
			Method::Simple(config) => Ok(Prepared {
				path: config.path.clone(),
				extra_tags: BTreeMap::new(),
			}),
			Method::Postgresql(_config) => {
				// Implemented in the postgresql submodule (btrfs first); wired
				// here once the snapshot machinery lands.
				Err(miette::miette!(
					"the postgresql backup method is not implemented yet"
				))
			}
		}
	}

	/// Release whatever `prepare` set up (snapshot, mount, staging dir).
	pub async fn cleanup(&self, _prepared: Prepared) -> Result<()> {
		match self {
			Method::Simple(_) => Ok(()),
			Method::Postgresql(_) => Ok(()),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn simple_prepare_returns_its_path_and_no_tags() {
		let method = Method::Simple(SimpleConfig {
			path: PathBuf::from("/data/custom"),
		});
		let prepared = method.prepare().await.unwrap();
		assert_eq!(prepared.path, PathBuf::from("/data/custom"));
		assert!(prepared.extra_tags.is_empty());
		method.cleanup(prepared).await.unwrap();
	}

	#[tokio::test]
	async fn postgresql_prepare_is_not_yet_implemented() {
		let method = Method::Postgresql(PostgresqlConfig {
			cluster: "main".into(),
			data_dir: None,
			version: None,
			port: None,
			socket: None,
			strategy: None,
		});
		assert!(method.prepare().await.is_err());
	}
}
