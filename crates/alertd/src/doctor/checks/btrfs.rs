//! btrfs filesystem health.
//!
//! Auto-discovers mounted btrfs filesystems and flags the conditions that
//! precede a btrfs outage but stay invisible to plain `df`:
//!
//! - **Unallocated space low** — btrfs carves raw device space into data and
//!   metadata chunks on demand. Once unallocated space runs out it can't make
//!   new metadata chunks and hits ENOSPC even while `df` still shows free
//!   space.
//! - **Metadata allocation near full** — the used fraction of already-allocated
//!   metadata chunks; combined with low unallocated space this is the ENOSPC
//!   trap. (Data exhaustion shows up via the `disk_free` check instead.)
//! - **Snapshot / subvolume count** — a large backlog slows commits and balance
//!   and eats metadata.
//! - **Device error counters** — non-zero `btrfs device stats` counters, an
//!   early sign of failing storage.
//!
//! Linux-only; skips elsewhere, when no btrfs filesystem is mounted, or when
//! the `btrfs` tool isn't installed. Per-filesystem command failures degrade to
//! a warning for that signal rather than failing the whole check. Thresholds
//! are deliberately conservative starting points (see the consts below).

use std::{collections::BTreeSet, path::PathBuf};

use serde_json::{Value, json};

use super::SweepContext;
use crate::doctor::Stat;
use crate::doctor::check::Check;

const NAME: &str = "btrfs";

/// Unallocated-space thresholds. btrfs needs ~1GiB unallocated to carve a new
/// data chunk and ~256MiB for metadata; dropping near that risks ENOSPC.
const UNALLOC_FAIL: u64 = 1 << 30; // 1 GiB
const UNALLOC_WARN: u64 = 3 * (1 << 30); // 3 GiB

/// Used fraction of allocated metadata chunks.
const METADATA_WARN_PCT: f64 = 90.0;
const METADATA_FAIL_PCT: f64 = 95.0;

/// Subvolume/snapshot backlog. Tune per deployment if heavy snapshotting is
/// expected; these are starting points.
const SUBVOL_WARN: usize = 100;
const SUBVOL_FAIL: usize = 300;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Sev {
	Warn,
	Fail,
}

pub async fn run(_ctx: SweepContext) -> Check {
	if !cfg!(target_os = "linux") {
		return Check::skip(
			NAME,
			"not supported on this platform",
			"btrfs checks are Linux-only",
		);
	}

	let mounts = btrfs_mounts();
	if mounts.is_empty() {
		return Check::skip(
			NAME,
			"no btrfs filesystems mounted",
			"nothing to check on this host",
		);
	}

	let mut findings: Vec<(Sev, String)> = Vec::new();
	let mut details: Vec<Value> = Vec::new();
	let mut stats: Vec<Stat> = Vec::new();

	for mount in &mounts {
		let label = mount.display().to_string();
		match inspect(mount).await {
			Ok(report) => {
				findings.extend(report.findings);
				details.push(report.detail);
				stats.extend(report.stats);
			}
			Err(CmdErr::NotInstalled) => {
				return Check::skip(
					NAME,
					"btrfs tool not installed",
					"`btrfs` not found on PATH",
				);
			}
			Err(CmdErr::Failed(msg)) => {
				findings.push((Sev::Warn, format!("{label}: {msg}")));
				details.push(json!({ "mountpoint": label, "error": msg }));
			}
		}
	}

	let worst = findings.iter().map(|(s, _)| *s).max();
	let reasons = findings
		.iter()
		.map(|(_, m)| m.as_str())
		.collect::<Vec<_>>()
		.join("; ");

	let count = mounts.len();
	let check = match worst {
		Some(Sev::Fail) => Check::fail(
			NAME,
			format!("{count} btrfs filesystem(s) checked"),
			reasons,
		),
		Some(Sev::Warn) => Check::warning(
			NAME,
			format!("{count} btrfs filesystem(s) checked"),
			reasons,
		),
		None => Check::pass(NAME, format!("{count} btrfs filesystem(s) healthy")),
	};
	check
		.with_detail("filesystems", Value::Array(details))
		.with_stats(stats)
}

/// Distinct btrfs filesystems, one mountpoint each (deduplicated by source
/// device so multiple subvolume mounts of one filesystem aren't inspected
/// repeatedly).
fn btrfs_mounts() -> Vec<PathBuf> {
	let disks = sysinfo::Disks::new_with_refreshed_list();
	let mut seen = BTreeSet::new();
	let mut out = Vec::new();
	for disk in disks.list() {
		if !disk
			.file_system()
			.to_string_lossy()
			.eq_ignore_ascii_case("btrfs")
		{
			continue;
		}
		let device = disk.name().to_string_lossy().to_string();
		if seen.insert(device) {
			out.push(disk.mount_point().to_path_buf());
		}
	}
	out
}

