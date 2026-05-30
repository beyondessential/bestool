use sysinfo::System;

use super::CheckContext;
use crate::doctor::check::Check;

pub async fn run(_ctx: CheckContext) -> Check {
	let secs = System::uptime();
	Check::pass("uptime", humanise(secs)).with_detail("uptime_secs", secs)
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
