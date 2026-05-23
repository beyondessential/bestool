use std::{
	fmt::Debug,
	fs,
	path::{Path, PathBuf},
};

use itertools::Itertools;
use miette::{IntoDiagnostic, Result, miette};
use node_semver::Version;
use tracing::{debug, instrument};

pub mod config;
pub mod connection_url;
pub mod pm2;
pub mod roots;
pub mod server_info;
pub mod services;

#[cfg(feature = "doctor")]
pub mod doctor;

/// What kind of server to interact with.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ApiServerKind {
	Central,
	Facility,
}

impl ApiServerKind {
	pub fn package_name(&self) -> &'static str {
		match self {
			Self::Central => "central-server",
			Self::Facility => "facility-server",
		}
	}

	pub fn from_str_ci(s: &str) -> Option<Self> {
		match s {
			"central" | "central-server" => Some(Self::Central),
			"facility" | "facility-server" => Some(Self::Facility),
			_ => None,
		}
	}
}

#[instrument(level = "debug")]
pub fn find_tamanu(root: Option<&Path>) -> Result<(Version, PathBuf)> {
	#[inline]
	fn inner(root: Option<&Path>) -> Result<(Version, PathBuf)> {
		if let Some(root) = root {
			let version = roots::version_of_root(root)?
				.ok_or_else(|| miette!("no tamanu found in --root={root:?}"))?;
			Ok((version, root.canonicalize().into_diagnostic()?))
		} else {
			roots::find_versions()?
				.into_iter()
				.next()
				.ok_or_else(|| miette!("no tamanu discovered, use --root"))
		}
	}

	inner(root).inspect(|(version, root)| debug!(?root, ?version, "found Tamanu root"))
}

#[instrument(level = "debug")]
pub fn find_package(root: impl AsRef<Path> + Debug) -> ApiServerKind {
	fn inner(root: &Path) -> Result<ApiServerKind> {
		fs::read_dir(root.join("packages"))
			.into_diagnostic()?
			.filter_map_ok(|e| e.file_name().into_string().ok())
			.process_results(|mut iter| {
				iter.find_map(|dir_name| ApiServerKind::from_str_ci(&dir_name))
					.ok_or_else(|| miette!("Tamanu servers not found"))
			})
			.into_diagnostic()?
	}

	inner(root.as_ref())
		.inspect(|kind| debug!(?root, ?kind, "using this Tamanu for config"))
		.map_err(|err| debug!(?err, "failed to detect package, assuming facility"))
		.unwrap_or(ApiServerKind::Facility)
}
