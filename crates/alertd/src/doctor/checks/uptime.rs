use sysinfo::System;

use super::SweepContext;
use crate::doctor::Stat;
use crate::doctor::check::Check;

/// Below this uptime the host has rebooted recently, which may be unexpected.
const WARN_UPTIME_SECS: u64 = 10 * 60;

pub async fn run(_ctx: SweepContext) -> Check {
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
	check
		.with_detail("uptime_secs", secs)
		.with_stat(Stat::gauge("uptime_seconds", secs as f64).help("Host uptime"))
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

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn emits_uptime_stat() {
		let ctx = SweepContext {
			tamanu: None,
			http_client: reqwest::Client::new(),
		};
		let check = run(ctx).await;
		assert!(check.stats.iter().any(|s| s.name == "uptime_seconds"));
	}
}
