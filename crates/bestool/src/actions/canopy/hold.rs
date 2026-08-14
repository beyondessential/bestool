//! Operator commands for captures held on this device.
//!
//! A hold is device-local state: nothing schedules it, nothing expires it, and
//! Canopy plays no part in its lifecycle. It is created by a backup asked to
//! keep its capture, and released here.

use clap::{Parser, Subcommand};
use jiff::{SpanRound, Timestamp, Unit};
use miette::{Result, bail};
use tracing::{info, warn};

use super::{
	super::Context,
	backup::{DaemonError, hold, request_daemon_hold},
};

/// Manage captures held on this device as local rollback points.
#[derive(Debug, Clone, Parser)]
pub struct HoldArgs {
	#[command(subcommand)]
	pub action: HoldAction,
}

#[derive(Debug, Clone, Subcommand)]
pub enum HoldAction {
	/// Tell a backup that is already running to keep its capture.
	///
	/// Takes effect when the run finishes; the transfer in progress is not
	/// interrupted or restarted, so a run that has already spent hours uploading
	/// keeps that work and leaves a local rollback point behind as well.
	///
	/// Only reaches a run hosted by the alertd daemon.
	Keep(KeepArgs),

	/// List the captures held on this device.
	List,

	/// Release a held capture and forget it.
	Drop(DropArgs),
}

#[derive(Debug, Clone, Parser)]
pub struct KeepArgs {
	/// The backup type whose running backup should keep its capture.
	#[arg(long = "type", value_name = "TYPE")]
	pub backup_type: String,
}

#[derive(Debug, Clone, Parser)]
pub struct DropArgs {
	/// The hold to release, as shown by `bestool canopy hold list`.
	pub id: String,
}

pub async fn run(args: HoldArgs, _ctx: Context) -> Result<()> {
	match args.action {
		HoldAction::Keep(args) => keep(&args.backup_type).await,
		HoldAction::List => list().await,
		HoldAction::Drop(args) => drop_hold(&args.id).await,
	}
}

async fn keep(backup_type: &str) -> Result<()> {
	match request_daemon_hold(backup_type).await {
		Ok(true) => {
			info!(
				backup_type,
				"the running backup will keep its capture; release it with `bestool canopy hold drop`"
			);
			Ok(())
		}
		Ok(false) => bail!(
			"no backup of type '{backup_type}' is running in the alertd daemon, so there is no \
			 capture to keep; start one with `bestool canopy backup --type {backup_type} --hold`"
		),
		Err(DaemonError::Unreachable(err)) => bail!(
			"the alertd daemon is not reachable ({err}), so a run in flight can't be told to keep \
			 its capture; only a daemon-hosted run can be reached this way"
		),
		Err(DaemonError::Failed(message)) => bail!("asking the daemon to keep the capture failed: {message}"),
	}
}

async fn list() -> Result<()> {
	let records = hold::list().await?;
	if records.is_empty() {
		println!("no held captures on this device");
		return Ok(());
	}

	let now = Timestamp::now();
	println!(
		"{:<40}  {:<10}  {:<21}  {:<10}  {:<8}  CAPTURE",
		"ID", "BACKEND", "FROZEN", "HELD FOR", "UPLOADED"
	);
	for record in &records {
		let present = hold::capture_present(&record.capture).await;
		println!(
			"{:<40}  {:<10}  {:<21}  {:<10}  {:<8}  {}",
			record.id,
			record.capture.backend(),
			record
				.taken_at
				.map_or_else(|| "(no freeze instant)".to_owned(), |at| at.to_string()),
			humanise(now - record.held_at),
			if record.uploaded { "yes" } else { "no" },
			if present { "present" } else { "MISSING" },
		);
	}

	let missing = futures::future::join_all(
		records
			.iter()
			.map(|record| hold::capture_present(&record.capture)),
	)
	.await
	.into_iter()
	.filter(|present| !present)
	.count();
	if missing > 0 {
		warn!(
			"{missing} of {} held captures are gone; those holds are not rollback points",
			records.len()
		);
	}
	Ok(())
}

/// A coarse age for a listing: which day or hour it is matters, minutes do not.
fn humanise(span: jiff::Span) -> String {
	span.round(SpanRound::new().largest(Unit::Day).smallest(Unit::Minute))
		.map(|rounded| {
			let days = rounded.get_days();
			let hours = rounded.get_hours();
			if days > 0 {
				format!("{days}d {hours}h")
			} else {
				format!("{hours}h {}m", rounded.get_minutes())
			}
		})
		.unwrap_or_else(|_| "unknown".to_owned())
}

async fn drop_hold(id: &str) -> Result<()> {
	let record = hold::load(id).await?;
	if hold::capture_present(&record.capture).await {
		hold::release(&record.capture).await?;
		info!(hold = %id, backend = record.capture.backend(), "released the held capture");
	} else {
		// Dropping is what the operator asked for, and the capture is already
		// gone; the record going with it is the outcome either way.
		warn!(
			hold = %id,
			"the capture was already gone; forgetting the hold (it was not a rollback point)"
		);
	}
	hold::remove_record(id).await?;
	Ok(())
}
