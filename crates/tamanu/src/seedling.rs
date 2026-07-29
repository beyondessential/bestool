//! Recognising a Seedling host, and speaking its operator interface.
//!
//! On a host that runs the Seedling application orchestrator, Tamanu is a
//! Seedling app rather than a set of units or pm2 processes, so the lifecycle,
//! log, and database commands act through the daemon.
//!
//! Everything that speaks the interface itself lives in [`oi`], behind the
//! `seedling` feature. What stays here is the part of a Seedling host that
//! needs no protocol client to observe: where its state lives, and whether it
//! carries an identity for us.
//!
//! spec: SEED

use std::{
	io,
	path::{Path, PathBuf},
};

use miette::{Report, miette};
use tracing::{debug, info, warn};

#[cfg(feature = "seedling")]
mod oi;
#[cfg(feature = "seedling")]
pub use oi::*;

/// The daemon's service unit, installed and enabled by the Seedling package.
pub const DAEMON_UNIT: &str = "seedling.service";

/// Where the Seedling package keeps the daemon's state. Root-only, so an
/// unprivileged process can read neither the daemon's published identity nor
/// the authorised keys it holds.
pub const DATA_DIR: &str = "/var/lib/seedling";

/// The identity the Seedling package generates for us and authorises with the
/// daemon, so these commands work on a freshly provisioned host with nothing
/// configured for any operator.
///
/// The file is owned by root and readable by nobody else, which is what keeps
/// it from handing daemon access to every local user.
const HOST_KEY: &str = "/etc/bestool/seedling.key";

/// Environment variable relocating the host identity. Set by tests, and
/// honoured for ad-hoc relocation.
const KEY_ENV: &str = "BESTOOL_SEEDLING_KEY";

/// This host's Seedling identity for us, and whether this process can use it.
///
/// spec: SEED#speaking-the-operator-interface
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostIdentity {
	/// The host carries no identity for us: only an operator's own key can
	/// reach the daemon.
	Absent,
	/// Present, and this process can read it.
	Readable(PathBuf),
	/// Present, but reading it needs privileges this process does not have.
	NeedsElevation(PathBuf),
}

impl HostIdentity {
	/// Whether the host carries an identity for us at all, regardless of
	/// whether this process can read it.
	pub fn present(&self) -> bool {
		!matches!(self, Self::Absent)
	}

	/// Whether reaching this identity requires running elevated.
	pub fn needs_elevation(&self) -> bool {
		matches!(self, Self::NeedsElevation(_))
	}
}

/// Where the host identity lives.
pub fn host_key_path() -> PathBuf {
	std::env::var_os(KEY_ENV)
		.map(PathBuf::from)
		.unwrap_or_else(|| PathBuf::from(HOST_KEY))
}

/// Classify this host's identity for us.
///
/// Readability is settled by opening the file rather than by reading its mode:
/// ownership, mode bits, ACLs, and the capabilities of the running process all
/// bear on the answer, and only the kernel knows all of them.
pub fn host_identity() -> HostIdentity {
	let path = host_key_path();
	let probe = std::fs::File::open(&path).map(drop);
	let identity = classify(path, probe, privileged());
	debug!(?identity, "resolved the host's Seedling identity");
	identity
}

fn classify(path: PathBuf, probe: io::Result<()>, privileged: bool) -> HostIdentity {
	match probe {
		Ok(()) => HostIdentity::Readable(path),
		Err(err) if err.kind() == io::ErrorKind::NotFound => HostIdentity::Absent,
		// Privileged and still refused: elevating again would not help, so
		// treat it as no identity rather than looping through sudo.
		Err(err) if err.kind() == io::ErrorKind::PermissionDenied && !privileged => {
			HostIdentity::NeedsElevation(path)
		}
		Err(err) => {
			warn!(?path, %err, "cannot read the host's Seedling identity");
			HostIdentity::Absent
		}
	}
}

/// Re-run this command elevated, exiting with the status it returns.
///
/// A Seedling host keeps both the daemon's published identity and our own
/// identity under root-only paths, so an unprivileged invocation of a
/// Seedling-aware command cannot get far. Rather than telling the operator to
/// run it again themselves, run it for them, the same way the host service
/// manager path elevates itself before touching rootful podman.
///
/// Returns only when sudo could not be run at all, hence the error return.
///
/// spec: SEED#speaking-the-operator-interface
pub async fn elevate(reason: &str) -> Report {
	if !cfg!(unix) {
		return miette!("{reason}, and this platform cannot elevate to reach it");
	}

	info!(reason, "re-running elevated");
	let args: Vec<String> = std::env::args().collect();
	match tokio::process::Command::new("sudo")
		.args(args)
		.status()
		.await
	{
		Ok(status) => std::process::exit(status.code().unwrap_or(1)),
		Err(err) => miette!("{reason}, and elevating to reach it failed: {err}"),
	}
}

/// Whether this process is running as root.
#[cfg(target_os = "linux")]
fn privileged() -> bool {
	rustix::process::geteuid().is_root()
}

#[cfg(not(target_os = "linux"))]
fn privileged() -> bool {
	false
}

/// The daemon's published identity, which a co-located client pins instead of
/// prompting or keeping a store of previously seen daemons.
pub fn fingerprint_path(data_dir: &Path) -> PathBuf {
	data_dir.join("oi.fingerprint")
}

#[cfg(test)]
mod tests {
	use super::*;

	fn path() -> PathBuf {
		PathBuf::from("/etc/bestool/seedling.key")
	}

	#[test]
	fn a_readable_key_is_usable_as_is() {
		let id = classify(path(), Ok(()), false);
		assert_eq!(id, HostIdentity::Readable(path()));
		assert!(id.present());
		assert!(!id.needs_elevation());
	}

	#[test]
	fn a_missing_key_is_absent() {
		let id = classify(path(), Err(io::Error::from(io::ErrorKind::NotFound)), false);
		assert_eq!(id, HostIdentity::Absent);
		assert!(!id.present());
		assert!(!id.needs_elevation());
	}

	#[test]
	fn a_refused_key_needs_elevation_when_unprivileged() {
		let id = classify(
			path(),
			Err(io::Error::from(io::ErrorKind::PermissionDenied)),
			false,
		);
		assert_eq!(id, HostIdentity::NeedsElevation(path()));
		assert!(id.present(), "the host does carry an identity for us");
		assert!(id.needs_elevation());
	}

	#[test]
	fn a_refused_key_is_absent_when_already_privileged() {
		// Elevating again would land in the same place, so this must not
		// report as elevatable or the command would loop through sudo.
		let id = classify(
			path(),
			Err(io::Error::from(io::ErrorKind::PermissionDenied)),
			true,
		);
		assert_eq!(id, HostIdentity::Absent);
		assert!(!id.needs_elevation());
	}

	#[test]
	fn an_unreadable_key_for_any_other_reason_is_absent() {
		let id = classify(
			path(),
			Err(io::Error::from(io::ErrorKind::InvalidData)),
			false,
		);
		assert_eq!(id, HostIdentity::Absent);
	}

	#[test]
	fn a_real_file_reads_as_readable() {
		let dir = tempfile::tempdir().unwrap();
		let key = dir.path().join("seedling.key");
		std::fs::write(&key, b"not really a key").unwrap();

		let probe = std::fs::File::open(&key).map(drop);
		assert_eq!(
			classify(key.clone(), probe, false),
			HostIdentity::Readable(key)
		);
	}
}
