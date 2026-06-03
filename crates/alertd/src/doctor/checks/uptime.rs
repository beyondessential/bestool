use sysinfo::System;

use super::CheckContext;
use crate::doctor::check::Check;

/// Below this uptime the host has rebooted recently, which may be unexpected.
const WARN_UPTIME_SECS: u64 = 10 * 60;

pub async fn run(_ctx: CheckContext) -> Check {
	let secs = System::uptime();
	let summary = humanise(secs);
	let check = if secs < WARN_UPTIME_SECS {
		Check::warning(
			"uptime",
			summary,
			"host rebooted within the last 10 minutes",
		)
	} else {
		Check::pass("uptime", summary)
	};
	check.with_detail("uptime_secs", secs)
}

fn humanise(secs: u64) -> String {
	let d = secs / 86400;
	let h = (secs % 86400) / 3600;
	let m = (secs % 3600) / 60;
	if d > 0 {
		format!("{d}d {h}h")
	} else if h > 0 {
		format!("{h}h {m}m")
	} else {
		format!("{m}m")
	}
}
