//! Finding room for an in-place restore, which is not the question a staged one
//! asks.
//!
//! Named for what it answers rather than for the resource, because there are two
//! resources and the whole point is that they are different numbers.
//!
//! A staged restore needs free space for a whole second copy of the capture. An
//! in-place one writes only the divergence, and what that costs depends on
//! *where* the capture lives:
//!
//! - **Filesystem free space** absorbs the *net growth* — bytes added by new and
//!   grown entries, less bytes freed by deleted and shrunk ones. On a rollback
//!   this is usually small and often negative.
//! - **A copy-on-write store** absorbs the *total bytes written*, because every
//!   block written over a block the capture still references is copied aside
//!   first. Which store that is depends on the backend, and for a snapshot on
//!   the same filesystem it is that filesystem's own free space.
//!
//! Getting this wrong in the safe direction still fails the operator: gating an
//! in-place restore on the size of the capture refuses a restore that fits,
//! which is the whole reason this mode exists.

use std::path::Path;

use miette::{Result, bail};
use tracing::{debug, info, warn};

use super::sync::Delta;
use crate::actions::canopy::backup::{
	hold::{HeldCapture, HoldRecord},
	// The same questions the staged path asks, so the same answers: free space on
	// the volume backing a path that may not exist yet, and one rendering of a
	// byte count across every refusal an operator might see.
	postgresql::space as pg_space,
};

/// Headroom over an estimate, for filesystem overhead, rounding, and the writes
/// a stopped cluster's neighbours make while the restore runs.
fn with_headroom(bytes: u64) -> u64 {
	bytes.saturating_add(bytes / 20).max(64 * 1024 * 1024)
}

/// Where the capture's copy-on-write cost lands.
#[derive(Debug)]
pub enum CowStore {
	/// The capture is a snapshot sharing the live tree's own free space, so
	/// every byte written over a block it references comes out of that.
	SameFilesystem,
	/// A store of its own, with its own headroom and its own way of running out.
	Separate {
		/// What to call it when there is not enough.
		name: String,
		/// Bytes it can still absorb, where that can be read.
		available: Option<u64>,
		/// What an operator does about a shortfall.
		remedy: String,
	},
	/// The capture is a copy of its own, so overwriting the live tree copies
	/// nothing aside and costs only what it adds.
	None,
}

/// Which store a held capture's writes are charged to.
pub async fn cow_store(record: &HoldRecord) -> CowStore {
	match &record.capture {
		// A btrfs snapshot's retained extents come out of the same filesystem the
		// live tree is on, so there is one pool and one number.
		HeldCapture::Btrfs { .. } => CowStore::SameFilesystem,
		#[cfg(unix)]
		HeldCapture::Lvm { vg, lv, .. } => thin_pool_store(vg, lv).await,
		#[cfg(not(unix))]
		HeldCapture::Lvm { .. } => CowStore::SameFilesystem,
		#[cfg(windows)]
		HeldCapture::Vss { .. } => shadow_storage(record).await,
		#[cfg(not(windows))]
		HeldCapture::Vss { .. } => CowStore::SameFilesystem,
		// A staged base backup is an ordinary directory of its own.
		HeldCapture::BaseBackup { .. } => CowStore::None,
	}
}

/// Refuse an in-place restore that does not fit, before anything is written.
///
/// `live` is the tree being restored onto; its filesystem takes the net growth.
/// `cow_bytes`, where a backend can give one, is an exact block-level count of
/// what diverged — a better figure than the walk's for the copy-on-write store,
/// and meaningless for the filesystem, which cares how much the tree *grows*
/// rather than how much is written over.
pub async fn ensure_room(
	live: &Path,
	delta: &Delta,
	store: &CowStore,
	cow_bytes: Option<u64>,
) -> Result<()> {
	let growth = delta.net_growth();
	// The exact figure can only raise the bar: it counts blocks the walk may not
	// have attributed to any one entry, never fewer.
	let written = delta.written_bytes().max(cow_bytes.unwrap_or(0));
	info!(
		growth,
		written,
		"sized the divergence between the capture and the live tree"
	);

	// Where the copy-on-write store is the filesystem itself, the two demands are
	// on one pool and the larger governs — writing over a snapshotted block costs
	// the block whether or not the file grew.
	let filesystem_need = match store {
		CowStore::SameFilesystem => growth.max(written),
		CowStore::Separate { .. } | CowStore::None => growth,
	};

	if filesystem_need > 0 {
		let required = with_headroom(filesystem_need);
		let Some(available) = pg_space::available(live) else {
			// Not knowing is not a reason to refuse a rollback an operator is
			// depending on; the write itself reports a full filesystem plainly.
			warn!(
				"could not read the free space on {}; restoring in place needs about {} there",
				live.display(),
				pg_space::fmt_bytes(required),
			);
			return check_cow_store(store, written);
		};
		if available < required {
			bail!(
				"restoring in place needs about {} free on {} but only {} is available; \
				 free up space and retry",
				pg_space::fmt_bytes(required),
				live.display(),
				pg_space::fmt_bytes(available),
			);
		}
		debug!(required, available, "the live filesystem has room for the divergence");
	}

	check_cow_store(store, written)
}

