use std::path::{Path, PathBuf};

use sysinfo::Disks;

use super::MachineCx;
use crate::Stat;
use crate::check::{Check, Instance};

const WARN_PCT_USED: f64 = 80.0;
const FAIL_PCT_USED: f64 = 95.0;

pub async fn run(ctx: MachineCx) -> Check {
	let disks = Disks::new_with_refreshed_list();

	// The mount Tamanu's files sit on, alongside the machine's root. A
	// deployment known only through its database has no files here, so there is
	// no second mount to consider.
	let tamanu_mount = ctx
		.tamanu
		.as_ref()
		.and_then(|t| t.root.as_ref())
		.and_then(|root| best_mount_for(&disks, root));
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

	let mounts: Vec<Mount> = considered
		.into_iter()
		.map(|disk| Mount {
			path: disk.mount_point().to_string_lossy().into_owned(),
			total: disk.total_space(),
			free: disk.available_space(),
		})
		.collect();
	grade(&mounts)
}

struct Mount {
	path: String,
	total: u64,
	free: u64,
}

/// One instance per mount considered, keyed by its path and graded on its own
/// use. The headline names the fullest.
fn grade(mounts: &[Mount]) -> Check {
	let mut worst_pct: f64 = 0.0;
	let mut worst_summary = String::new();
	let mut instances: Vec<Instance> = Vec::new();
	let mut stats: Vec<Stat> = Vec::new();

	for Mount {
		path: mount,
		total,
		free,
	} in mounts
	{
		let (total, free) = (*total, *free);
		let used = total.saturating_sub(free);
		let pct = if total > 0 {
			((used as f64 / total as f64) * 100.0).round()
		} else {
			0.0
		};
		let described = format!(
			"{pct:.0}% used ({} of {} free)",
			human_bytes(free),
			human_bytes(total)
		);
		if pct > worst_pct {
			worst_pct = pct;
			worst_summary = format!("{mount} {described}");
		}
		stats.push(
			Stat::gauge("free_bytes", free as f64)
				.group("bytes")
				.label("mount", mount.clone())
				.help("Free disk space"),
		);
		stats.push(
			Stat::gauge("total_bytes", total as f64)
				.group("bytes")
				.label("mount", mount.clone())
				.help("Total disk space"),
		);
		stats.push(
			Stat::gauge("percent_used", pct)
				.label("mount", mount.clone())
				.help("Disk used, percent"),
		);
		let instance = if pct >= FAIL_PCT_USED {
			Instance::fail(mount, format!("at or above {FAIL_PCT_USED}% used"))
		} else if pct >= WARN_PCT_USED {
			Instance::warning(mount, format!("at or above {WARN_PCT_USED}% used"))
		} else {
			Instance::pass(mount)
		};
		instances.push(
			instance
				.with_detail("mountpoint", mount.as_str())
				.with_detail("free_bytes", free)
				.with_detail("total_bytes", total)
				.with_detail("percent_used", pct),
		);
	}

	Check::instanced("disk_free", worst_summary, instances).with_stats(stats)
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

#[cfg(test)]
mod tests {
	use super::*;
	use crate::check::CheckStatus;

	const GB: u64 = 1 << 30;

	fn mount(path: &str, used_pct: u64) -> Mount {
		Mount {
			path: path.into(),
			total: 100 * GB,
			free: (100 - used_pct) * GB,
		}
	}

	#[test]
	fn each_mount_is_graded_as_its_own_instance() {
		let check = grade(&[
			mount("/", 50),
			mount("/var/lib/tamanu", 82),
			mount("/srv", 96),
		]);
		let instances = check.instances.as_ref().expect("instanced");

		let status = |key: &str| &instances.iter().find(|i| i.key == key).unwrap().status;
		assert!(matches!(status("/"), CheckStatus::Pass));
		assert!(matches!(status("/var/lib/tamanu"), CheckStatus::Warning(_)));
		assert!(matches!(status("/srv"), CheckStatus::Fail(_)));

		assert!(matches!(check.status, CheckStatus::Fail(_)));
		assert!(
			check.summary.starts_with("/srv 96% used"),
			"{}",
			check.summary
		);

		let tamanu = instances
			.iter()
			.find(|i| i.key == "/var/lib/tamanu")
			.unwrap();
		assert_eq!(tamanu.detail["percent_used"], 82.0);
		assert_eq!(tamanu.detail["total_bytes"], 100 * GB);
	}

	#[test]
	fn mounts_under_the_warning_line_pass() {
		let check = grade(&[mount("/", 10), mount("/data", 79)]);
		assert!(matches!(check.status, CheckStatus::Pass));
		assert_eq!(check.instances.unwrap().len(), 2);
	}
}