enum CmdErr {
	NotInstalled,
	Failed(String),
}

async fn btrfs_cmd(args: &[&str]) -> Result<String, CmdErr> {
	// device stats / subvolume list need CAP_SYS_ADMIN; elevate when not root so
	// an interactive run still collects, matching the root daemon sweep.
	match super::privileged("btrfs").args(args).output().await {
		Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
		Ok(o) => Err(CmdErr::Failed(format!(
			"`btrfs {}` exited {}: {}",
			args.join(" "),
			o.status,
			String::from_utf8_lossy(&o.stderr).trim()
		))),
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(CmdErr::NotInstalled),
		Err(e) => Err(CmdErr::Failed(format!(
			"could not run `btrfs {}`: {e}",
			args.join(" ")
		))),
	}
}

struct FsReport {
	findings: Vec<(Sev, String)>,
	detail: Value,
	stats: Vec<Stat>,
}

async fn inspect(mount: &std::path::Path) -> Result<FsReport, CmdErr> {
	let label = mount.display().to_string();
	let mount_str = mount.to_string_lossy().into_owned();

	let usage = parse_usage(&btrfs_cmd(&["filesystem", "usage", "--raw", &mount_str]).await?);
	let errors = parse_device_errors(&btrfs_cmd(&["device", "stats", &mount_str]).await?);
	let subvols = count_subvolumes(&btrfs_cmd(&["subvolume", "list", &mount_str]).await?);

	let mut findings = Vec::new();

	if usage.device_unallocated < UNALLOC_FAIL {
		findings.push((
			Sev::Fail,
			format!(
				"{label}: only {} unallocated (btrfs can't allocate new chunks)",
				gib(usage.device_unallocated)
			),
		));
	} else if usage.device_unallocated < UNALLOC_WARN {
		findings.push((
			Sev::Warn,
			format!("{label}: {} unallocated", gib(usage.device_unallocated)),
		));
	}

	let meta_pct = pct(usage.metadata_used, usage.metadata_size);
	if meta_pct >= METADATA_FAIL_PCT {
		findings.push((
			Sev::Fail,
			format!("{label}: metadata {meta_pct:.0}% of allocated chunks used"),
		));
	} else if meta_pct >= METADATA_WARN_PCT {
		findings.push((
			Sev::Warn,
			format!("{label}: metadata {meta_pct:.0}% of allocated chunks used"),
		));
	}

	if subvols > SUBVOL_FAIL {
		findings.push((
			Sev::Fail,
			format!("{label}: {subvols} subvolumes/snapshots"),
		));
	} else if subvols > SUBVOL_WARN {
		findings.push((
			Sev::Warn,
			format!("{label}: {subvols} subvolumes/snapshots"),
		));
	}

	if !errors.is_empty() {
		let listed = errors
			.iter()
			.map(|(k, v)| format!("{k}={v}"))
			.collect::<Vec<_>>()
			.join(", ");
		findings.push((Sev::Warn, format!("{label}: device errors ({listed})")));
	}

	let device_errors_total: u64 = errors.iter().map(|(_, v)| *v).sum();
	let stats = vec![
		Stat::gauge("device_unallocated_bytes", usage.device_unallocated as f64)
			.label("mount", label.clone())
			.help("Unallocated btrfs space"),
		Stat::gauge("metadata_percent", meta_pct)
			.label("mount", label.clone())
			.help("btrfs metadata chunk usage, percent"),
		Stat::gauge("subvolumes", subvols as f64)
			.label("mount", label.clone())
			.help("btrfs subvolumes/snapshots"),
		Stat::gauge("device_errors", device_errors_total as f64)
			.label("mount", label.clone())
			.help("btrfs device error counters (sum)"),
	];

	let detail = json!({
		"mountpoint": label,
		"device_size": usage.device_size,
		"device_unallocated": usage.device_unallocated,
		"metadata_size": usage.metadata_size,
		"metadata_used": usage.metadata_used,
		"metadata_pct": meta_pct,
		"subvolumes": subvols,
		"device_errors": errors
			.iter()
			.map(|(k, v)| json!({ "counter": k, "value": v }))
			.collect::<Vec<_>>(),
	});

	Ok(FsReport {
		findings,
		detail,
		stats,
	})
}

fn pct(used: u64, size: u64) -> f64 {
	if size == 0 {
		0.0
	} else {
		used as f64 / size as f64 * 100.0
	}
}

fn gib(bytes: u64) -> String {
	format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
}

