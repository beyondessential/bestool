//! Captures held on this device as local rollback points.
//!
//! A held capture is created deliberately and released deliberately: nothing
//! expires it, and no later backup clears it. That is what makes it trustworthy
//! across an upgrade window, and it is also why it needs watching — a hold
//! nobody drops keeps costing storage indefinitely.
//!
//! Two conditions are reported, and they are not the same problem:
//!
//! - **Held a long time** — untidy, and more expensive the longer it runs.
//! - **Capture gone** — the record names a rollback point that no longer exists.
//!   This is the serious one: the operator believes they can roll back and
//!   cannot, and nothing about the hold itself gives that away.
//!
//! Where the platform keeps shadow copies in a bounded store shared with every
//! other snapshot on the volume, that store's headroom is reported too, since
//! filling it is what silently evicts a hold. The store is host-wide
//! configuration; this check reports it and never changes it.
//!
//! The hold records are read from the on-disk layout the backup driver writes
//! (`/var/lib/bestool/held-snapshots/*.json`, or the machine-wide application
//! data directory on Windows) rather than through the driver, which lives in the
//! binary rather than this crate. Only the fields this check needs are parsed.

use std::path::PathBuf;

use jiff::{Timestamp, Unit};
use serde::Deserialize;
use serde_json::{Value, json};

use super::SweepContext;
use crate::doctor::{Stat, check::Check};

const NAME: &str = "held_captures";

/// How long a hold may sit before it is reported as forgotten. An upgrade window
/// — the reason to take one — is hours to days; past a week nobody is waiting on
/// it any more.
const STALE_AFTER_DAYS: i32 = 7;

/// The parts of a hold record this check reads.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct HoldRecord {
	id: String,
	backup_type: String,
	#[serde(default)]
	taken_at: Option<Timestamp>,
	held_at: Timestamp,
	source: PathBuf,
	#[serde(default)]
	uploaded: bool,
}

pub async fn run(_ctx: SweepContext) -> Check {
	let dir = records_dir();
	let records = read_records(&dir).await;
	if records.is_empty() {
		return Check::skip(
			NAME,
			"no captures held on this device",
			format!("no hold records in {}", dir.display()),
		);
	}

	let now = Timestamp::now();
	let mut missing: Vec<String> = Vec::new();
	let mut stale: Vec<String> = Vec::new();
	let mut details: Vec<Value> = Vec::new();

	for record in &records {
		let present = capture_readable(&record.source).await;
		let held_days = (now - record.held_at)
			.round(jiff::SpanRound::new().largest(Unit::Day))
			.map(|span| span.get_days())
			.unwrap_or(0);

		if !present {
			missing.push(format!("{} (capture gone)", record.id));
		} else if held_days >= STALE_AFTER_DAYS {
			stale.push(format!("{} (held {held_days}d)", record.id));
		}

		details.push(json!({
			"id": record.id,
			"type": record.backup_type,
			"frozenAt": record.taken_at.map(|at| at.to_string()),
			"heldAt": record.held_at.to_string(),
			"heldDays": held_days,
			"uploaded": record.uploaded,
			"source": record.source.display().to_string(),
			"capturePresent": present,
		}));
	}

	let mut stats = vec![Stat::gauge("held_captures", records.len() as f64)];
	let storage = shadow_storage().await;
	if let Some(free) = storage.as_ref().and_then(ShadowStorage::free_bytes) {
		stats.push(Stat::gauge("shadow_storage_free_bytes", free as f64));
	}

	let summary = format!("{} capture(s) held", records.len());
	let check = if !missing.is_empty() {
		Check::fail(
			NAME,
			summary,
			format!(
				"the capture behind {} is gone, so it is not a rollback point: {}",
				if missing.len() == 1 {
					"a hold"
				} else {
					"holds"
				},
				missing.join(", ")
			),
		)
	} else if !stale.is_empty() {
		Check::warning(
			NAME,
			summary,
			format!(
				"held for over {STALE_AFTER_DAYS} days and still costing storage: {}; \
				 release with `bestool canopy hold drop <id>`",
				stale.join(", ")
			),
		)
	} else {
		Check::pass(NAME, summary)
	};

	let check = check.with_detail("holds", Value::Array(details));
	match storage {
		Some(storage) => check
			.with_detail("shadowStorage", storage.detail())
			.with_stats(stats),
		None => check.with_stats(stats),
	}
}

