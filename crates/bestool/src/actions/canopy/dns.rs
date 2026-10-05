//! `bestool canopy dns` — publishing the addresses a DNS name resolves to.
//!
//! Canopy publishes an A record per IPv4 address and an AAAA record per IPv6
//! one, so a machine needs no access to a DNS zone of its own. Registration is
//! driven from here rather than by a periodic reconciliation: publishing
//! addresses directs traffic at a host, so it follows an operator's instruction.
//!
//! spec: NAM#commands

use std::net::SocketAddr;

use clap::{Parser, Subcommand};
use miette::Result;
use serde_json::Value;

use super::names::{ask, joined, list, tell, text};
use crate::actions::Context;

/// The DNS records canopy publishes for this server.
#[derive(Debug, Clone, Parser)]
#[clap(verbatim_doc_comment)]
pub struct DnsArgs {
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
	/// Report the DNS names canopy holds registrations for on this server.
	Show,

	/// Publish the addresses a DNS name resolves to.
	///
	/// Replaces whatever was registered for the DNS name before. The DNS name
	/// must sit within a domain the named application's group controls, and one
	/// another application already declares is refused.
	Register {
		/// The DNS name to publish records at.
		name: String,

		/// The type of the application the DNS name is for, such as
		/// `tamanu-facility` or `msupply`.
		#[arg(long = "type", short = 't', required = true)]
		application_type: String,

		/// Every external address this server is reachable at.
		#[arg(required = true)]
		addresses: Vec<std::net::IpAddr>,
	},

	/// Take a DNS name's records down and free the DNS name.
	Withdraw {
		/// The DNS name to withdraw.
		name: String,
	},
}

pub async fn run(args: DnsArgs, _ctx: Context) -> Result<()> {
	let (endpoint, query) = match args.command.clone().unwrap_or(Command::Show) {
		Command::Show => ("status", Vec::new()),
		Command::Withdraw { name } => ("dns-withdraw", vec![("name", name)]),
		Command::Register {
			name,
			application_type,
			addresses,
		} => (
			"dns-register",
			vec![
				("name", name),
				("type", application_type),
				(
					"addresses",
					addresses
						.iter()
						.map(ToString::to_string)
						.collect::<Vec<_>>()
						.join(","),
				),
			],
		),
	};

	// Registering and withdrawing change what the world resolves for this
	// server, so they go as a POST the daemon only accepts from root.
	let answer = if endpoint == "status" {
		ask(&args.server_addr, endpoint, &query).await?
	} else {
		tell(&args.server_addr, endpoint, &query).await?
	};
	if args.json {
		println!(
			"{}",
			serde_json::to_string_pretty(&answer).unwrap_or_default()
		);
		return Ok(());
	}

	match endpoint {
		"status" => show(&answer),
		_ => registered(&answer),
	}
	Ok(())
}

fn show(answer: &Value) {
	println!(
		"DNS grant: {}",
		if answer["dnsEntitled"].as_bool().unwrap_or(false) {
			"held"
		} else {
			"not held"
		}
	);
	println!("Domains: {}", joined(answer, "domains"));

	let names: Vec<String> = answer["registeredNames"]
		.as_array()
		.map(|rows| rows.iter().filter_map(Value::as_str).map(str::to_owned).collect())
		.unwrap_or_default();
	list("Names registered", &names);
}

/// Report one registration: what canopy will publish, and what it has so far.
///
/// The two differ until the zone catches up, which is why the answer carries
/// both rather than waiting.
fn registered(answer: &Value) {
	println!("{}", text(answer, "name"));
	println!("  will publish: {}", joined(answer, "addresses"));
	println!("  published:    {}", joined(answer, "publishedAddresses"));
	println!(
		"  zone caught up: {}",
		if answer["published"].as_bool().unwrap_or(false) {
			"yes"
		} else {
			"not yet"
		}
	);
	if let Some(err) = answer["lastError"].as_str() {
		println!("  last publish failed: {err}");
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn registering_needs_an_application_type_and_withdrawing_does_not() {
		assert!(DnsArgs::try_parse_from(["dns", "register", "a.example.com", "203.0.113.5"]).is_err());
		assert!(
			DnsArgs::try_parse_from([
				"dns",
				"register",
				"a.example.com",
				"--type",
				"msupply",
				"203.0.113.5"
			])
			.is_ok()
		);
		assert!(DnsArgs::try_parse_from(["dns", "withdraw", "a.example.com"]).is_ok());
	}
}
