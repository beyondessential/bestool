//! How much a thin LV has diverged from its snapshot, by block mapping.
//!
//! A thin pool records which blocks each thin device owns, and `thin_delta`
//! diffs two devices' mappings. That is the divergence, exactly, for the cost of
//! reading pool metadata.
//!
//! It is block-level, and a thin pool keeps no map from a block back to the file
//! that owns it, so it cannot say *which* entries diverged — only how many bytes
//! did. That is still the number the copy-on-write side of the space gate wants,
//! and it is far better than the estimate the walk arrives at, so this
//! contributes it and leaves the entries to the walk.
//!
//! Reading pool metadata means holding a metadata snapshot open, which needs
//! privileges the daemon may not have and tooling that may not be installed.
//! Every failure here is silent and returns `None`: the gate then uses the
//! walk's estimate, which is a worse number rather than a wrong answer.

use std::path::Path;

use tracing::debug;

use super::super::blockdev;
use crate::actions::canopy::backup::hold::{HeldCapture, HoldRecord};

/// Bytes that differ between the held snapshot LV and the live LV it rolls back
/// onto, or `None` where the pool cannot be asked.
pub async fn diverged_bytes(record: &HoldRecord, live: &Path) -> Option<u64> {
	let HeldCapture::Lvm { vg, lv, .. } = &record.capture else {
		return None;
	};

	let live_source = blockdev::findmnt("SOURCE", live).await?;
	let live_lv = blockdev::lvs("lv_name", &live_source, false).await?;
	let pool = blockdev::lvs("pool_lv", &format!("{vg}/{lv}"), false).await?;
	if pool.is_empty() {
		debug!("the held capture's LV is not in a thin pool, so its delta cannot be read");
		return None;
	}

	let held_id = blockdev::lvs("thin_id", &format!("{vg}/{lv}"), false).await?;
	let live_id = blockdev::lvs("thin_id", &format!("{vg}/{live_lv}"), false).await?;
	let block_size: u64 = blockdev::lvs("chunk_size", &format!("{vg}/{pool}"), true)
		.await?
		.parse()
		.ok()?;

	// Both the device-mapper name and the metadata device path go through the
	// mangling: an unmangled name is not merely unresolvable, it can name a
	// *different* pool, and a `reserve_metadata_snap` sent to one of those is then
	// never released.
	let pool_dm = blockdev::dm_name(vg, &pool);
	let metadata = format!("/dev/mapper/{pool_dm}_tmeta");

	blockdev::run("dmsetup", &["message", &pool_dm, "0", "reserve_metadata_snap"]).await?;
	let delta = blockdev::capture(
		"thin_delta",
		&["--snap1", &held_id, "--snap2", &live_id, "-m", &metadata],
	)
	.await;
	// Released whatever the diff did: a metadata snapshot left reserved is carried
	// by the pool until something else releases it.
	let _ = blockdev::run("dmsetup", &["message", &pool_dm, "0", "release_metadata_snap"]).await;

	let blocks = differing_blocks(&delta?)?;
	let bytes = blocks.saturating_mul(block_size);
	debug!(blocks, bytes, "thin_delta sized the divergence from the capture");
	Some(bytes)
}

/// The number of blocks `thin_delta` reports as differing.
///
/// Its XML has one element per run of blocks, `left_only` and `right_only` being
/// blocks only one side maps and `different` being blocks both map to different
/// data. All three are divergence; `same` is not.
fn differing_blocks(xml: &str) -> Option<u64> {
	let mut total = 0u64;
	let mut saw_any = false;
	for line in xml.lines() {
		let line = line.trim();
		if !line.starts_with('<') {
			continue;
		}
		let differing = line.starts_with("<left_only ")
			|| line.starts_with("<right_only ")
			|| line.starts_with("<different ");
		saw_any |= differing || line.starts_with("<same ");
		if !differing {
			continue;
		}
		let Some(length) = attribute(line, "length") else {
			continue;
		};
		total = total.saturating_add(length);
	}
	saw_any.then_some(total)
}

/// The value of a numeric XML attribute on one element line.
fn attribute(line: &str, name: &str) -> Option<u64> {
	let at = line.find(&format!("{name}=\""))? + name.len() + 2;
	let rest = &line[at..];
	let end = rest.find('"')?;
	rest[..end].parse().ok()
}

#[cfg(test)]
mod tests {
	use super::*;

	const DELTA: &str = r#"<superblock>
  <diff left="1" right="2">
    <same begin="0" length="1000"/>
    <different begin="1000" length="20"/>
    <left_only begin="1020" length="5"/>
    <right_only begin="1025" length="3"/>
    <same begin="1028" length="500"/>
  </diff>
</superblock>
"#;

	#[test]
	fn counts_every_kind_of_difference_and_no_sameness() {
		assert_eq!(differing_blocks(DELTA), Some(28));
	}

	#[test]
	fn nothing_diverged_is_zero_rather_than_unknown() {
		// A pool that answers "all the same" has sized the divergence at nothing,
		// which is a useful answer and not the same as being unable to say.
		let xml = "<superblock>\n<diff left=\"1\" right=\"2\">\n<same begin=\"0\" length=\"1000\"/>\n</diff>\n</superblock>";
		assert_eq!(differing_blocks(xml), Some(0));
	}

	#[test]
	fn output_with_no_mappings_at_all_is_unknown() {
		assert_eq!(differing_blocks("not xml at all"), None);
	}

	#[test]
	fn reads_an_attribute_off_an_element() {
		assert_eq!(attribute(r#"<different begin="10" length="20"/>"#, "length"), Some(20));
		assert_eq!(attribute(r#"<different begin="10" length="20"/>"#, "begin"), Some(10));
		assert_eq!(attribute("<different/>", "length"), None);
	}
}
