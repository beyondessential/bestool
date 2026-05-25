//! Thin layer between the parent `KopiaArgs` and the shared `bestool-kopia`
//! crate. The actual logic (binary location, elevation, snapshot types,
//! filters, picker, …) lives in `bestool-kopia` so the doctor check and
//! this subcommand share one implementation; leaf commands import the
//! pieces they need from `bestool_kopia` directly.

use bestool_kopia::{Elevation, LINUX_KOPIA_USER, find_kopia_binary, linux_elevation};
use miette::{Result, miette};
use tracing::debug;

use super::KopiaArgs;

/// Resolve the kopia binary to invoke, honouring `--kopia-bin` on `KopiaArgs`.
pub fn kopia_binary(args: &KopiaArgs) -> Result<std::path::PathBuf> {
	find_kopia_binary(args.kopia_bin.as_deref()).ok_or_else(|| {
		miette!("could not find a `kopia` binary in PATH; install kopia or pass --kopia-bin")
	})
}

/// On Linux, if there's a system kopia install and we're a non-`kopia`
/// user, re-exec the current command under `sudo -u kopia --`. If `sudo`
/// can't elevate (no NOPASSWD rule, no TTY), the resulting kopia invocation
/// will fail loudly. The `--no-sudo` flag on `KopiaArgs` skips this
/// entirely.
///
/// Doesn't return on a successful re-exec — `exec` replaces the process
/// image.
pub fn maybe_reexec_as_kopia(args: &KopiaArgs) -> Result<()> {
	if args.no_sudo {
		return Ok(());
	}
	if !cfg!(target_os = "linux") {
		return Ok(());
	}
	match linux_elevation() {
		Elevation::Direct => Ok(()),
		Elevation::Sudo => {
			debug!("re-executing under sudo -u {LINUX_KOPIA_USER}");
			exec_under_sudo(LINUX_KOPIA_USER)
		}
		Elevation::Skip(reason) => {
			debug!("not re-executing under kopia: {reason}");
			Ok(())
		}
	}
}

/// Replace the current process with `sudo -u <user> -- <argv>`. Only
/// returns if the exec itself failed.
#[cfg(unix)]
fn exec_under_sudo(user: &str) -> Result<()> {
	use std::os::unix::process::CommandExt;

	let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
	let Some((exe, rest)) = argv.split_first() else {
		return Err(miette!("no argv to re-exec"));
	};

	let mut cmd = std::process::Command::new("sudo");
	cmd.arg("-u").arg(user).arg("--").arg(exe).args(rest);
	let err = cmd.exec();
	Err(miette!("failed to re-exec under sudo: {err}"))
}

#[cfg(not(unix))]
fn exec_under_sudo(_user: &str) -> Result<()> {
	Err(miette!("sudo re-exec only supported on Unix"))
}
