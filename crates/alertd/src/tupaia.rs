//! The Tupaia application on this machine.
//!
//! spec: SUBJ

use std::path::Path;

/// The application type as canopy names it.
pub const TYPE_SLUG: &str = "tupaia";

/// The root manifest of the Tupaia checkout, whose presence says Tupaia is
/// installed here.
pub const PACKAGE_JSON: &str = "/home/ubuntu/tupaia/package.json";

/// The git directory of the Tupaia checkout.
pub const GIT_DIR: &str = "/home/ubuntu/tupaia/.git";

/// The `name` the monorepo's root manifest gives itself.
const ROOT_PACKAGE_NAME: &str = "tupaia";

/// Whether Tupaia is installed on this machine.
pub fn installed() -> bool {
	std::fs::read_to_string(PACKAGE_JSON).is_ok_and(|manifest| is_root_manifest(&manifest))
}

/// Whether a `package.json` is the Tupaia monorepo's root manifest.
pub fn is_root_manifest(manifest: &str) -> bool {
	serde_json::from_str::<serde_json::Value>(manifest)
		.ok()
		.and_then(|manifest| {
			manifest
				.get("name")
				.and_then(|name| name.as_str())
				.map(|name| name == ROOT_PACKAGE_NAME)
		})
		.unwrap_or(false)
}

/// The installed product version: the commit the checkout has checked out.
pub fn version() -> Option<String> {
	checked_out_commit(Path::new(GIT_DIR))
}

/// The full hash of the commit `HEAD` names in a git directory, read from its
/// files rather than by running git.
///
/// `HEAD` is either a hash, when detached, or a symbolic ref, resolved through
/// its loose ref file or else `packed-refs`. Anything else is no version.
pub fn checked_out_commit(git_dir: &Path) -> Option<String> {
	let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
	let head = head.trim();
	let Some(name) = head.strip_prefix("ref:") else {
		return as_hash(head);
	};
	let name = name.trim();
	if !name.starts_with("refs/") || name.split('/').any(|part| part.is_empty() || part == "..") {
		return None;
	}

	if let Ok(loose) = std::fs::read_to_string(git_dir.join(name)) {
		return as_hash(loose.trim());
	}

	let packed = std::fs::read_to_string(git_dir.join("packed-refs")).ok()?;
	packed.lines().find_map(|line| {
		let (hash, packed_name) = line.split_once(' ')?;
		(packed_name.trim() == name)
			.then(|| as_hash(hash))
			.flatten()
	})
}

fn as_hash(text: &str) -> Option<String> {
	(matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit()))
		.then(|| text.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
	use super::*;

	const HASH: &str = "0123456789abcdef0123456789abcdef01234567";
	const OTHER: &str = "89abcdef0123456789abcdef0123456789abcdef";

	fn git_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
		let dir = tempfile::tempdir().unwrap();
		for (path, contents) in files {
			let path = dir.path().join(path);
			std::fs::create_dir_all(path.parent().unwrap()).unwrap();
			std::fs::write(path, contents).unwrap();
		}
		dir
	}

	#[test]
	fn only_the_monorepo_root_manifest_is_tupaia() {
		assert!(is_root_manifest(
			r#"{"name": "tupaia", "private": true, "workspaces": ["packages/*"]}"#
		));
		assert!(!is_root_manifest(r#"{"name": "@tupaia/central-server"}"#));
		assert!(!is_root_manifest(r#"{"private": true}"#));
		assert!(!is_root_manifest("not json"));
	}

	#[test]
	fn a_branch_resolves_through_its_loose_ref() {
		let dir = git_dir(&[
			("HEAD", "ref: refs/heads/dev\n"),
			("refs/heads/dev", &format!("{HASH}\n")),
			("packed-refs", &format!("{OTHER} refs/heads/dev\n")),
		]);
		assert_eq!(checked_out_commit(dir.path()).as_deref(), Some(HASH));
	}

	#[test]
	fn a_branch_resolves_through_packed_refs_without_a_loose_ref() {
		let dir = git_dir(&[
			("HEAD", "ref: refs/heads/feature/x\n"),
			(
				"packed-refs",
				&format!(
					"# pack-refs with: peeled fully-peeled sorted\n{OTHER} refs/heads/dev\n{HASH} refs/heads/feature/x\n^{OTHER}\n"
				),
			),
		]);
		assert_eq!(checked_out_commit(dir.path()).as_deref(), Some(HASH));
	}

	#[test]
	fn a_detached_head_is_its_own_commit() {
		let dir = git_dir(&[("HEAD", &format!("{}\n", HASH.to_uppercase()))]);
		assert_eq!(checked_out_commit(dir.path()).as_deref(), Some(HASH));
	}

	#[test]
	fn an_unresolvable_head_is_no_version() {
		assert_eq!(checked_out_commit(git_dir(&[]).path()), None);
		let dir = git_dir(&[("HEAD", "ref: refs/heads/gone\n")]);
		assert_eq!(checked_out_commit(dir.path()), None);
		let dir = git_dir(&[("HEAD", "ref: refs/../../escape\n")]);
		assert_eq!(checked_out_commit(dir.path()), None);
		let dir = git_dir(&[("HEAD", "garbage\n")]);
		assert_eq!(checked_out_commit(dir.path()), None);
	}
}