/// The second of the two resources: what a copy-on-write store has to absorb.
fn check_cow_store(store: &CowStore, written: u64) -> Result<()> {
	if let CowStore::Separate {
		name,
		available,
		remedy,
	} = store
	{
		let required = with_headroom(written);
		match available {
			Some(available) if *available < required => bail!(
				"restoring in place writes about {} over blocks the capture still \
				 references, which {name} has to hold, but only {} of it is free; \
				 {remedy}",
				pg_space::fmt_bytes(required),
				pg_space::fmt_bytes(*available),
			),
			Some(available) => {
				debug!(required, available = *available, "{name} has room for the divergence");
			}
			// Not knowing is not a reason to refuse, but it is a reason to say so:
			// this store filling is what deletes the capture mid-restore.
			None => warn!(
				"could not read how much room is left in {name}; restoring in place \
				 writes about {} through it, and the capture is lost if it fills",
				pg_space::fmt_bytes(required),
			),
		}
	}

	Ok(())
}

/// The fixed allocation behind a thick LVM snapshot, and what is left of it.
#[cfg(unix)]
async fn thick_snapshot_store(vg: &str, lv: &str) -> CowStore {
	let qualified = format!("{vg}/{lv}");
	let size: Option<u64> = super::blockdev::lvs("lv_size", &qualified, true)
		.await
		.and_then(|raw| raw.parse().ok());
	let used: Option<f64> = super::blockdev::lvs("snap_percent", &qualified, false)
		.await
		.and_then(|raw| raw.parse().ok());
	let available = match (size, used) {
		#[expect(
			clippy::cast_precision_loss,
			clippy::cast_possible_truncation,
			clippy::cast_sign_loss,
			reason = "a snapshot's fill is reported as a percentage, so the answer is \
			          approximate by construction and only ever compared against an estimate"
		)]
		(Some(size), Some(used)) => Some((size as f64 * (100.0 - used) / 100.0) as u64),
		_ => None,
	};
	CowStore::Separate {
		name: format!("the snapshot {qualified}'s own allocation"),
		available,
		remedy: format!("extend it with `lvextend {qualified}` first"),
	}
}

/// The thin pool behind a held LVM capture, and what is left in it.
#[cfg(unix)]
async fn thin_pool_store(vg: &str, lv: &str) -> CowStore {
	let Some(pool) = super::blockdev::lvs("pool_lv", &format!("{vg}/{lv}"), false).await else {
		// Not in a pool, so it is a thick snapshot: the blocks the origin displaces
		// are copied into the *snapshot's own* fixed allocation, not into the live
		// filesystem. Charging the writes to the filesystem would refuse restores
		// that comfortably fit, and would leave unwatched the store that actually
		// runs out — a thick snapshot that fills is invalidated outright, which is
		// the capture vanishing partway through overwriting the destination.
		return thick_snapshot_store(vg, lv).await;
	};
	let qualified = format!("{vg}/{pool}");
	let size: Option<u64> = super::blockdev::lvs("lv_size", &qualified, true)
		.await
		.and_then(|raw| raw.parse().ok());
	let used: Option<f64> = super::blockdev::lvs("data_percent", &qualified, false)
		.await
		.and_then(|raw| raw.parse().ok());
	let available = match (size, used) {
		#[expect(
			clippy::cast_precision_loss,
			clippy::cast_possible_truncation,
			clippy::cast_sign_loss,
			reason = "a pool's free space is reported as a percentage, so the answer is \
			          approximate by construction and only ever compared against an estimate"
		)]
		(Some(size), Some(used)) => Some((size as f64 * (100.0 - used) / 100.0) as u64),
		_ => None,
	};
	CowStore::Separate {
		name: format!("the thin pool {qualified}"),
		available,
		remedy: format!("extend it with `lvextend --poolmetadatasize` / `lvextend {qualified}` first"),
	}
}

