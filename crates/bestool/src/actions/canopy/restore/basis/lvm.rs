//! How much a thin LV has diverged from its snapshot, by block mapping.
//!
//! A thin pool records which blocks each thin device owns, and `thin_delta`
//! diffs two devices' mappings. That is the divergence, exactly, for the cost of
//! reading pool metadata.
//!
//! It is block-level, and a thin pool keeps no map from a block back to the file
//! that owns it, so it cannot say *which* entries diverged — only how many bytes
//! did. That is still the number the space gate wants, and it is far better than
//! the estimate the walk arrives at, so this contributes it and leaves the
//! entries to the walk.
//!
//! Reading pool metadata means holding a metadata snapshot open, which needs
//! privileges the daemon may not have and tooling that may not be installed.
//! Every failure here is silent and returns `None`: the space gate then uses the
//! walk's estimate, which is a worse number rather than a wrong answer.

use std::path::Path;

use tokio::process::Command;
use tracing::debug;

use crate::actions::canopy::backup::hold::{HeldCapture, HoldRecord};

/// Bytes that differ between the held snapshot LV and the live LV it rolls back
/// onto, or `None` where the pool cannot be asked.
pub async fn diverged_bytes(record: &HoldRecord, live: &Path) -> Option<u64> {
	let HeldCapture::Lvm { vg, lv, .. } = &record.capture else {
		return None;
	};

	let live_source = capture(
		"findmnt",
		&["-n", "-o", "SOURCE", "--target", &live.to_string_lossy()],
	)
	.await?;
	let live_lv = capture(
		"lvs",
		&["--noheadings", "-o", "lv_name", live_source.trim()],
	)
	.await?;
	let pool = capture(
		"lvs",
		&["--noheadings", "-o", "pool_lv", &format!("{vg}/{lv}")],
	)
	.await?;
	let pool = pool.trim();
	if pool.is_empty() {
		debug!("the held capture's LV is not in a thin pool, so its delta cannot be read");
		return None;
	}

	let held_id = thin_id(vg, lv).await?;
	let live_id = thin_id(vg, live_lv.trim()).await?;
	let block_size = chunk_bytes(vg, pool).await?;

	// The metadata snapshot has to be reserved for the diff and released after,
	// or the pool carries it until something else does.
	let metadata = format!("/dev/mapper/{}-{}_tmeta", mangle(vg), mangle(pool));
	run_ok("dmsetup", &["message", &format!("{vg}-{pool}"), "0", "reserve_metadata_snap"]).await?;
	let delta = capture(
		"thin_delta",
		&[
			"--snap1",
			&held_id,
			"--snap2",
			&live_id,
			"-m",
			&metadata,
		],
	)
	.await;
	let _ = run_ok(
		"dmsetup",
		&["message", &format!("{vg}-{pool}"), "0", "release_metadata_snap"],
	)
	.await;

	let blocks = differing_blocks(&delta?)?;
	let bytes = blocks.saturating_mul(block_size);
	debug!(blocks, bytes, "thin_delta sized the divergence from the capture");
	Some(bytes)
}

/// The thin device id of an LV within its pool.
async fn thin_id(vg: &str, lv: &str) -> Option<String> {
	let out = capture(
		"lvs",
		&["--noheadings", "-o", "thin_id", &format!("{vg}/{lv}")],
	)
	.await?;
	let id = out.trim().to_owned();
	(!id.is_empty()).then_some(id)
}

/// The pool's block size in bytes, which is what a differing block costs.
async fn chunk_bytes(vg: &str, pool: &str) -> Option<u64> {
	let out = capture(
		"lvs",
		&[
			"--noheadings",
			"--units",
			"b",
			"--nosuffix",
			"-o",
			"chunk_size",
			&format!("{vg}/{pool}"),
		],
	)
	.await?;
	out.trim().parse().ok()
}

/// Device-mapper names escape hyphens by doubling them, so a VG or LV with one
/// in its name does not resolve under its plain spelling.
fn mangle(name: &str) -> String {
	name.replace('-', "--")
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
		let differing = line.starts_with("<left_only ")
			|| line.starts_with("<right_only ")
			|| line.starts_with("<different ");
		if !line.starts_with('<') {
			continue;
		}
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

async fn capture(program: &str, args: &[&str]) -> Option<String> {
	let output = Command::new(program)
		.args(args)
		.stdin(std::process::Stdio::null())
		.output()
		.await
		.inspect_err(|err| debug!("could not run {program}: {err}"))
		.ok()?;
	if !output.status.success() {
		debug!("{program} exited {}", output.status);
		return None;
	}
	Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

async fn run_ok(program: &str, args: &[&str]) -> Option<()> {
	capture(program, args).await.map(|_| ())
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

	#[test]
	fn device_mapper_names_double_their_hyphens() {
		assert_eq!(mangle("vg-data"), "vg--data");
		assert_eq!(mangle("vg0"), "vg0");
	}
}
