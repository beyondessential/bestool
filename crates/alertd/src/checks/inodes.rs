//! Inode exhaustion.
//!
//! Fixed-inode filesystems (ext4, xfs, vfat, tmpfs, …) carve a set number of
//! inodes at mkfs time; a workload that creates lots of small files can run out
//! of inodes while `df` still shows free bytes, and writes then fail with
//! ENOSPC. btrfs allocates inodes dynamically and reports no meaningful inode
//! count, so it's excluded here (its space pressure is covered by the `btrfs`
//! check); any filesystem `df` reports with a zero inode total is skipped for
//! the same reason.
//!
//! Linux-only: reads `df -P -i -T`. Skips elsewhere or when `df` is unavailable.

use tokio::process::Command;

use super::MachineCx;
use crate::Stat;
use crate::check::{Check, Instance};

const NAME: &str = "inodes";

const WARN_PCT: f64 = 85.0;
const FAIL_PCT: f64 = 95.0;

pub async fn run(_ctx: MachineCx) -> Check {
	if !cfg!(target_os = "linux") {
		return Check::skip(
			NAME,
			"not supported on this platform",
			"inode accounting is read from Linux `df`",
		);
	}

	let output = match Command::new("df").args(["-P", "-i", "-T"]).output().await {
		Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
		Ok(o) => {
			// df ran but failed, so we couldn't read inode usage at all — the
			// check couldn't run. That's broken, not a skip (which is for df not
			// being present at all, handled below).
			return Check::broken(
				NAME,
				"df failed",
				format!(
					"`df -PiT` exited {}: {}",
					o.status,
					String::from_utf8_lossy(&o.stderr).trim()
				),
			);
		}
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
			return Check::skip(NAME, "df not found", "`df` not on PATH");
		}
		Err(e) => return Check::skip(NAME, "df unavailable", format!("could not run df: {e}")),
	};

	let filesystems = parse_df(&output);
	if filesystems.is_empty() {
		return Check::skip(
			NAME,
			"no inode-counted filesystems",
			"every mounted filesystem reports no fixed inode count (e.g. btrfs)",
		);
	}

	grade(&filesystems)
}

/// One instance per filesystem, keyed by its mount point, each graded on its own
/// inode use. The headline names the fullest.
fn grade(filesystems: &[FsInodes]) -> Check {
	let mut worst_pct = 0.0_f64;
	let mut worst: Option<String> = None;
	let mut instances = Vec::new();
	let mut stats = Vec::new();
	for fs in filesystems {
		let pct = fs.pct_used();
		let described = format!(
			"{pct:.0}% inodes used ({} of {} free) on {}",
			fs.total - fs.used,
			fs.total,
			fs.fstype,
		);
		if pct > worst_pct {
			worst_pct = pct;
			worst = Some(format!("{} {described}", fs.mount));
		}
		stats.push(
			Stat::gauge("percent_used", pct.round())
				.label("mount", fs.mount.clone())
				.help("Inodes used, percent"),
		);
		stats.push(
			Stat::gauge("inodes_used", fs.used as f64)
				.label("mount", fs.mount.clone())
				.help("Inodes in use"),
		);
		let instance = if pct >= FAIL_PCT {
			Instance::fail(&fs.mount, described)
		} else if pct >= WARN_PCT {
			Instance::warning(&fs.mount, described)
		} else {
			Instance::pass(&fs.mount)
		};
		instances.push(
			instance
				.with_detail("mountpoint", fs.mount.as_str())
				.with_detail("fstype", fs.fstype.as_str())
				.with_detail("inodes_total", fs.total)
				.with_detail("inodes_used", fs.used)
				.with_detail("percent_used", pct.round()),
		);
	}

	let summary = worst.unwrap_or_else(|| format!("{} filesystem(s) OK", filesystems.len()));
	Check::instanced(NAME, summary, instances).with_stats(stats)
}

struct FsInodes {
	mount: String,
	fstype: String,
	total: u64,
	used: u64,
}

impl FsInodes {
	fn pct_used(&self) -> f64 {
		if self.total == 0 {
			0.0
		} else {
			self.used as f64 / self.total as f64 * 100.0
		}
	}
}

