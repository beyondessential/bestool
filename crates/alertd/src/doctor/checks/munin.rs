//! Reports whether munin-node is present on the host, as a top-level status
//! fact for canopy (`munin: bool`).
//!
//! Not a health signal — always a pass, carried in `payload_extras` like the
//! timezone — so the fleet can see which hosts can be harvested by munin
//! (which scrapes alertd's `/metrics` in munin format).

#[cfg(target_os = "linux")]
use bestool_tamanu::systemd;

use super::SweepContext;
use crate::doctor::check::Check;

const NAME: &str = "munin";
#[cfg(target_os = "linux")]
const UNIT: &str = "munin-node.service";

pub async fn run(_ctx: SweepContext) -> Check {
	build(detect().await)
}

/// Build the fact-check from the detection result. Split from [`run`] so tests
/// exercise it without opening the shared systemd D-Bus connection, which is
/// cached process-wide and hangs when reused across per-test tokio runtimes.
fn build(detected: bool) -> Check {
	let summary = if detected {
		"munin-node present"
	} else {
		"munin-node not installed"
	};
	Check::pass(NAME, summary).with_payload_extra("munin", detected)
}

/// True when munin-node's systemd unit is installed, or its binary is on `PATH`.
#[cfg(target_os = "linux")]
async fn detect() -> bool {
	if systemd::unit_file_exists(UNIT).await.unwrap_or(false) {
		return true;
	}
	binary_on_path("munin-node")
}

/// munin is a Linux monitoring stack; treat other platforms as never having it.
#[cfg(not(target_os = "linux"))]
async fn detect() -> bool {
	false
}

#[cfg(target_os = "linux")]
fn binary_on_path(name: &str) -> bool {
	let Some(path) = std::env::var_os("PATH") else {
		return false;
	};
	std::env::split_paths(&path).any(|dir| dir.join(name).is_file())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn reports_the_munin_bool_fact() {
		for detected in [true, false] {
			let check = build(detected);
			assert_eq!(check.name, "munin");
			assert_eq!(
				check.payload_extras.get("munin").and_then(|v| v.as_bool()),
				Some(detected)
			);
		}
	}
}
