//! Reporting a run's progress to Canopy while it is in flight.
//!
//! A [`ProgressReporter`] task samples the run's cumulative counters on a fixed
//! cadence and POSTs them, so Canopy shows a run advancing rather than a
//! figureless in-progress row. The counters come from two places: the kopia
//! engine's own progress (a [`ProgressCell`] the snapshot phase keeps current)
//! and the re-signing proxy's S3 tallies (read live through a [`TrafficHandle`]).
//!
//! Progress is best-effort telemetry: a failed or refused post is logged and the
//! run carries on. All figures are cumulative from the start of the run, so a
//! dropped or repeated sample costs only resolution, never a total.

use std::{
	sync::{Arc, Mutex},
	time::Duration,
};

use bestool_canopy::{
	CanopyClient,
	schema::{BackupPurpose, ProgressArgs},
};
use bestool_kopia::{proxy::TrafficHandle, progress::KopiaProgress};
use jiff::Timestamp;
use tokio::task::JoinHandle;
use tracing::debug;
use uuid::Uuid;

/// How often a run posts a progress sample while in flight. Comfortably above
/// the rate Canopy accepts, and fixed so a misconfiguration can't post faster.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(30);

/// The latest kopia engine progress, plus the verbatim line it was parsed from
/// (surfaced to Canopy as opaque detail).
#[derive(Clone)]
struct LatestKopia {
	raw: String,
	parsed: KopiaProgress,
}

/// A run's live progress, written by the run and read by its reporter task.
/// Backup runs share one; a restore has none (it reports only proxy tallies).
#[derive(Default)]
pub(crate) struct ProgressCell {
	kopia: Mutex<Option<LatestKopia>>,
	taken_at: Mutex<Option<Timestamp>>,
}

impl ProgressCell {
	/// Record the latest kopia progress line and its parse.
	pub(crate) fn set_kopia(&self, raw: String, parsed: KopiaProgress) {
		*self.kopia.lock().unwrap() = Some(LatestKopia { raw, parsed });
	}

	/// Record the instant the run froze its data. Write-once: the first value
	/// stands, matching Canopy's own rule.
	pub(crate) fn set_taken_at(&self, at: Timestamp) {
		let mut slot = self.taken_at.lock().unwrap();
		if slot.is_none() {
			*slot = Some(at);
		}
	}

	/// The freeze instant, if the run has recorded one.
	pub(crate) fn taken_at(&self) -> Option<Timestamp> {
		*self.taken_at.lock().unwrap()
	}

	fn latest_kopia(&self) -> Option<LatestKopia> {
		self.kopia.lock().unwrap().clone()
	}
}

/// A background task posting a run's progress to Canopy until stopped.
pub(crate) struct ProgressReporter {
	handle: JoinHandle<()>,
}

impl ProgressReporter {
	/// Start sampling and posting progress for a run. `cell` carries the engine
	/// counters and freeze instant for a backup; a restore passes `None` and
	/// reports proxy tallies only.
	pub(crate) fn spawn(
		client: Arc<CanopyClient>,
		run_id: Uuid,
		backup_type: String,
		purpose: BackupPurpose,
		traffic: TrafficHandle,
		cell: Option<Arc<ProgressCell>>,
	) -> Self {
		let handle = tokio::spawn(async move {
			loop {
				let args = build_args(run_id, &backup_type, purpose, &traffic, cell.as_deref());
				if let Err(err) = client.backup_progress(&args).await {
					debug!(%err, "posting backup progress to canopy failed (ignored)");
				}
				tokio::time::sleep(SAMPLE_INTERVAL).await;
			}
		});
		Self { handle }
	}

	/// Stop sampling. The task is aborted rather than drained: a final sample is
	/// redundant (the completion report backfills from the last one), and aborting
	/// means a slow post can never delay the run's own completion.
	pub(crate) async fn stop(self) {
		self.handle.abort();
		let _ = self.handle.await;
	}
}

