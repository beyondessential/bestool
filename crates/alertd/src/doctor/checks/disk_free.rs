use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use sysinfo::Disks;

use super::SweepContext;
use crate::doctor::check::Check;

const WARN_PCT_USED: f64 = 80.0;
const FAIL_PCT_USED: f64 = 95.0;

pub async fn run(ctx: SweepContext) -> Check {
	let disks = Disks::new_with_refreshed_list();

	let tamanu_mount = ctx
		.tamanu
		.as_ref()
		.and_then(|t| best_mount_for(&disks, &t.tamanu_root));
	let root_mount = if cfg!(windows) {
		best_mount_for(&disks, &PathBuf::from(r"C:\"))
	} else {
		best_mount_for(&disks, &PathBuf::from("/"))
	};

	let mut considered: Vec<&sysinfo::Disk> = Vec::new();
	if let Some(d) = root_mount {
		considered.push(d);
	}
	if let Some(d) = tamanu_mount
		&& !considered
			.iter()
			.any(|x| x.mount_point() == d.mount_point())
	{
		considered.push(d);
	}

	if considered.is_empty() {
		return Check::warning(
			"disk_free",
			"no matching mount found",
			"sysinfo returned no disks for /, C:, or the Tamanu root",
		);
	}

	let mut worst_pct: f64 = 0.0;
	let mut worst_summary = String::new();
	let mut mounts: Vec<Value> = Vec::new();

	for disk in considered {
		let total = disk.total_space();
		let free = disk.available_space();
		let used = total.saturating_sub(free);
		let pct = if total > 0 {
			((used as f64 / total as f64) * 100.0).round()
		} else {
			0.0
		};
		if pct > worst_pct {
			worst_pct = pct;
			worst_summary = format!(
				"{} {:.0}% used ({} of {} free)",
				disk.mount_point().display(),
				pct,
				human_bytes(free),
				human_bytes(total)
			);
		}
		mounts.push(json!({
			"mountpoint": disk.mount_point().to_string_lossy(),
			"free_bytes": free,
			"total_bytes": total,
			"percent_used": pct,
		}));
	}

	let mut details = Map::new();
	details.insert("mounts".into(), Value::Array(mounts));

	let check = if worst_pct >= FAIL_PCT_USED {
		Check::fail(
			"disk_free",
			worst_summary.clone(),
			format!("at or above {FAIL_PCT_USED}% used"),
		)
	} else if worst_pct >= WARN_PCT_USED {
		Check::warning(
			"disk_free",
			worst_summary.clone(),
			format!("at or above {WARN_PCT_USED}% used"),
		)
	} else {
		Check::pass("disk_free", worst_summary)
	};
	check.with_details(details)
}

fn best_mount_for<'a>(disks: &'a Disks, path: &Path) -> Option<&'a sysinfo::Disk> {
	disks
		.iter()
		.filter(|d| path.starts_with(d.mount_point()))
		.max_by_key(|d| d.mount_point().as_os_str().len())
}

fn human_bytes(b: u64) -> String {
	const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];
	let mut value = b as f64;
	let mut unit = 0;
	while value >= 1024.0 && unit < UNITS.len() - 1 {
		value /= 1024.0;
		unit += 1;
	}
	format!("{value:.1}{}", UNITS[unit])
}
