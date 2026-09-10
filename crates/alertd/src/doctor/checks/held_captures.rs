//! Captures held on this device as local rollback points.
//!
//! A held capture is created deliberately and released deliberately: nothing
//! expires it, and no later backup clears it. That is what makes it trustworthy
//! across an upgrade window, and it is also why it needs watching — a hold
//! nobody drops keeps costing storage indefinitely.
//!
//! Three conditions are reported, and they are not the same problem:
//!
//! - **Held a long time** — untidy, and more expensive the longer it runs.
//! - **Capture not mounted** — the capture is exposed by a mount, and that mount
//!   is not there. The capture behind it is very often still intact, so this is
//!   reported as its own condition with its own remedy.
//! - **Capture gone** — the mount is in place and the rollback point still can't
//!   be read. The operator believes they can roll back and cannot, and nothing
//!   about the hold itself gives that away.
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
	#[serde(default)]
	capture: HoldCapture,
}

/// Only what tells this check where a capture is exposed. The driver owns the
/// full shape; a backend it does not know about still parses, and is judged by
/// readability alone.
#[derive(Deserialize, Default)]
#[serde(tag = "backend", rename_all = "kebab-case")]
enum HoldCapture {
	Btrfs {
		mount: PathBuf,
	},
	Lvm {
		mount: PathBuf,
	},
	Vss {},
	BaseBackup {},
	#[serde(other)]
	#[default]
	Unknown,
}

/// The conditions one hold meets, which are independent of each other: a hold
/// nobody released is still costing storage whether or not its capture can be
/// read, and is worth saying so alongside.
#[derive(Debug, PartialEq, Eq)]
struct Conditions {
	gone: bool,
	detached: bool,
	stale: bool,
}

/// `attached` is `None` where nothing was probed: a readable capture, or a
/// backend with no exposure to probe. An unreadable capture whose exposure
/// cannot be judged is reported as detached rather than gone, since claiming a
/// rollback point is lost is the more expensive thing to get wrong.
fn classify(present: bool, attached: Option<bool>, held_days: i32) -> Conditions {
	Conditions {
		gone: !present && attached == Some(true),
		detached: !present && attached != Some(true),
		stale: held_days >= STALE_AFTER_DAYS,
	}
}

impl HoldCapture {
	/// Where the capture is attached, for the backends that expose one through a
	/// mount. A base backup is a plain directory, so an unreadable one is gone
	/// rather than detached.
	fn exposure(&self) -> Option<&std::path::Path> {
		match self {
			Self::Btrfs { mount } | Self::Lvm { mount } => Some(mount),
			// A shadow copy is either there or it is not, and the driver grades it
			// that way too. Reporting a lost junction as detached would have the
			// two disagree about the same hold.
			Self::Vss {} | Self::BaseBackup {} | Self::Unknown => None,
		}
	}
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
	let mut detached: Vec<String> = Vec::new();
	let mut stale: Vec<String> = Vec::new();
	let mut details: Vec<Value> = Vec::new();