/// Build one cumulative progress sample from the current engine + proxy figures.
fn build_args(
	run_id: Uuid,
	backup_type: &str,
	purpose: BackupPurpose,
	traffic: &TrafficHandle,
	cell: Option<&ProgressCell>,
) -> ProgressArgs {
	let t = traffic.get();
	let to_i64 = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);

	let latest = cell.and_then(ProgressCell::latest_kopia);
	let engine = latest.as_ref().map(|l| &l.parsed);
	let taken_at = cell.and_then(ProgressCell::taken_at);

	// The verbatim kopia status line rides along as opaque detail Canopy stores
	// without interpreting.
	let mut extra = serde_json::Map::new();
	if let Some(l) = &latest {
		extra.insert("kopia_status".to_owned(), l.raw.clone().into());
	}

	ProgressArgs::builder()
		.run_id(run_id)
		.type_(backup_type.to_owned())
		.purpose(purpose)
		.maybe_bytes_hashed(engine.map(|p| p.hashed_bytes))
		.maybe_bytes_uploaded(engine.map(|p| p.uploaded_bytes))
		.maybe_bytes_cached(engine.map(|p| p.cached_bytes))
		.maybe_bytes_estimated(engine.and_then(|p| p.estimated_bytes))
		.maybe_files_done(engine.map(|p| p.hashed_files + p.cached_files))
		.maybe_errors(engine.and_then(|p| p.errors))
		.maybe_ignored_errors(engine.and_then(|p| p.ignored_errors))
		.maybe_snapshot_taken_at(taken_at)
		.s3_sent_raw_bytes(to_i64(t.sent_raw))
		.s3_sent_payload_bytes(to_i64(t.sent_payload))
		.s3_received_raw_bytes(to_i64(t.received_raw))
		.s3_received_payload_bytes(to_i64(t.received_payload))
		.extra(extra)
		.build()
}

#[cfg(test)]
mod tests {
	use super::*;

	fn run_id() -> Uuid {
		"00000000-0000-0000-0000-000000000000".parse().unwrap()
	}

	#[test]
	fn backup_sample_maps_engine_and_traffic() {
		let handle = TrafficHandle::for_test(2_000_000, 1_950_000, 4096, 0);

		let cell = ProgressCell::default();
		cell.set_kopia(
			" | 2 hashing, 15 hashed (1.2 GB), 3 cached (100 MB), uploaded 1.1 GB, estimated 5 GB (24.0%) 1m left".to_owned(),
			KopiaProgress {
				hashing_files: 2,
				hashed_files: 15,
				hashed_bytes: 1_200_000_000,
				cached_files: 3,
				cached_bytes: 100_000_000,
				uploaded_bytes: 1_100_000_000,
				estimated_bytes: Some(5_000_000_000),
				errors: None,
				ignored_errors: None,
			},
		);
		cell.set_taken_at("2026-07-28T04:12:00Z".parse().unwrap());

		let args = build_args(run_id(), "tamanu-postgres", BackupPurpose::Backup, &handle, Some(&cell));

		assert_eq!(args.bytes_hashed, Some(1_200_000_000));
		assert_eq!(args.bytes_uploaded, Some(1_100_000_000));
		assert_eq!(args.bytes_cached, Some(100_000_000));
		assert_eq!(args.bytes_estimated, Some(5_000_000_000));
		assert_eq!(args.files_done, Some(18));
		// Not exposed by the engine line: omitted, not zero.
		assert_eq!(args.bytes_read, None);
		assert_eq!(args.files_estimated, None);
		assert_eq!(args.current_path, None);
		assert_eq!(args.s3_sent_raw_bytes, Some(2_000_000));
		assert_eq!(args.s3_received_raw_bytes, Some(4096));
		assert_eq!(
			args.snapshot_taken_at,
			Some("2026-07-28T04:12:00Z".parse().unwrap())
		);
		assert!(args.extra.contains_key("kopia_status"));
	}

	#[test]
	fn restore_sample_has_traffic_only() {
		let handle = TrafficHandle::for_test(0, 0, 500_000_000, 480_000_000);

		// No cell: a restore reports proxy tallies only.
		let args = build_args(run_id(), "tamanu-postgres", BackupPurpose::Restore, &handle, None);

		assert_eq!(args.purpose, Some(BackupPurpose::Restore));
		assert_eq!(args.s3_received_raw_bytes, Some(500_000_000));
		assert_eq!(args.bytes_uploaded, None);
		assert_eq!(args.bytes_hashed, None);
		assert_eq!(args.snapshot_taken_at, None);
		assert!(args.extra.is_empty());
	}
}
