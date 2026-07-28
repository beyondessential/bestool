//! Find out what is keeping a directory tree busy.
//!
//! Windows refuses to rename a directory while a process is running an
//! executable from inside it, or has its working directory there, and the error
//! it hands back — "access is denied" — names nothing an operator can act on.
//! That bites a whole-install postgres restore in particular: the tree being
//! swapped holds `bin\postgres.exe`, whose image file stays locked until the
//! process object is torn down, which outlasts the service reporting stopped.

use std::{
	fmt,
	path::{Path, PathBuf},
};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// A process using a tree, and what ties it there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
	pub pid: u32,
	pub name: String,
	pub reason: Reason,
	/// The path that put it in the list (its executable or working directory).
	pub path: PathBuf,
}

/// Why a process counts as using a tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
	/// It's running an executable from inside the tree.
	Running,
	/// Its working directory is inside the tree.
	WorkingDirectory,
}

impl fmt::Display for Holder {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let Self {
			pid, name, path, ..
		} = self;
		match self.reason {
			Reason::Running => write!(f, "{name} (pid {pid}) running {}", path.display()),
			Reason::WorkingDirectory => {
				write!(f, "{name} (pid {pid}) working in {}", path.display())
			}
		}
	}
}

/// The processes running from, or sitting in, `dir`.
///
/// Best-effort and inherently racy — a process can come or go between the scan
/// and whatever the caller does next — and blind to a process that merely holds
/// an open handle from elsewhere (an antivirus scan, a backup agent). An empty
/// result means "none found", not "none".
pub fn holders_of(dir: &Path) -> Vec<Holder> {
	let mut sys = System::new();
	sys.refresh_processes_specifics(
		ProcessesToUpdate::All,
		true,
		ProcessRefreshKind::nothing()
			.with_exe(UpdateKind::Always)
			.with_cwd(UpdateKind::Always),
	);

	let mut out: Vec<Holder> = sys
		.processes()
		.iter()
		.filter_map(|(pid, proc)| {
			let name = proc.name().to_string_lossy().into_owned();
			let (reason, path) = match (proc.exe(), proc.cwd()) {
				(Some(exe), _) if is_under(exe, dir) => (Reason::Running, exe),
				(_, Some(cwd)) if is_under(cwd, dir) => (Reason::WorkingDirectory, cwd),
				_ => return None,
			};
			Some(Holder {
				pid: pid.as_u32(),
				name,
				reason,
				path: path.to_path_buf(),
			})
		})
		.collect();
	out.sort_by_key(|holder| holder.pid);
	out
}

/// A one-line description of what's holding `dir`, to append to the error from a
/// failed swap. Falls back to pointing at the tools that find the holders this
/// can't see.
pub fn describe_holders(dir: &Path) -> String {
	let holders = holders_of(dir);
	if holders.is_empty() {
		return "\nnothing was found running from it: look for an open handle \
			(Sysinternals handle64.exe, or Resource Monitor's associated-handles \
			search) and check this is running elevated"
			.into();
	}

	let list = holders
		.iter()
		.map(|holder| format!("\n  - {holder}"))
		.collect::<String>();
	format!("\nstill in use by:{list}")
}

/// Whether `path` is `dir` or below it, matching the way the filesystem compares
/// them (Windows paths are case-insensitive).
fn is_under(path: &Path, dir: &Path) -> bool {
	if cfg!(windows) {
		let path = path.to_string_lossy().to_lowercase();
		let dir = dir.to_string_lossy().to_lowercase();
		Path::new(&path).starts_with(Path::new(&dir))
	} else {
		path.starts_with(dir)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn under_matches_the_tree_but_not_a_name_prefix() {
		let dir = Path::new("/srv/pg/16");
		assert!(is_under(Path::new("/srv/pg/16"), dir));
		assert!(is_under(Path::new("/srv/pg/16/bin/postgres"), dir));
		assert!(!is_under(Path::new("/srv/pg/166/bin/postgres"), dir));
		assert!(!is_under(Path::new("/srv/pg"), dir));
	}

	#[cfg(windows)]
	#[test]
	fn under_ignores_case_on_windows() {
		assert!(is_under(
			Path::new(r"c:\program files\postgresql\12\bin\postgres.exe"),
			Path::new(r"C:\Program Files\PostgreSQL\12")
		));
	}

	#[test]
	fn this_process_holds_its_own_executable() {
		let exe = std::env::current_exe().unwrap();
		let dir = exe.parent().unwrap();
		let me = std::process::id();
		let holders = holders_of(dir);
		assert!(
			holders.iter().any(|holder| holder.pid == me),
			"expected pid {me} among {holders:?}"
		);
	}

	#[test]
	fn an_unused_tree_has_no_holders_and_describes_the_next_step() {
		let tmp = tempfile::tempdir().unwrap();
		assert!(holders_of(tmp.path()).is_empty());
		assert!(describe_holders(tmp.path()).contains("handle64"));
	}
}
