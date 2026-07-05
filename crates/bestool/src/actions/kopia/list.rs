use bestool_kopia::{
	Snapshot, build_filter, fetch_snapshots, format_tags, format_taken, human_bytes, parse_tag_kv,
	parse_tags, short_id,
};
use clap::Parser;
use comfy_table::{ContentArrangement, Table, modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL};
use jiff::Timestamp;
use miette::{Context as _, IntoDiagnostic as _, Result};

use super::{
	KopiaArgs,
	common::{current_hostname, kopia_binary},
};
use crate::actions::Context;

/// List kopia snapshots, defaulting to those from this host.
#[derive(Debug, Clone, Parser)]
pub struct ListArgs {
	/// List snapshots from every host (otherwise: only this hostname).
	#[arg(long, conflicts_with = "source_host")]
	pub all: bool,

	/// Filter by source host. Defaults to this machine's hostname.
	#[arg(long, value_name = "HOST")]
	pub source_host: Option<String>,

	/// Filter by tag. Repeatable. Format: `key:value`.
	#[arg(long = "tag", value_name = "KEY:VALUE", value_parser = parse_tag_arg)]
	pub tags: Vec<String>,

	/// Filter snapshots whose source path contains this substring (case-insensitive).
	#[arg(long, value_name = "SUBSTR")]
	pub path: Option<String>,

	/// Only show snapshots taken within this duration (e.g. `24h`, `7d`).
	#[arg(long, value_name = "DURATION")]
	pub since: Option<String>,

	/// Cap to the N most recent matches.
	#[arg(long, short = 'n', value_name = "N")]
	pub limit: Option<usize>,

	/// Emit machine-readable JSON instead of a table.
	#[arg(long)]
	pub json: bool,
}

fn parse_tag_arg(s: &str) -> Result<String, String> {
	parse_tag_kv(s).map(|_| s.to_string())
}

pub async fn run(args: ListArgs, ctx: Context) -> Result<()> {
	let kopia = ctx.require::<KopiaArgs>();
	let bin = kopia_binary(kopia)?;

	let snapshots = fetch_snapshots(&bin, &parse_tags(&args.tags)?)?;

	let filter = build_filter(
		args.all,
		args.source_host,
		current_hostname(),
		args.path,
		args.since.as_deref(),
		args.limit,
	)?;

	let now = Timestamp::now();
	let matches = filter.apply(&snapshots, now);

	if args.json {
		serde_json::to_writer_pretty(std::io::stdout().lock(), &matches)
			.into_diagnostic()
			.wrap_err("emitting JSON")?;
		println!();
	} else {
		render_table(&matches);
	}

	Ok(())
}

fn render_table(snapshots: &[Snapshot]) {
	if snapshots.is_empty() {
		println!("No matching snapshots.");
		return;
	}

	let mut table = Table::new();
	table
		.load_preset(UTF8_FULL)
		.apply_modifier(UTF8_ROUND_CORNERS)
		.set_content_arrangement(ContentArrangement::Dynamic)
		.set_header(vec!["ID", "Taken", "Source", "Size", "Tags"]);

	for snap in snapshots {
		table.add_row(vec![
			short_id(&snap.id),
			snap.taken_at()
				.map(format_taken)
				.unwrap_or_else(|| "—".into()),
			format!(
				"{}@{}:{}",
				snap.source.user_name, snap.source.host, snap.source.path
			),
			snap.total_size()
				.map(human_bytes)
				.unwrap_or_else(|| "—".into()),
			format_tags(&snap.tags),
		]);
	}

	println!("{table}");
}