	for record in &records {
		let present = capture_readable(&record.source).await;
		let held_days = held_days(now - record.held_at);
		let attached = match record.capture.exposure() {
			Some(path) if !present => Some(is_attached(path).await),
			_ => None,
		};

		let conditions = classify(present, attached, held_days);
		if conditions.gone {
			missing.push(format!("{} (capture gone)", record.id));
		}
		if conditions.detached {
			detached.push(record.id.clone());
		}
		if conditions.stale {
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
			"captureAttached": attached,
			"captureState": if conditions.gone {
				"gone"
			} else if conditions.detached {
				"detached"
			} else {
				"present"
			},
		}));
	}

	let mut stats = vec![Stat::gauge("held_captures", records.len() as f64)];
	let storage = shadow_storage().await;
	if let Some(free) = storage.as_ref().and_then(ShadowStorage::free_bytes) {
		stats.push(Stat::gauge("shadow_storage_free_bytes", free as f64));
	}

	let summary = format!("{} capture(s) held", records.len());
	let mut reasons: Vec<String> = Vec::new();
	if !missing.is_empty() {
		reasons.push(format!(
			"the capture behind {} is gone, so it is not a rollback point: {}",
			if missing.len() == 1 {
				"a hold"
			} else {
				"holds"
			},
			missing.join(", ")
		));
	}
	if !detached.is_empty() {
		reasons.push(format!(
			"the capture behind {} is not mounted, so it cannot be read as a rollback \
			 point until it is reattached; the underlying capture is often still \
			 intact: {}",
			if detached.len() == 1 {
				"a hold"
			} else {
				"holds"
			},
			detached.join(", ")
		));
	}
	if !stale.is_empty() {
		reasons.push(format!(
			"held for over {STALE_AFTER_DAYS} days and still costing storage: {}; \
			 release with `bestool canopy hold drop <id>`",
			stale.join(", ")
		));
	}

	let check = match reasons.is_empty() {
		true => Check::pass(NAME, summary),
		false if missing.is_empty() => Check::warning(NAME, summary, reasons.join("; ")),
		false => Check::fail(NAME, summary, reasons.join("; ")),
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

/// Whether the capture can be read, which is what makes it a rollback point.
/// Why it cannot is a separate question: [`is_attached`] tells a capture that is
/// merely detached from one that is gone.
async fn capture_readable(source: &std::path::Path) -> bool {
	tokio::fs::read_dir(source)
		.await
		.map(|_| true)
		.unwrap_or(false)
}

/// Whole days a hold has been held. Elapsed time rather than calendar days, so
/// the span is rounded against invariant 24-hour days: rounding to a calendar
/// unit without a reference date is an error, not a zero.
fn held_days(span: jiff::Span) -> i32 {
	span.round(
		jiff::SpanRound::new()
			.largest(Unit::Day)
			.relative(jiff::SpanRelativeTo::days_are_24_hours()),
	)
	.map(|rounded| rounded.get_days())
	.unwrap_or(0)
}

/// Whether a capture's exposure path currently has something mounted on it,
/// judged by its device differing from its parent's.
///
/// A path that is not there at all counts as attached: nothing is going to
/// appear at it, so the capture is gone rather than waiting to be reattached.
#[cfg(unix)]
async fn is_attached(path: &std::path::Path) -> bool {
	use std::os::unix::fs::MetadataExt as _;

	let Some(parent) = path.parent() else {
		return true;
	};
	let Ok(here) = tokio::fs::metadata(path).await else {
		return true;
	};
	let Ok(above) = tokio::fs::metadata(parent).await else {
		return false;
	};
	here.dev() != above.dev()
}

/// A held shadow copy is reached through a junction, which is either there or
/// not.
#[cfg(not(unix))]
async fn is_attached(path: &std::path::Path) -> bool {
	tokio::fs::symlink_metadata(path).await.is_ok()
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

	/// Rounding a span to days needs a reference for what a day is worth. Without
	/// one it is an error, and an error swallowed here would read as a hold that
	/// is always brand new, so the staleness condition would never be reached.
	#[test]
	fn a_holds_age_counts_the_days_that_have_elapsed() {
		let held: Timestamp = "2026-07-01T10:00:00Z".parse().unwrap();
		let days = |s: &str| {
			let now: Timestamp = s.parse().unwrap();
			held_days(now - held)
		};

		assert_eq!(days("2026-07-01T22:00:00Z"), 0);
		assert_eq!(days("2026-07-08T09:59:00Z"), 6);
		assert_eq!(days("2026-07-08T10:00:00Z"), 7);
		assert_eq!(days("2026-09-09T10:00:00Z"), 70);
	}

	/// Staleness is not conditional on the capture being readable. A hold that is
	/// both detached and long forgotten has to report both, since reattaching it
	/// and releasing it are different remedies and the second still applies.
	#[test]
	fn a_detached_hold_is_still_reported_as_stale() {
		assert_eq!(
			classify(false, Some(false), 8),
			Conditions {
				gone: false,
				detached: true,
				stale: true,
			}
		);
	}

	/// A backend with no exposure to probe cannot be called gone with confidence,
	/// and claiming a rollback point is lost is the more expensive mistake.
	#[test]
	fn an_unreadable_capture_with_nothing_to_probe_is_not_called_gone() {
		assert_eq!(
			classify(false, None, 0),
			Conditions {
				gone: false,
				detached: true,
				stale: false,
			}
		);
	}

	#[test]
	fn a_capture_is_gone_only_when_what_exposes_it_is_in_place() {
		// Readable: nothing to report but its age.
		assert_eq!(
			classify(true, None, 0),
			Conditions {
				gone: false,
				detached: false,
				stale: false,
			}
		);
		// Unreadable while attached: the capture really has gone.
		assert_eq!(
			classify(false, Some(true), 0),
			Conditions {
				gone: true,
				detached: false,
				stale: false,
			}
		);
		// Readable and long held: the cleanup case on its own.
		assert_eq!(
			classify(true, None, 9),
			Conditions {
				gone: false,
				detached: false,
				stale: true,
			}
		);
	}

	/// A held capture reported as stale has to actually reach the threshold.
	#[test]
	fn a_hold_past_the_threshold_reads_as_stale() {
		let held: Timestamp = "2026-07-01T10:00:00Z".parse().unwrap();
		let now: Timestamp = "2026-09-09T10:00:00Z".parse().unwrap();
		assert!(held_days(now - held) >= STALE_AFTER_DAYS);
	}

	#[test]
	fn a_capture_is_exposed_where_its_backend_attaches_it() {
		let mount = |json: &str| {
			serde_json::from_str::<HoldCapture>(json)
				.unwrap()
				.exposure()
				.map(|path| path.display().to_string())
		};

		assert_eq!(
			mount(r#"{ "backend": "btrfs", "mount": "/z" }"#),
			Some("/z".to_owned())
		);
		assert_eq!(
			mount(r#"{ "backend": "lvm", "vg": "v", "lv": "l", "mount": "/z" }"#),
			Some("/z".to_owned())
		);
		// A shadow copy is graded present-or-gone, as the driver grades it.
		assert_eq!(
			mount(r#"{ "backend": "vss", "shadow_id": "s", "junction": "C:\\j" }"#),
			None
		);
		// A base backup is the data itself, so there is nothing to reattach.
		assert_eq!(mount(r#"{ "backend": "base-backup", "root": "/r" }"#), None);
		assert_eq!(mount(r#"{ "backend": "something-later" }"#), None);
	}

	/// A record the driver wrote before it carried a capture must still be
	/// listed: dropping it would hide a hold rather than report it.
	#[test]
	fn a_record_without_a_capture_parses() {
		let json = r#"{
			"id": "x-20260814T054412Z",
			"backup_type": "x",
			"held_at": "2026-08-14T11:02:00Z",
			"source": "/var/lib/bestool/held-source/x",
			"uploaded": false
		}"#;
		let record: HoldRecord = serde_json::from_str(json).unwrap();
		assert!(record.capture.exposure().is_none());
	}

	#[cfg(unix)]
	#[tokio::test]
	async fn a_directory_with_nothing_mounted_on_it_is_not_attached() {
		let dir = std::env::temp_dir().join(format!(
			"bestool-held-attach-{}-{}",
			std::process::id(),
			Timestamp::now().as_nanosecond()
		));
		std::fs::create_dir_all(&dir).unwrap();

		assert!(!is_attached(&dir).await);
		// Nothing will ever appear at a path that is not there, so it counts as
		// attached and the capture reads as gone rather than waiting to come back.
		assert!(is_attached(&dir.join("missing")).await);

		std::fs::remove_dir_all(&dir).unwrap();
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
