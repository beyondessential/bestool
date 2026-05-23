use std::process::Command;

use super::CheckContext;
use crate::doctor::check::Check;

pub async fn run(_ctx: CheckContext) -> Check {
	if !cfg!(target_os = "linux") {
		return Check::pass("time_sync", "time sync check skipped (non-Linux)")
			.with_detail("skipped", true);
	}

	let output = match Command::new("timedatectl")
		.args(["show", "-p", "NTPSynchronized", "--value"])
		.output()
	{
		Ok(o) if o.status.success() => o,
		Ok(_) | Err(_) => {
			return Check::warning(
				"time_sync",
				"timedatectl unavailable",
				"could not run timedatectl",
			)
			.with_detail("synchronized", serde_json::Value::Null);
		}
	};
	let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
	let synced = stdout == "yes";

	let check = if synced {
		Check::pass("time_sync", "NTP synchronised")
	} else {
		Check::warning(
			"time_sync",
			"NTP not synchronised",
			"timedatectl reports no",
		)
	};
	check
		.with_detail("synchronized", synced)
		.with_detail("service", "timedatectl")
}