/// The shadow storage area a held VSS capture lives in, and its headroom.
///
/// This is the resource an in-place restore on Windows actually spends: when the
/// diff area reaches its cap VSS deletes shadows to make room, and a hold is
/// often the only one there. The source would then vanish partway through a copy
/// that has already overwritten the destination, silently, until the next read
/// fails.
#[cfg(windows)]
async fn shadow_storage(record: &HoldRecord) -> CowStore {
	// The volume the shadow was taken of, as the capture recorded it. Not guessed
	// from the capture's path: that is the junction bestool made to expose the
	// shadow, which need not be on the shadowed volume, and a headroom figure for
	// the wrong volume is worse than none — this is the number whose whole job is
	// to stop VSS deleting the capture mid-copy.
	let Some(volume) = shadowed_volume(record) else {
		warn!(
			"could not tell which volume hold {} was shadowed from, so its shadow \
			 storage headroom is unknown; the capture is lost if that store fills",
			record.id,
		);
		return CowStore::Separate {
			name: "the volume's shadow copy storage".to_owned(),
			available: None,
			remedy: "check it with `vssadmin list shadowstorage`".to_owned(),
		};
	};

	// Resolved absolutely rather than by name: this runs elevated, and Windows
	// searches the application directory (and sometimes the working directory)
	// before the system one, so a planted `vssadmin.exe` would run as admin.
	let vssadmin = std::path::PathBuf::from(
		std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()),
	)
	.join("System32")
	.join("vssadmin.exe");
	let available = tokio::process::Command::new(vssadmin)
		.args(["list", "shadowstorage", &format!("/for={volume}")])
		.stdin(std::process::Stdio::null())
		.output()
		.await
		.ok()
		.filter(|output| output.status.success())
		.and_then(|output| parse_shadow_headroom(&String::from_utf8_lossy(&output.stdout)));
	CowStore::Separate {
		name: format!("{volume}'s shadow copy storage"),
		available,
		remedy: format!(
			"raise the cap with `vssadmin resize shadowstorage /for={volume} /on=<vol> /maxsize=<size>` first"
		),
	}
}

/// Which volume a held shadow copy was taken of.
///
/// The capture records it when it notes the change journal's position, which is
/// read from the shadowed volume itself. Without that there is nothing here
/// worth guessing from.
#[cfg(windows)]
fn shadowed_volume(record: &HoldRecord) -> Option<String> {
	match &record.diverged_since {
		Some(crate::actions::canopy::backup::hold::DivergenceMark::UsnJournal { volume, .. }) => {
			Some(volume.to_str()?.to_owned())
		}
		_ => None,
	}
}

/// Bytes the shadow storage area can still take, from `vssadmin list
/// shadowstorage`: its cap less what is already used.
///
/// Best-effort by nature — the output is human-facing and localised — so an
/// unrecognised shape yields `None` and the restore proceeds with a warning
/// rather than being refused on a parse.
#[cfg(windows)]
fn parse_shadow_headroom(output: &str) -> Option<u64> {
	let used = find_size(output, "Used Shadow Copy Storage space");
	let maximum = find_size(output, "Maximum Shadow Copy Storage space");
	match (used, maximum) {
		// An unbounded cap is bounded in practice by the volume it sits on, which
		// the filesystem check already covers.
		(_, Some(None)) => None,
		(Some(Some(used)), Some(Some(maximum))) => Some(maximum.saturating_sub(used)),
		_ => None,
	}
}

/// The size on the line labelled `label`. The outer `Option` is whether the line
/// was found at all; the inner is whether it carried a number, since `UNBOUNDED`
/// is a valid value for the cap.
#[cfg(windows)]
fn find_size(output: &str, label: &str) -> Option<Option<u64>> {
	let line = output
		.lines()
		.find(|line| line.trim_start().starts_with(label))?;
	let (_, value) = line.split_once(':')?;
	Some(parse_size(value.trim()))
}