/// Where the backup driver writes hold records.
fn records_dir() -> PathBuf {
	#[cfg(unix)]
	{
		PathBuf::from("/var/lib/bestool/held-snapshots")
	}
	#[cfg(not(unix))]
	{
		std::env::var_os("ProgramData")
			.map(PathBuf::from)
			.unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
			.join("bestool")
			.join("held-snapshots")
	}
}

async fn read_records(dir: &std::path::Path) -> Vec<HoldRecord> {
	let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
		return Vec::new();
	};
	let mut records = Vec::new();
	while let Ok(Some(entry)) = entries.next_entry().await {
		let path = entry.path();
		if path.extension().is_none_or(|ext| ext != "json") {
			continue;
		}
		if let Ok(bytes) = tokio::fs::read(&path).await
			&& let Ok(record) = serde_json::from_slice::<HoldRecord>(&bytes)
		{
			records.push(record);
		}
	}
	records.sort_by_key(|record| record.held_at);
	records
}

/// Whether the capture is still there, judged by whether its contents can be
/// listed. Deliberately not a per-backend probe: a snapshot that has been
/// deleted, unmounted, or lost with its volume all read the same way from here —
/// the rollback point cannot be read, so it is not one.
async fn capture_readable(source: &std::path::Path) -> bool {
	tokio::fs::read_dir(source)
		.await
		.map(|_| true)
		.unwrap_or(false)
}

/// The volume shadow store's size and usage, where the platform has one.
struct ShadowStorage {
	used_bytes: Option<u64>,
	max_bytes: Option<u64>,
	unbounded: bool,
}

impl ShadowStorage {
	fn free_bytes(&self) -> Option<u64> {
		match (self.max_bytes, self.used_bytes) {
			(Some(max), Some(used)) => Some(max.saturating_sub(used)),
			_ => None,
		}
	}

	fn detail(&self) -> Value {
		json!({
			"usedBytes": self.used_bytes,
			"maxBytes": self.max_bytes,
			"unbounded": self.unbounded,
			"freeBytes": self.free_bytes(),
		})
	}
}

#[cfg(windows)]
async fn shadow_storage() -> Option<ShadowStorage> {
	let output = tokio::process::Command::new("vssadmin")
		.args(["list", "shadowstorage"])
		.stdin(std::process::Stdio::null())
		.output()
		.await
		.ok()?;
	parse_shadow_storage(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(not(windows))]
async fn shadow_storage() -> Option<ShadowStorage> {
	None
}

/// Pull the used and maximum figures out of a shadow-storage listing. The
/// maximum may be unbounded, which is not the same as unknown: it means eviction
/// pressure from the store filling can't arise.
#[cfg_attr(
	all(not(windows), not(test)),
	expect(
		dead_code,
		reason = "only called on Windows; exercised by the tests below"
	)
)]
fn parse_shadow_storage(output: &str) -> Option<ShadowStorage> {
	let mut used = None;
	let mut max = None;
	let mut unbounded = false;
	for line in output.lines() {
		let line = line.trim();
		if let Some(value) = line.strip_prefix("Used Shadow Copy Storage space:") {
			used = parse_size(value);
		} else if let Some(value) = line.strip_prefix("Maximum Shadow Copy Storage space:") {
			// The figure carries a trailing percentage of the volume, which has to
			// come off before the word itself can be recognised.
			if size_word(value).eq_ignore_ascii_case("UNBOUNDED") {
				unbounded = true;
			} else {
				max = parse_size(value);
			}
		}
	}
	(used.is_some() || max.is_some() || unbounded).then_some(ShadowStorage {
		used_bytes: used,
		max_bytes: max,
		unbounded,
	})
}

/// The figure without its trailing percentage-of-volume: `12.5 GB (10%)` → `12.5
/// GB`, `UNBOUNDED (100%)` → `UNBOUNDED`.
fn size_word(value: &str) -> &str {
	value.split('(').next().unwrap_or(value).trim()
}

