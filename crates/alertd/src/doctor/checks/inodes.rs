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

use serde_json::{Value, json};
use tokio::process::Command;

use super::SweepContext;
use crate::doctor::check::Check;

const NAME: &str = "inodes";

const WARN_PCT: f64 = 85.0;
const FAIL_PCT: f64 = 95.0;

pub async fn run(_ctx: SweepContext) -> Check {
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

	let mut worst_pct = 0.0_f64;
	let mut worst: Option<String> = None;
	let mut details = Vec::new();
	for fs in &filesystems {
		let pct = fs.pct_used();
		if pct > worst_pct {
			worst_pct = pct;
			worst = Some(format!(
				"{} {:.0}% inodes used ({} of {} free) on {}",
				fs.mount,
				pct,
				fs.total - fs.used,
				fs.total,
				fs.fstype,
			));
		}
		details.push(json!({
			"mountpoint": fs.mount,
			"fstype": fs.fstype,
			"inodes_total": fs.total,
			"inodes_used": fs.used,
			"percent_used": pct.round(),
		}));
	}

	let summary = worst.unwrap_or_else(|| format!("{} filesystem(s) OK", filesystems.len()));
	let check = if worst_pct >= FAIL_PCT {
		Check::fail(NAME, summary.clone(), summary)
	} else if worst_pct >= WARN_PCT {
		Check::warning(NAME, summary.clone(), summary)
	} else {
		Check::pass(NAME, summary)
	};
	check.with_detail("filesystems", Value::Array(details))
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
