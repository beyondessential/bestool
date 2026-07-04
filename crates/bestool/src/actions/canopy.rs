#[cfg(any(feature = "canopy-register", feature = "canopy-import"))]
use std::io::Read as _;

#[cfg(any(feature = "canopy-register", feature = "canopy-import"))]
use base64::{
	Engine as _,
	engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use clap::{Parser, Subcommand};
#[cfg(any(feature = "canopy-register", feature = "canopy-import"))]
use miette::{IntoDiagnostic as _, bail, miette};
use miette::Result;

use super::Context;

/// Interact with Canopy.
#[derive(Debug, Clone, Parser)]
pub struct CanopyArgs {
	/// Canopy subcommand
	#[command(subcommand)]
	pub action: Action,
}

super::subcommands! {
	[CanopyArgs => |args: CanopyArgs, mut ctx: Context| -> Result<(Action, Context)> {
		let action = args.action.clone();
		ctx.provide(args);
		Ok((action, ctx))
	}]

	#[cfg(feature = "canopy-register")]
	register => Register(RegisterArgs),
	#[cfg(feature = "canopy-export")]
	export => Export(ExportArgs),
	#[cfg(feature = "canopy-import")]
	import => Import(ImportArgs),
	#[cfg(feature = "canopy-tags")]
	tags => Tags(TagsArgs),
	#[cfg(feature = "canopy-backup")]
	backup => Backup(BackupArgs),
	#[cfg(feature = "canopy-restore")]
	restore => Restore(RestoreArgs),
	#[cfg(feature = "canopy-restore")]
	kopia => Kopia(KopiaArgs),
	#[cfg(feature = "canopy-unregister")]
	unregister => Unregister(UnregisterArgs)
}

/// Load the registration for a command that takes an optional `--config <DIR>`.
///
/// With an explicit dir, reads exactly that dir. With the default location,
/// uses the migration-aware loader so a legacy host that the daemon hasn't
/// migrated yet is still picked up.
#[cfg(any(feature = "canopy-register", feature = "canopy-export"))]
async fn load_registration(
	config: Option<&std::path::Path>,
) -> Result<Option<bestool_canopy::registration::Registration>> {
	match config {
		Some(dir) => bestool_canopy::registration::load_from(dir).await,
		None => bestool_canopy::registration::load().await,
	}
}

/// Ask a running alertd daemon to restart so it re-reads the registration.
///
/// The daemon reads the device key and server id once at startup and caches
/// them for its lifetime, so enrolling or unenrolling only takes effect after
/// it restarts. Best-effort: a daemon that isn't running (or isn't reachable)
/// is fine — it reads the registration afresh whenever it next starts.
///
/// Only wired up when the daemon is built into this binary; otherwise there's
/// no control client to reach it with.
#[cfg(all(
	feature = "alertd",
	any(feature = "canopy-register", feature = "canopy-unregister")
))]
async fn restart_daemon_for_registration_change() {
	let addrs = bestool_alertd::commands::default_server_addrs();
	if let Err(err) = bestool_alertd::commands::restart(&addrs).await {
		tracing::debug!(%err, "alertd daemon not reachable");
		println!("(alertd daemon not reachable; it will re-read the registration on next start)");
	}
}

#[cfg(all(
	not(feature = "alertd"),
	any(feature = "canopy-register", feature = "canopy-unregister")
))]
async fn restart_daemon_for_registration_change() {}

/// Elevate up front when the registration can't be written from here.
///
/// `canopy register` / `import` only write the registration at the very end —
/// after prompting for a passphrase, and (for register) after consuming the
/// one-shot enrollment token over the network. If the registration directory
/// isn't writable — the common case on a deployed host where `/etc/bestool` is
/// root-owned — failing at that last step wastes all of it. So check up front
/// and, when we aren't already privileged, re-exec the whole command under
/// sudo; the elevated run does the prompt and the write.
///
/// Returns `Ok(())` to proceed in-process when we can already write, or when
/// we're root and sudo wouldn't change anything (let the operation run and
/// surface any genuine error, e.g. a read-only filesystem). Non-Unix is always
/// a no-op: there's no sudo, and the dir's ACLs govern writability directly.
#[cfg(all(
	unix,
	any(
		feature = "canopy-register",
		feature = "canopy-import",
		feature = "canopy-unregister"
	)
))]
fn ensure_writable_or_reexec(dir: &std::path::Path) -> Result<()> {
	if registration_dir_writable(dir) || privilege::user::privileged() {
		return Ok(());
	}

	tracing::info!(
		dir = %dir.display(),
		"registration directory is not writable; re-executing under sudo"
	);
	let args: Vec<String> = std::env::args().collect();
	let status = std::process::Command::new("sudo")
		.args(args)
		.status()
		.into_diagnostic()?;
	std::process::exit(status.code().unwrap_or(1));
}

