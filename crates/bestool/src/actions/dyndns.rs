use std::{collections::BTreeMap, mem::take};

use aws_sdk_route53::{
	operation::get_hosted_zone::GetHostedZoneOutput,
	types::{Change, ChangeBatch, HostedZone, ResourceRecord, ResourceRecordSet, RrType},
};
use clap::Parser;

use ip_network::IpNetwork;
use local_ip_address::list_afinet_netifas;
use miette::{bail, IntoDiagnostic, Result};
use regex::Regex;
use tracing::debug;

use crate::aws::{self, AwsArgs};

use super::Context;

/// Update a DNS record in Route53 with the current IP address.
///
/// This is specifically made for Tamanu Iti, so it reads private IP address(es), not public ones.
#[derive(Debug, Clone, Parser)]
pub struct DyndnsArgs {
	/// Interfaces to query for IP addresses.
	///
	/// This is in priority order: the first interface with a global scope IP address is used.
	/// Regexes are supported.
	///
	/// If none are provided, the default is to use `eth*`, `enp*`, and `wlan*` interfaces first,
	/// in this order, then any other.
	#[arg(long, value_name = "INTERFACE")]
	pub interfaces: Vec<Regex>,

	/// The domain name to update.
	#[arg(long, value_name = "DOMAIN")]
	pub domain: String,

	/// The Route53 zone ID for that domain.
	#[arg(long, value_name = "ZONE_ID")]
	pub zone_id: String,

	/// Only use IPv4 addresses.
	///
	/// By default both IPv4 and IPv6 addresses are used. If that causes issues, you may want to
	/// disable it with this.
	#[arg(long, short = '4')]
	pub ipv4_only: bool,

	/// Don't make any changes, just print what would be done.
	#[arg(long, short = 'n')]
	pub dry_run: bool,

	#[command(flatten)]
	pub aws: AwsArgs,
}

pub async fn run(mut ctx: Context<DyndnsArgs>) -> Result<()> {
	let local_ips_by_interface: BTreeMap<String, Vec<IpNetwork>> = list_afinet_netifas()
		.into_diagnostic()?
		.into_iter()
		.filter_map(|(iface, ip)| {
			let ip = IpNetwork::from(ip);
			if ip.is_ipv4() || ip.is_global() {
				Some((iface, ip))
			} else {
				None
			}
		})
		.fold(BTreeMap::new(), |mut map, (iface, ip)| {
			map.entry(iface).or_default().push(ip);
			map
		});
	debug!(?local_ips_by_interface, "fetched local routable IPs");

	let iface_rxs: Vec<Regex> = if ctx.args_top.interfaces.is_empty() {
		["^eth.*", "^enp.*", "^wlan.*"]
			.map(|pattern| Regex::new(pattern).unwrap())
			.to_vec()
	} else {
		take(&mut ctx.args_top.interfaces)
	};

	let mut ips = None;
	'outer: for rx in iface_rxs {
		for (iface, these_ips) in &local_ips_by_interface {
			debug!(interface=%iface, pattern=%rx, "checking interface against pattern");
			if rx.is_match(iface) {
				ips.replace(these_ips);
				break 'outer;
			}
		}
	}

	let Some(ips) = ips else {
		bail!("no interfaces matched");
	};
	debug!(?ips, "got desired local IPs");

	let DyndnsArgs {
		domain,
		dry_run,
		zone_id,
		ipv4_only,
		..
	} = &ctx.args_top;
	let aws = aws::init(&ctx.args_top.aws).await;
	let client = aws_sdk_route53::Client::new(&aws);

	let GetHostedZoneOutput {
		hosted_zone: Some(HostedZone {
			name: hosted_zone, ..
		}),
		..
	} = client
		.get_hosted_zone()
		.id(zone_id)
		.send()
		.await
		.into_diagnostic()?
	else {
		bail!("couldn't find zone with ID {zone_id}");
	};

	let fqdn = if !domain.ends_with('.') {
		format!("{domain}.")
	} else {
		domain.into()
	};
	if !fqdn.ends_with(&hosted_zone) {
		bail!("domain {fqdn} doesn't match with zone's name {hosted_zone}");
	}

	let record_sets = client
		.list_resource_record_sets()
		.hosted_zone_id(zone_id)
		.start_record_name(domain)
		.send()
		.await
		.into_diagnostic()?;

	let mut change_batch =
		ChangeBatch::builder().comment(format!("{} dyndns update for {fqdn}", crate::APP_NAME));

	for set in record_sets.resource_record_sets() {
		if set.name() != fqdn {
			continue;
		}

		if let RrType::A | RrType::Aaaa = set.r#type() {
			println!(
				"{domain}: delete {} record: {}",
				match set.r#type {
					RrType::A => "A",
					RrType::Aaaa => "AAAA",
					_ => unreachable!(),
				},
				set.resource_records()
					.iter()
					.map(|record| record.value.as_str())
					.collect::<Vec<_>>()
					.join(", ")
			);

			change_batch = change_batch.changes(
				Change::builder()
					.action(aws_sdk_route53::types::ChangeAction::Delete)
					.resource_record_set(set.clone())
					.build()
					.into_diagnostic()?,
			);
		}
	}

	for ip in ips {
		match ip {
			IpNetwork::V4(ipv4) => {
				let addr = ipv4.network_address().to_string();
				println!("{domain}: create A record: {addr}");
				change_batch = change_batch.changes(
					Change::builder()
						.action(aws_sdk_route53::types::ChangeAction::Create)
						.resource_record_set(
							ResourceRecordSet::builder()
								.r#type(RrType::A)
								.name(domain)
								.ttl(5)
								.resource_records(
									ResourceRecord::builder()
										.value(addr)
										.build()
										.into_diagnostic()?,
								)
								.build()
								.into_diagnostic()?,
						)
						.build()
						.into_diagnostic()?,
				);
			}
			IpNetwork::V6(ipv6) => {
				if *ipv4_only {
					continue;
				}

				let addr = ipv6.network_address().to_string();
				println!("{domain}: create AAAA record: {addr}");
				change_batch = change_batch.changes(
					Change::builder()
						.action(aws_sdk_route53::types::ChangeAction::Create)
						.resource_record_set(
							ResourceRecordSet::builder()
								.r#type(RrType::Aaaa)
								.name(domain)
								.ttl(5)
								.resource_records(
									ResourceRecord::builder()
										.value(addr)
										.build()
										.into_diagnostic()?,
								)
								.build()
								.into_diagnostic()?,
						)
						.build()
						.into_diagnostic()?,
				);
			}
		}
	}

	// TODO: deduplicate changes, or at least do nothing if we delete + create the same records
	let change_batch = change_batch.build().into_diagnostic()?;

	if *dry_run {
		println!("dry run, not making changes");
		return Ok(());
	}

	client
		.change_resource_record_sets()
		.hosted_zone_id(zone_id)
		.change_batch(change_batch)
		.send()
		.await
		.into_diagnostic()?;

	println!("done");
	Ok(())
}
