use sysinfo::System;

use super::CheckContext;
use crate::doctor::check::Check;

pub async fn run(_ctx: CheckContext) -> Check {
	let load = System::load_average();
	if cfg!(target_os = "windows") {
		return Check::pass("load", "load average not available on Windows")
			.with_detail("skipped", true);
	}

	let summary = format!(
		"load average: {:.2}, {:.2}, {:.2}",
		load.one, load.five, load.fifteen
	);
	Check::pass("load", summary)
		.with_detail("one_min", load.one)
		.with_detail("five_min", load.five)
		.with_detail("fifteen_min", load.fifteen)
}