/// A size as `vssadmin` prints one, e.g. `25.0 GB (2%)`. Its units are decimal
/// despite the binary spelling elsewhere in this tool; the number only ever
/// gates against an estimate, so the distinction does not change an outcome.
#[cfg(windows)]
fn parse_size(text: &str) -> Option<u64> {
	let text = text.split('(').next()?.trim();
	let (number, unit) = text.split_once(' ')?;
	let number: f64 = number.trim().replace(',', "").parse().ok()?;
	let scale: u64 = match unit.trim().to_ascii_uppercase().as_str() {
		"B" | "BYTES" => 1,
		"KB" => 1024,
		"MB" => 1024 * 1024,
		"GB" => 1024 * 1024 * 1024,
		"TB" => 1024_u64.pow(4),
		"PB" => 1024_u64.pow(5),
		_ => return None,
	};
	#[expect(
		clippy::cast_precision_loss,
		clippy::cast_possible_truncation,
		clippy::cast_sign_loss,
		reason = "vssadmin prints one decimal place, so the value is approximate as printed"
	)]
	Some((number * scale as f64) as u64)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn headroom_is_five_percent_over_a_floor() {
		// Small deltas are dominated by the floor, which covers the writes a
		// restore's own neighbours make while it runs.
		assert_eq!(with_headroom(0), 64 * 1024 * 1024);
		let ten_gib = 10 * 1024 * 1024 * 1024;
		assert_eq!(with_headroom(ten_gib), ten_gib + ten_gib / 20);
	}

	#[tokio::test]
	async fn a_delta_that_frees_more_than_it_writes_still_needs_room_on_a_shared_store() {
		// The point of the two numbers: 500 bytes are freed, so the tree shrinks,
		// but 1000 bytes are written over blocks the snapshot holds and a shared
		// store has to find all 1000.
		let delta = Delta {
			copy_bytes: 1000,
			displaced_bytes: 1000,
			remove_bytes: 500,
			..Default::default()
		};
		assert_eq!(delta.net_growth(), 0);
		assert_eq!(delta.written_bytes(), 1000);
	}

	#[tokio::test]
	async fn an_empty_delta_needs_nothing_anywhere() {
		let tmp = tempfile::tempdir().unwrap();
		ensure_room(tmp.path(), &Delta::default(), &CowStore::None, None)
			.await
			.unwrap();
	}

	#[tokio::test]
	async fn a_separate_store_without_room_refuses_and_says_what_to_do() {
		let tmp = tempfile::tempdir().unwrap();
		let delta = Delta {
			copy_bytes: 10 * 1024 * 1024 * 1024,
			displaced_bytes: 10 * 1024 * 1024 * 1024,
			..Default::default()
		};
		let store = CowStore::Separate {
			name: "the thin pool vg0/pool".into(),
			available: Some(1024),
			remedy: "extend it first".into(),
		};
		let err = ensure_room(tmp.path(), &delta, &store, None).await.unwrap_err().to_string();
		assert!(err.contains("the thin pool vg0/pool"), "got: {err}");
		assert!(err.contains("extend it first"), "got: {err}");
	}

	#[tokio::test]
	async fn a_separate_store_of_unknown_size_does_not_refuse() {
		// Not knowing is not a reason to block a rollback an operator is depending
		// on; it is a reason to warn.
		let tmp = tempfile::tempdir().unwrap();
		let delta = Delta {
			copy_bytes: 10 * 1024 * 1024 * 1024,
			displaced_bytes: 10 * 1024 * 1024 * 1024,
			..Default::default()
		};
		let store = CowStore::Separate {
			name: "shadow copy storage".into(),
			available: None,
			remedy: "raise the cap".into(),
		};
		ensure_room(tmp.path(), &delta, &store, None).await.unwrap();
	}

	#[cfg(windows)]
	#[test]
	fn reads_the_shadow_storage_headroom() {
		let output = "\r\nShadow Copy Storage association\r\n   For volume: (C:)\\\\?\\Volume{a}\\\r\n   Shadow Copy Storage volume: (C:)\\\\?\\Volume{a}\\\r\n   Used Shadow Copy Storage space: 25.0 GB (2%)\r\n   Allocated Shadow Copy Storage space: 26.0 GB (2%)\r\n   Maximum Shadow Copy Storage space: 100.0 GB (10%)\r\n";
		let headroom = parse_shadow_headroom(output).unwrap();
		assert_eq!(headroom, 75 * 1024 * 1024 * 1024);
	}

	#[cfg(windows)]
	#[test]
	fn an_unbounded_cap_is_not_a_number_to_gate_on() {
		let output = "   Used Shadow Copy Storage space: 25.0 GB (2%)\r\n   Maximum Shadow Copy Storage space: UNBOUNDED (100%)\r\n";
		assert_eq!(parse_shadow_headroom(output), None);
	}

	#[cfg(windows)]
	#[test]
	fn unrecognised_output_yields_nothing_rather_than_a_wrong_number() {
		assert_eq!(parse_shadow_headroom("No items found that satisfy the query."), None);
	}

	#[cfg(windows)]
	#[test]
	fn parses_the_sizes_vssadmin_prints() {
		assert_eq!(parse_size("25.0 GB (2%)"), Some(25 * 1024 * 1024 * 1024));
		assert_eq!(parse_size("512 MB"), Some(512 * 1024 * 1024));
		assert_eq!(parse_size("1,024 MB"), Some(1024 * 1024 * 1024));
		assert_eq!(parse_size("UNBOUNDED"), None);
	}
}