/// `12.5 GB` → bytes.
fn parse_size(value: &str) -> Option<u64> {
	let mut parts = size_word(value).split_whitespace();
	let number: f64 = parts.next()?.replace(',', "").parse().ok()?;
	let scale: f64 = match parts.next()?.to_ascii_uppercase().as_str() {
		"B" | "BYTES" => 1.0,
		"KB" => 1024.0,
		"MB" => 1024f64.powi(2),
		"GB" => 1024f64.powi(3),
		"TB" => 1024f64.powi(4),
		_ => return None,
	};
	Some((number * scale) as u64)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn shadow_storage_figures_are_read_from_a_listing() {
		let output = "\
Shadow Copy Storage association
   For volume: (C:)\\\\?\\Volume{abc}\\
   Shadow Copy Storage volume: (C:)\\\\?\\Volume{abc}\\
   Used Shadow Copy Storage space: 12.0 GB (1%)
   Allocated Shadow Copy Storage space: 13.0 GB (1%)
   Maximum Shadow Copy Storage space: 100 GB (10%)
";
		let storage = parse_shadow_storage(output).unwrap();
		assert_eq!(storage.used_bytes, Some(12 * 1024 * 1024 * 1024));
		assert_eq!(storage.max_bytes, Some(100 * 1024 * 1024 * 1024));
		assert!(!storage.unbounded);
		assert_eq!(storage.free_bytes(), Some(88 * 1024 * 1024 * 1024));
	}

	/// An unbounded store is the configuration that makes eviction a non-issue,
	/// so it must read as unbounded rather than as a missing figure.
	#[test]
	fn an_unbounded_maximum_is_not_an_unknown_one() {
		let output = "   Used Shadow Copy Storage space: 1.0 GB (1%)\n   \
		               Maximum Shadow Copy Storage space: UNBOUNDED (100%)\n";
		let storage = parse_shadow_storage(output).unwrap();
		assert!(storage.unbounded);
		assert_eq!(storage.max_bytes, None);
		assert_eq!(storage.free_bytes(), None);
	}

	#[test]
	fn a_listing_with_no_association_reports_nothing() {
		assert!(parse_shadow_storage("No shadow copy storage associations found.\n").is_none());
	}

	#[test]
	fn sizes_carry_their_units() {
		assert_eq!(parse_size(" 512 MB (5%)"), Some(512 * 1024 * 1024));
		assert_eq!(parse_size(" 1,024 KB (1%)"), Some(1024 * 1024));
		assert_eq!(parse_size(" nonsense"), None);
	}

	/// The record format is written by another crate, so the fields this check
	/// depends on are pinned here: a rename there should fail a test, not
	/// silently empty the listing.
	#[test]
	fn a_hold_record_parses_from_the_on_disk_shape() {
		let json = r#"{
			"id": "tamanu-postgres-20260814T054412Z",
			"backup_type": "tamanu-postgres",
			"taken_at": "2026-08-14T05:44:12Z",
			"held_at": "2026-08-14T11:02:00Z",
			"source": "/var/lib/bestool/held-source/x/16/main",
			"uploaded": true,
			"capture": { "backend": "btrfs", "toplevel_mount": "/x", "snapshot_path": "/x/y", "mount": "/z" }
		}"#;
		let record: HoldRecord = serde_json::from_str(json).unwrap();
		assert_eq!(record.id, "tamanu-postgres-20260814T054412Z");
		assert_eq!(record.backup_type, "tamanu-postgres");
		assert!(record.uploaded);
		assert!(record.taken_at.is_some());
	}

	/// A base backup has no freeze instant, and the check must still read it.
	#[test]
	fn a_record_without_a_freeze_instant_parses() {
		let json = r#"{
			"id": "x-20260814T054412Z",
			"backup_type": "x",
			"held_at": "2026-08-14T11:02:00Z",
			"source": "/var/lib/bestool/held-source/x",
			"uploaded": false,
			"capture": { "backend": "base-backup", "root": "/var/lib/bestool/held-source/x" }
		}"#;
		let record: HoldRecord = serde_json::from_str(json).unwrap();
		assert_eq!(record.taken_at, None);
	}
}
