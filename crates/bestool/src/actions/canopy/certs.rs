//! `bestool canopy certs` — what canopy holds for this server, and asking for
//! more.
//!
//! Collection is the daemon's, on a schedule: a certificate is obtained before
//! it is needed rather than while a client waits. These commands report what
//! that has produced, pre-provision a DNS name, and run a collection without
//! waiting for the next tick.
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

	/// Ask canopy to certify a DNS name, without waiting for it to be discovered.
	///
	/// The DNS name still has to be one the named application's entitlement
	/// covers; this is for pre-provisioning, not for reaching past the grant.
	Request {
		/// The DNS name to certify.
		name: String,

		/// The type of the application the DNS name is for, such as
		/// `tamanu-facility` or `msupply`.
		///
		/// A DNS name requested ahead of its site cannot be attributed from
		/// Caddy's configuration, so the application is named here.
		#[arg(long = "type", short = 't', required = true)]
		application_type: String,
	},

	/// Run a collection now rather than waiting for the schedule.
	Collect,
}

pub async fn run(args: CertsArgs, _ctx: Context) -> Result<()> {
	let (endpoint, query) = match args.command.clone().unwrap_or(Command::List) {
		Command::List => ("status", Vec::new()),
		Command::Collect => ("collect", Vec::new()),
		Command::Request {
			name,
			application_type,
		} => (
			"request",
			vec![("name", name), ("type", application_type)],
		),
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

	// Only orders still in flight, refused or reporting an error are worth the
	// space: an issued order is already covered by the chain above it.
	let orders: Vec<String> = answer["orders"]
		.as_array()
		.map(|rows| {
			rows.iter()
				.filter(|row| row["state"].as_str() != Some("issued") || !row["reason"].is_null())
				.map(order_line)
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

/// One order: the DNS name, what state it is in, the application type the
/// request carried, and the reason canopy gave where it gave one.
///
/// A request that never reached an answer has no state of canopy's, and is
/// reported as failed rather than with an empty one.
///
/// spec: TLS#commands
fn order_line(row: &Value) -> String {
	let state = row["state"]
		.as_str()
		.filter(|state| !state.is_empty())
		.unwrap_or("failed");
	let mut line = format!("{} — {state}", text(row, "name"));
	if let Some(application_type) = row["applicationType"].as_str() {
		line.push_str(&format!(" (as {application_type})"));
	}
	if let Some(reason) = row["reason"].as_str() {
		line.push_str(&format!(": {reason}"));
	}
	line
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	#[test]
	fn a_refusal_shows_as_its_state_with_canopys_reason_and_the_type_sent() {
		let line = order_line(&json!({
			"name": "app.example.com",
			"state": "undeclared",
			"applicationType": "msupply",
			"reason": "declare app.example.com on an application",
		}));
		assert_eq!(
			line,
			"app.example.com — undeclared (as msupply): declare app.example.com on an application"
		);
	}

	#[test]
	fn an_order_with_no_state_is_failed_not_blank() {
		let line = order_line(&json!({
			"name": "app.example.com",
			"state": "",
			"reason": "reaching canopy: timed out",
		}));
		assert_eq!(line, "app.example.com — failed: reaching canopy: timed out");
	}

	#[test]
	fn requesting_needs_an_application_type() {
		use clap::Parser as _;
		assert!(CertsArgs::try_parse_from(["certs", "request", "app.example.com"]).is_err());
		assert!(
			CertsArgs::try_parse_from(["certs", "request", "app.example.com", "--type", "msupply"])
				.is_ok()
		);
	}
}
