use std::path::{Path, PathBuf};

use leon::Template;
use leon_macros::template;
use miette::{IntoDiagnostic, Result};
use node_semver::Version;
use serde::Deserialize;
use tracing::{debug, instrument, trace};

const KNOWN_ROOTS: &[Template<'static>] = &[
	// container
	template!("/app"),
	// linux installs
	template!("/opt/bes/tamanu"),
	template!("/var/lib/tamanu"),
	// windows
	template!("/Tamanu/[12].*.*"),
	template!("/Tamanu/release-[12].*.*"),
	template!("/Tamanu/release-v[12].*.*"),
	// dev
	template!("{ home }/tamanu"),
	template!("{ home }/projects/tamanu"),
	template!("{ home }/work/tamanu"),
	template!("{ home }/code/js/tamanu"),
];

#[instrument(level = "debug")]
pub fn find_roots() -> Result<Vec<PathBuf>> {
	let home = dirs::home_dir().map_or("/home".into(), |path| path.to_string_lossy().into_owned());
	debug!(?home, "home directory");

	let mut paths = Vec::new();
	for template in KNOWN_ROOTS {
		let pattern = template.render(&[("home", &home)])?;
		debug!(?pattern, "searching for root in pattern");
		for entry in glob::glob(&pattern).into_diagnostic()? {
			if let Ok(path) = entry {
				trace!(?path, "found possible root");
				paths.push(path);
			}
		}
	}

	Ok(paths)
}

#[instrument(level = "debug")]
pub fn version_of_root(root: impl AsRef<Path> + std::fmt::Debug) -> Result<Option<Version>> {
	let root = root.as_ref();
	let pkg_file = root.join("package.json");
	if !pkg_file.exists() {
		return Ok(None);
	}
	trace!(?pkg_file, "found package.json");

	let pkg_json: PackageJson =
		json5::from_str(&std::fs::read_to_string(pkg_file).into_diagnostic()?).into_diagnostic()?;
	trace!(?pkg_json, "read package.json");
	Ok(Some(pkg_json.version.clone()))
}

#[derive(Debug, Clone, Deserialize)]
struct PackageJson {
	pub version: Version,
}

#[instrument(level = "debug")]
pub fn find_versions() -> Result<Vec<(Version, PathBuf)>> {
	let mut roots: Vec<_> = find_roots()?
		.into_iter()
		.filter_map(|root| {
			version_of_root(root.clone())
				.ok()
				.and_then(|v| v.map(|v| (v, root)))
		})
		.collect();

	roots.sort_by_key(|(v, _)| v.clone());
	roots.reverse();
	Ok(roots)
}
