use sysinfo::{CpuRefreshKind, RefreshKind, System};

use super::SweepContext;
use crate::doctor::check::{Check, CheckStatus};

/// Multiplier on the logical core count above which the 5-minute load average
/// is treated as a hard failure.
const FAIL_PER_CORE: f64 = 4.0;
/// Multiplier on the logical core count above which the 5-minute load average
/// is treated as a warning.
const WARN_PER_CORE: f64 = 1.5;

pub async fn run(_ctx: SweepContext) -> Check {
	if cfg!(target_os = "windows") {
		return Check::skip(
			"load",
			"not available on Windows",
			"sysinfo does not report load average on Windows",
		);
	}

	let sys =
		System::new_with_specifics(RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing()));
	let cores = sys.cpus().len().max(1);

	let load = System::load_average();
	let summary = format!(
		"load average: {:.2}, {:.2}, {:.2} ({cores} cores)",
		load.one, load.five, load.fifteen
	);

	let check = match tier(load.five, cores) {
		CheckStatus::Fail(_) => Check::fail(
			"load",
			summary,
			format!(
				"5-min load {:.2} over {:.1}x cores ({cores})",
				load.five, FAIL_PER_CORE
			),
		),
		CheckStatus::Warning(_) => Check::warning(
			"load",
			summary,
			format!(
				"5-min load {:.2} over {:.1}x cores ({cores})",
				load.five, WARN_PER_CORE
			),
		),
		_ => Check::pass("load", summary),
	};

	check
		.with_detail("one_min", load.one)
		.with_detail("five_min", load.five)
		.with_detail("fifteen_min", load.fifteen)
		.with_detail("cores", cores)
}

/// Tier the 5-minute load average against the logical core count.
fn tier(five: f64, cores: usize) -> CheckStatus {
	let cores = cores as f64;
	if five > FAIL_PER_CORE * cores {
		CheckStatus::Fail(String::new())
	} else if five > WARN_PER_CORE * cores {
		CheckStatus::Warning(String::new())
	} else {
		CheckStatus::Pass
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn tier_boundaries() {
		assert!(matches!(tier(5.9, 4), CheckStatus::Pass));
		assert!(matches!(tier(6.1, 4), CheckStatus::Warning(_)));
		assert!(matches!(tier(15.9, 4), CheckStatus::Warning(_)));
		assert!(matches!(tier(16.1, 4), CheckStatus::Fail(_)));
	}

	#[test]
	fn tier_single_core() {
		assert!(matches!(tier(1.4, 1), CheckStatus::Pass));
		assert!(matches!(tier(1.6, 1), CheckStatus::Warning(_)));
		assert!(matches!(tier(4.1, 1), CheckStatus::Fail(_)));
	}
}
