//! Asking the block layer about the volume a restore is writing to.
//!
//! Both the [btrfs](super::basis::btrfs) and [thin-LVM](super::basis::lvm) diff
//! bases, and the [space gate](super::room), have to ask `findmnt`, `lvs` and
//! `dmsetup` the same small set of questions. They ask them here so that one
//! restore does not resolve the same thin pool twice, and so the awkward parts —
//! device-mapper's name mangling, arguments that could be read as options — are
//! got right in one place rather than three.
//!
//! Every query returns `None` rather than an error. The callers all degrade to
//! something slower when the block layer cannot answer, and none of them should
//! refuse a rollback over a tool that is not installed.

#![cfg(unix)]

use std::path::Path;

use tokio::process::Command;
use tracing::debug;

/// Run a command and take its standard output, or `None` if it could not be run
/// or did not succeed.
pub async fn capture(program: &str, args: &[&str]) -> Option<String> {
	let output = Command::new(program)
		.args(args)
		.stdin(std::process::Stdio::null())
		.output()
		.await
		.inspect_err(|err| debug!("could not run {program}: {err}"))
		.ok()?;
	if !output.status.success() {
		debug!("{program} {} exited {}", args.join(" "), output.status);
		return None;
	}
	// Taken rather than copied: `find-new` prints a line per *extent*, so hours of
	// divergence on a large cluster is hundreds of MB, and borrowing-then-owning
	// would hold two copies of it at once.
	let mut text = match String::from_utf8(output.stdout) {
		Ok(text) => text,
		Err(err) => String::from_utf8_lossy(err.as_bytes()).into_owned(),
	};
	text.truncate(text.trim_end().len());
	let lead = text.len() - text.trim_start().len();
	text.drain(..lead);
	Some(text)
}

/// Run a command for its effect alone.
pub async fn run(program: &str, args: &[&str]) -> Option<()> {
	capture(program, args).await.map(|_| ())
}

/// One field of the mount backing `path`, e.g. `TARGET` for its mount point or
/// `SOURCE` for the device.
pub async fn findmnt(field: &str, path: &Path) -> Option<String> {
	// `--` so a path that begins with a dash is a path and not an option.
	capture(
		"findmnt",
		&["-n", "-o", field, "--target", "--", &path.to_string_lossy()],
	)
	.await
	.filter(|value| !value.is_empty())
}

/// One field of an LV, named `<vg>/<lv>` or by device path.
///
/// `bytes` asks for a plain byte count rather than the human-readable form
/// `lvs` prints by default.
pub async fn lvs(field: &str, target: &str, bytes: bool) -> Option<String> {
	let mut args = vec!["--noheadings", "-o", field];
	if bytes {
		args.extend(["--units", "b", "--nosuffix"]);
	}
	// `--` so an LV or VG name that begins with a dash cannot be read as an option.
	args.extend(["--", target]);
	capture("lvs", &args).await.filter(|value| !value.is_empty())
}

/// Device-mapper escapes a hyphen in a VG or LV name by doubling it, so a name
/// carrying one does not resolve under its plain spelling — and worse, `vg` +
/// `data-pool` and `vg-data` + `pool` both spell `vg-data-pool` unmangled, which
/// is a different device rather than a missing one.
pub fn mangle(name: &str) -> String {
	name.replace('-', "--")
}

/// The device-mapper name of an LV, as `dmsetup` and the `/dev/mapper` paths
/// spell it.
pub fn dm_name(vg: &str, lv: &str) -> String {
	format!("{}-{}", mangle(vg), mangle(lv))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_hyphen_in_a_name_is_doubled() {
		assert_eq!(mangle("vg-data"), "vg--data");
		assert_eq!(mangle("vg0"), "vg0");
	}

	/// The ambiguity the mangling exists to remove: without it these two
	/// different devices have the same name, so a message meant for one reaches
	/// the other.
	#[test]
	fn mangling_keeps_two_different_devices_apart() {
		assert_ne!(dm_name("vg", "data-pool"), dm_name("vg-data", "pool"));
		assert_eq!(dm_name("vg", "data-pool"), "vg-data--pool");
		assert_eq!(dm_name("vg-data", "pool"), "vg--data-pool");
	}
}