#[cfg(all(
	not(unix),
	any(
		feature = "canopy-register",
		feature = "canopy-import",
		feature = "canopy-unregister"
	)
))]
fn ensure_writable_or_reexec(_dir: &std::path::Path) -> Result<()> {
	Ok(())
}

/// Whether we can create the registration file in `dir`. Storing creates the
/// directory if missing and then writes a file inside it, so we test the
/// nearest existing ancestor for "can create an entry here" by actually trying
/// — more reliable than reasoning about mode bits, ACLs, ownership, and setgid.
#[cfg(all(
	unix,
	any(
		feature = "canopy-register",
		feature = "canopy-import",
		feature = "canopy-unregister"
	)
))]
fn registration_dir_writable(dir: &std::path::Path) -> bool {
	let mut candidate = dir;
	let existing = loop {
		if candidate.exists() {
			break candidate;
		}
		match candidate.parent() {
			Some(parent) => candidate = parent,
			None => return false,
		}
	};

	let probe = existing.join(format!(".bestool-write-test.{}", std::process::id()));
	match std::fs::File::create(&probe) {
		Ok(_) => {
			let _ = std::fs::remove_file(&probe);
			true
		}
		Err(_) => false,
	}
}

/// Read base64 input from stdin, erroring if it's empty.
#[cfg(any(feature = "canopy-register", feature = "canopy-import"))]
fn read_stdin(what: &str) -> Result<String> {
	let mut buf = String::new();
	std::io::stdin()
		.read_to_string(&mut buf)
		.into_diagnostic()
		.map_err(|e| miette!("reading {what} from stdin: {e}"))?;
	if buf.trim().is_empty() {
		bail!("no {what} given on the command line or stdin");
	}
	Ok(buf)
}

/// Base64-decode input, accepting every variant Canopy's lenient encoder might
/// produce (standard / no-pad / url-safe / url-safe-no-pad).
#[cfg(any(feature = "canopy-register", feature = "canopy-import"))]
fn decode_base64(input: &str) -> Result<Vec<u8>> {
	for engine in [&STANDARD, &STANDARD_NO_PAD, &URL_SAFE, &URL_SAFE_NO_PAD] {
		if let Ok(bytes) = engine.decode(input) {
			return Ok(bytes);
		}
	}
	Err(miette!("input is not valid base64"))
}

#[cfg(test)]
#[cfg(any(feature = "canopy-register", feature = "canopy-import"))]
mod tests {
	use super::*;

	#[test]
	fn decode_base64_accepts_all_variants() {
		let raw = b"\x00\xff\x10hello world?!";
		for encoded in [
			STANDARD.encode(raw),
			STANDARD_NO_PAD.encode(raw),
			URL_SAFE.encode(raw),
			URL_SAFE_NO_PAD.encode(raw),
		] {
			assert_eq!(decode_base64(&encoded).unwrap(), raw);
		}
	}

	#[test]
	fn decode_base64_rejects_garbage() {
		assert!(decode_base64("not valid base64 !!!! \u{00a0}").is_err());
	}

	#[cfg(unix)]
	#[test]
	fn registration_dir_writable_true_for_writable_dir() {
		let dir = std::env::temp_dir().join(format!("bestool-canopy-rw-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		assert!(registration_dir_writable(&dir), "fresh temp dir should be writable");
		// A not-yet-created subpath is writable too, via its nearest ancestor.
		assert!(registration_dir_writable(&dir.join("missing/deeper")));
		std::fs::remove_dir_all(&dir).ok();
	}

	#[cfg(unix)]
	#[test]
	fn registration_dir_writable_false_for_readonly_dir() {
		use std::os::unix::fs::PermissionsExt as _;
		// Root bypasses directory write bits, so this only holds unprivileged.
		if privilege::user::privileged() {
			return;
		}
		let dir = std::env::temp_dir().join(format!("bestool-canopy-ro-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
		assert!(!registration_dir_writable(&dir), "0500 dir must not be writable");
		std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok();
		std::fs::remove_dir_all(&dir).ok();
	}
}
