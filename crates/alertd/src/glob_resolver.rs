use std::{
	collections::HashSet,
	path::{Path, PathBuf},
};

use glob::glob;
use miette::{IntoDiagnostic, Result};
use tracing::{debug, warn};

/// Resolves glob patterns to concrete paths (directories and files)
#[derive(Debug, Clone)]
pub struct GlobResolver {
	patterns: Vec<String>,
}

impl GlobResolver {
	pub fn new(patterns: Vec<String>) -> Self {
		Self { patterns }
	}

	/// Resolve all glob patterns to concrete paths that exist
	///
	/// Returns directories and files separately for different handling
	pub fn resolve(&self) -> Result<ResolvedPaths> {
		let mut dirs = HashSet::new();
		let mut files = HashSet::new();

		for pattern in &self.patterns {
			debug!(?pattern, "resolving glob pattern");

			let entries = glob(pattern).into_diagnostic()?;

			for entry in entries {
				match entry {
					Ok(path) => {
						if path.is_dir() {
							debug!(?path, "resolved to directory");
							dirs.insert(path);
						} else if path.is_file() {
							debug!(?path, "resolved to file");
							files.insert(path);
						} else {
							debug!(?path, "skipping non-file, non-directory");
						}
					}
					Err(e) => {
						warn!("glob error for pattern {}: {}", pattern, e);
					}
				}
			}
		}

		Ok(ResolvedPaths {
			dirs: dirs.into_iter().collect(),
			files: files.into_iter().collect(),
		})
	}
}

/// Paths resolved from glob patterns
#[derive(Debug, Clone)]
pub struct ResolvedPaths {
	/// Directories that match the patterns
	pub dirs: Vec<PathBuf>,
	/// Individual files that match the patterns
	pub files: Vec<PathBuf>,
}

impl ResolvedPaths {
	/// Get all unique paths (both dirs and files)
	pub fn all_paths(&self) -> Vec<&Path> {
		self.dirs
			.iter()
			.map(|p| p.as_path())
			.chain(self.files.iter().map(|p| p.as_path()))
			.collect()
	}

	/// Check if this set of paths differs from another
	pub fn differs_from(&self, other: &ResolvedPaths) -> bool {
		let self_set: HashSet<_> = self.all_paths().into_iter().collect();
		let other_set: HashSet<_> = other.all_paths().into_iter().collect();
		self_set != other_set
	}
}