#[derive(Default, Debug, PartialEq, Eq)]
struct Usage {
	device_size: u64,
	device_unallocated: u64,
	metadata_size: u64,
	metadata_used: u64,
}

/// Parse the relevant figures out of `btrfs filesystem usage --raw`. With
/// `--raw`, every value is a plain byte count, so we just pull the labelled
/// numbers; the overall block is `Label: <bytes>` and the block-group lines are
/// `Metadata,<profile>: Size:<bytes>, Used:<bytes>`.
fn parse_usage(out: &str) -> Usage {
	let mut u = Usage::default();
	for line in out.lines() {
		let line = line.trim();
		if let Some(rest) = line.strip_prefix("Device size:") {
			u.device_size = first_u64(rest);
		} else if let Some(rest) = line.strip_prefix("Device unallocated:") {
			u.device_unallocated = first_u64(rest);
		} else if line.starts_with("Metadata,") {
			u.metadata_size = field_u64(line, "Size:");
			u.metadata_used = field_u64(line, "Used:");
		}
	}
	u
}

/// First whitespace-separated integer in `s` (trailing comma tolerated).
fn first_u64(s: &str) -> u64 {
	s.split_whitespace()
		.next()
		.map(|t| t.trim_end_matches(','))
		.and_then(|t| t.parse().ok())
		.unwrap_or(0)
}

/// Integer immediately following `key` in `line` (e.g. `Size:` → the bytes).
fn field_u64(line: &str, key: &str) -> u64 {
	line.find(key)
		.map(|i| &line[i + key.len()..])
		.and_then(|rest| {
			rest.trim_start()
				.split(|c: char| c == ',' || c.is_whitespace())
				.next()
				.filter(|t| !t.is_empty())
				.and_then(|t| t.parse().ok())
		})
		.unwrap_or(0)
}

/// Non-zero counters from `btrfs device stats`, lines like
/// `[/dev/sdb].write_io_errs    0`.
fn parse_device_errors(out: &str) -> Vec<(String, u64)> {
	let mut found = Vec::new();
	for line in out.lines() {
		let mut parts = line.split_whitespace();
		if let (Some(name), Some(value)) = (parts.next(), parts.next())
			&& let Ok(v) = value.parse::<u64>()
			&& v > 0
		{
			found.push((name.to_string(), v));
		}
	}
	found
}

/// Number of subvolumes/snapshots in `btrfs subvolume list` (one per line).
fn count_subvolumes(out: &str) -> usize {
	out.lines().filter(|l| !l.trim().is_empty()).count()
}

#[cfg(test)]
mod tests {
	use super::*;

	const USAGE: &str = "Overall:\n    Device size:\t\t\t  500107862016\n    Device allocated:\t\t\t  53687091200\n    Device unallocated:\t\t\t  446420770816\n    Device missing:\t\t\t             0\n    Used:\t\t\t\t  50000000000\n    Free (estimated):\t\t\t  448000000000\n\nData,single: Size:50000000000, Used:49000000000\nMetadata,single: Size:3221225472, Used:2000000000\nSystem,single: Size:33554432, Used:16384\n";

	#[test]
	fn parses_usage_raw() {
		let u = parse_usage(USAGE);
		assert_eq!(u.device_size, 500107862016);
		assert_eq!(u.device_unallocated, 446420770816);
		assert_eq!(u.metadata_size, 3221225472);
		assert_eq!(u.metadata_used, 2000000000);
	}

	#[test]
	fn metadata_pct_from_usage() {
		let u = parse_usage(USAGE);
		// 2.0e9 / 3.22e9 ≈ 62%
		assert!((pct(u.metadata_used, u.metadata_size) - 62.0).abs() < 2.0);
	}

	#[test]
	fn device_errors_only_nonzero() {
		let out = "[/dev/sdb].write_io_errs    0\n[/dev/sdb].read_io_errs     0\n[/dev/sdb].flush_io_errs    0\n[/dev/sdb].corruption_errs  4\n[/dev/sdb].generation_errs  0\n";
		let errs = parse_device_errors(out);
		assert_eq!(errs, vec![("[/dev/sdb].corruption_errs".to_string(), 4)]);
	}

	#[test]
	fn device_errors_all_clean() {
		let out = "[/dev/sdb].write_io_errs    0\n[/dev/sdb].corruption_errs  0\n";
		assert!(parse_device_errors(out).is_empty());
	}

	#[test]
	fn counts_subvolumes() {
		let out =
			"ID 256 gen 9 top level 5 path home\nID 257 gen 9 top level 5 path snapshots/a\n\n";
		assert_eq!(count_subvolumes(out), 2);
	}

	#[test]
	fn pct_zero_size_is_zero() {
		assert_eq!(pct(100, 0), 0.0);
	}
}
