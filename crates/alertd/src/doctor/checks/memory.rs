use sysinfo::{MemoryRefreshKind, RefreshKind, System};

use super::SweepContext;
use crate::doctor::Stat;
use crate::doctor::check::Check;

const WARN_PCT_USED: f64 = 90.0;
const FAIL_PCT_USED: f64 = 98.0;

pub async fn run(_ctx: SweepContext) -> Check {
	let sys = System::new_with_specifics(
		RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
	);
	let total = sys.total_memory();
	let used = sys.used_memory();
	let pct = if total > 0 {
		((used as f64 / total as f64) * 100.0).round()
	} else {
		0.0
	};

	let summary = format!("{pct:.0}% used");
	let check = if pct >= FAIL_PCT_USED {
		Check::fail("memory", summary.clone(), format!("≥{FAIL_PCT_USED}% used"))
	} else if pct >= WARN_PCT_USED {
		Check::warning("memory", summary.clone(), format!("≥{WARN_PCT_USED}% used"))
	} else {
		Check::pass("memory", summary)
	};

	check
		.with_detail("used_bytes", used)
		.with_detail("total_bytes", total)
		.with_detail("percent_used", pct)
		.with_stat(Stat::gauge("used_bytes", used as f64).help("Memory in use"))
		.with_stat(Stat::gauge("total_bytes", total as f64).help("Total memory"))
		.with_stat(Stat::gauge("percent_used", pct).help("Memory used, percent"))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn emits_memory_stats() {
		let ctx = SweepContext {
			tamanu: None,
			http_client: reqwest::Client::new(),
		};
		let check = run(ctx).await;
		let names: Vec<&str> = check.stats.iter().map(|s| s.name).collect();
		assert!(names.contains(&"used_bytes"));
		assert!(names.contains(&"total_bytes"));
		assert!(names.contains(&"percent_used"));
	}
}