/// Parse `df -P -i -T`. Columns are
/// `Filesystem Type Inodes IUsed IFree IUse% Mounted on`; the mountpoint is
/// last and may contain spaces, so it's rejoined from the remaining fields.
/// btrfs and any filesystem reporting a zero inode total are dropped.
fn parse_df(output: &str) -> Vec<FsInodes> {
	let mut out = Vec::new();
	for line in output.lines().skip(1) {
		let fields: Vec<&str> = line.split_whitespace().collect();
		if fields.len() < 7 {
			continue;
		}
		let fstype = fields[1];
		let (Ok(total), Ok(used)) = (fields[2].parse::<u64>(), fields[3].parse::<u64>()) else {
			continue;
		};
		if fstype.eq_ignore_ascii_case("btrfs") || total == 0 {
			continue;
		}
		out.push(FsInodes {
			mount: fields[6..].join(" "),
			fstype: fstype.to_string(),
			total,
			used,
		});
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::check::CheckStatus;

	const DF: &str = "Filesystem     Type     Inodes   IUsed     IFree IUse% Mounted on\n/dev/sda1      ext4    6553600  250000   6303600    4% /\ntmpfs          tmpfs   2048000     120   2047880    1% /run\n/dev/sdb1      btrfs         0       0         0     - /data\n/dev/sdc1      ext4    1310720 1245000     65720   95% /var\nstore          xfs     5000000 4600000    400000   92% /srv/with space\n";

	#[test]
	fn parses_and_excludes_btrfs_and_zero() {
		let fs = parse_df(DF);
		let mounts: Vec<&str> = fs.iter().map(|f| f.mount.as_str()).collect();
		assert_eq!(mounts, vec!["/", "/run", "/var", "/srv/with space"]);
		// btrfs row (zero total) excluded.
		assert!(!mounts.contains(&"/data"));
	}

	#[test]
	fn mountpoint_with_spaces_rejoined() {
		let fs = parse_df(DF);
		let srv = fs.iter().find(|f| f.fstype == "xfs").unwrap();
		assert_eq!(srv.mount, "/srv/with space");
	}

	#[test]
	fn pct_used_computed() {
		let fs = parse_df(DF);
		let var = fs.iter().find(|f| f.mount == "/var").unwrap();
		assert!((var.pct_used() - 95.0).abs() < 0.5);
	}

	#[test]
	fn each_filesystem_is_graded_as_its_own_instance() {
		let check = grade(&parse_df(DF));
		let instances = check.instances.as_ref().expect("instanced");
		assert_eq!(instances.len(), 4);

		let status = |key: &str| &instances.iter().find(|i| i.key == key).unwrap().status;
		assert!(matches!(status("/"), CheckStatus::Pass));
		assert!(matches!(status("/run"), CheckStatus::Pass));
		// 94.99% and 92% are past the warning line and short of the failing one.
		assert!(matches!(status("/var"), CheckStatus::Warning(_)));
		assert!(matches!(status("/srv/with space"), CheckStatus::Warning(_)));

		assert!(matches!(check.status, CheckStatus::Warning(_)));
		assert!(check.summary.starts_with("/var "), "{}", check.summary);
		let var = instances.iter().find(|i| i.key == "/var").unwrap();
		assert_eq!(var.detail["fstype"], "ext4");
		assert_eq!(var.detail["inodes_used"], 1245000);
	}

	#[test]
	fn one_filesystem_past_the_failing_line_fails_the_check() {
		let mut fs = parse_df(DF);
		fs[0].used = fs[0].total * 97 / 100;
		let check = grade(&fs);
		assert!(matches!(check.status, CheckStatus::Fail(_)));
		let root = check
			.instances
			.unwrap()
			.into_iter()
			.find(|i| i.key == "/")
			.unwrap();
		assert!(root.status.is_fatal());
	}

	#[test]
	fn pct_zero_total_is_zero() {
		let fs = FsInodes {
			mount: "/x".into(),
			fstype: "ext4".into(),
			total: 0,
			used: 0,
		};
		assert_eq!(fs.pct_used(), 0.0);
	}
}
