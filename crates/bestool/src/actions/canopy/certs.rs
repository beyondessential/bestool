//! `bestool canopy certs` — what canopy holds for this server, and asking for
//! more.
//!
//! Collection is the daemon's, on a schedule: a certificate is obtained before
//! it is needed rather than while a client waits. These commands report what
//! that has produced, pre-provision a name, and run a collection without waiting
//! for the next tick.
//!
//! spec: TLS#commands

use std::net::SocketAddr;

use clap::{Parser, Subcommand};
use miette::Result;
use serde_json::Value;

use super::names::{ask, joined, list, tell, text};
use crate::actions::Context;

/// The TLS certificates canopy holds for this server.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct CertsArgs {
	#[command(subcommand)]
	command: Option<Command>,

	/// Daemon HTTP address(es) to try (defaults to [::1]:8271 and 127.0.0.1:8271)
	#[arg(long, global = true)]
	server_addr: Vec<SocketAddr>,

	/// Print the daemon's answer as JSON rather than as a report.
	#[arg(long, global = true)]
	json: bool,
}

#[derive(Debug, Clone, Subcommand)]
enum Command {
	/// Report the certificates canopy holds and the chains this host serves.
	List,

	/// Ask canopy to certify a name, without waiting for it to be discovered.
	///
	/// The name still has to be one this server's entitlement covers; this is
	/// for pre-provisioning, not for reaching past the grant.
	Request {
		/// The name to certify.
		name: String,
	},

	/// Run a collection now rather than waiting for the schedule.
	Collect,
}

pub async fn run(args: CertsArgs, _ctx: Context) -> Result<()> {
	let (endpoint, query) = match args.command.clone().unwrap_or(Command::List) {
		Command::List => ("status", Vec::new()),
		Command::Collect => ("collect", Vec::new()),
		Command::Request { name } => ("request", vec![("name", name)]),
	};

	// Requesting and collecting spend orders at the authority, so they go as a
	// POST the daemon only accepts from root.
	let answer = if endpoint == "status" {
		ask(&args.server_addr, endpoint, &query).await?
	} else {
		tell(&args.server_addr, endpoint, &query).await?
	};
	if args.json {
		println!("{}", serde_json::to_string_pretty(&answer).unwrap_or_default());
		return Ok(());
	}

	report(&answer);
	Ok(())
}

fn report(answer: &Value) {
	let entitled = answer["entitled"].as_bool().unwrap_or(false);
	let paused = answer["paused"].as_bool().unwrap_or(false);
	println!(
		"TLS grant: {}{}",
		if entitled { "held" } else { "not held" },
		if paused { " (canopy reports this server paused)" } else { "" },
	);
	println!("Domains: {}", joined(answer, "domains"));

	let held: Vec<String> = answer["held"]
		.as_array()
		.map(|rows| {
			rows.iter()
				.map(|row| {
					format!(
						"{} — expires {}{}",
						text(row, "name"),
						text(row, "notAfter"),
						if row["usable"].as_bool().unwrap_or(false) {
							""
						} else {
							" (not servable)"
						},
					)
				})
				.collect()
		})
		.unwrap_or_default();
	list("Chains this host serves", &held);

	let canopy: Vec<String> = answer["canopyHolds"]
		.as_array()
		.map(|rows| {
			rows.iter()
				.map(|row| {
					let mut line = format!("{} — expires {}", text(row, "name"), text(row, "notAfter"));
					if row["revoked"].as_bool().unwrap_or(false) {
						line.push_str(" (revoked)");
					}
					if row["keyMustBeReplaced"].as_bool().unwrap_or(false) {
						line.push_str(" (key must be replaced)");
					}
					line
				})
				.collect()
		})
		.unwrap_or_default();
	list("Certificates canopy holds", &canopy);

	// Only orders still in flight or reporting an error are worth the space: an
	// issued order is already covered by the chain above it.
	let orders: Vec<String> = answer["orders"]
		.as_array()
		.map(|rows| {
			rows.iter()
				.filter(|row| {
					row["state"].as_str() != Some("issued") || !row["lastError"].is_null()
				})
				.map(|row| {
					let mut line = format!("{} — {}", text(row, "name"), text(row, "state"));
					if let Some(err) = row["lastError"].as_str() {
						line.push_str(&format!(": {err}"));
					}
					line
				})
				.collect()
		})
		.unwrap_or_default();
	if !orders.is_empty() {
		list("Orders in flight", &orders);
	}

	if let Some(err) = answer["lastError"].as_str() {
		println!("Last pass failed: {err}");
	}
}
