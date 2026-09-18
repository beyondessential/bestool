//! Whether this server is holding the certificates it should be getting from
//! canopy.
//!
//! What is graded is the collection, not the certificates themselves: that a
//! name this application ought to have a chain for has one, and that the chain
//! is not running down. Obtaining and serving them is the daemon's, and the
//! certificates the host actually serves are graded by `caddy_certs`.
//!
//! The check reports for an application rather than the machine. A grant, a
//! pause, and the domains a name must sit under are each an application's own,
//! and the group that answers for a failing certificate is the application's:
//! a machine may host two applications belonging to different groups, so a
//! result filed against the machine would reach the wrong people for one of
//! them.
//!
//! Canopy's entitlement answer says which application declares each name, so a
//! name is attributed from what canopy reports rather than guessed at from the
//! host. The daemon asks across every application together, because nothing here
//! ties a Caddy site to one; what it collected is still attributable, so asking
//! as the machine and reporting per application are not in tension.
//!
//! spec: CHK-CCO

use std::collections::BTreeSet;

use bestool_canopy::{certificates as certs, names::AppEntitlement, names::Entitlement};
use jiff::Timestamp;
use serde_json::{Value, json};
use tracing::debug;

use super::TamanuCx;
use crate::{Stat, check::Check, runtime::caddy};

const NAME: &str = "canopy_certificates";

/// How near expiry a collected chain may get before the collection is judged to
/// have stopped working, as a fraction of that chain's own lifetime.
///
/// Canopy re-orders on its own and the server keeps collecting, so a chain that
/// has run down this far means the collection has stopped rather than that a
/// renewal is merely in flight. A fraction rather than a duration because canopy
/// chooses the lifetime: a fixed threshold would fire far too late for a
/// short-lived chain and far too early for a long-lived one.
///
/// Set below the third of its life at which a renewal is due, so a renewal under
/// way is never mistaken for a stalled collection.
const RUNDOWN_FRACTION: f64 = 1.0 / 8.0;

pub async fn run(ctx: TamanuCx) -> Check {
	let Some(canopy) = ctx.canopy.as_deref() else {
		return Check::skip(
			NAME,
			"no canopy connectivity",
			"this sweep has no canopy client, so what this server may do could not be asked",
		);
	};

	let entitlement = match canopy.names_entitlements().await {
		Ok(wire) => Entitlement::from_wire(&wire),
		Err(err) => {
			return Check::broken(
				NAME,
				"could not ask canopy what this server may do",
				super::fmt_chain(&err),
			);
		}
	};

	// Canopy names the application each entry belongs to by its type, which is
	// what this check is running for. A machine hosting one application is
	// answered in the flat fields, and that entry is the answer.
	let Some(app) = entitlement.for_type(ctx.app.kind.type_slug()) else {
		return Check::skip(
			NAME,
			"canopy holds no entitlement for this application",
			"canopy answered with no domains or grants for this application, so it is not obtaining certificates",
		);
	};

	// A grant or a pause is an application's own, so one application skipping
	// leaves the others on the machine graded as they were. A skip closes an
	// issue the check had already opened, which is what quietens this during an
	// incident rather than adding to it.
	//
	// spec: CHK-CCO#when-it-skips
	if !app.may_manage_tls {
		return Check::skip(
			NAME,
			"no TLS grant",
			"this application is not obtaining certificates from canopy",
		);
	}
	if app.paused {
		return Check::skip(
			NAME,
			"paused in canopy",
			"this application is not obtaining certificates from canopy",
		);
	}

	let subjects = match caddy::read_active_subjects(&ctx.http).await {
		Some(subjects) => subjects,
		None => {
			return Check::skip(
				NAME,
				"caddy's configuration could not be read",
				"which names this host answers on could not be established, so what it should be collecting is unknown",
			);
		}
	};

	let chains = match certs::load_chains(&certs::default_dir()).await {
		Ok(chains) => chains,
		Err(err) => {
			return Check::broken(
				NAME,
				"could not read the collected chains",
				format!("{err}"),
			);
		}
	};

	grade(app, &subjects, &chains, Timestamp::now())
}

/// Grade one application's names: those Caddy serves that its entitlement
/// covers.
///
/// A name Caddy serves that this application's entitlement does not cover is not
/// this check's business — nothing should be collecting a chain for it.
///
/// spec: CHK-CCO#which-names-it-grades
fn grade(
	app: &AppEntitlement,
	subjects: &BTreeSet<String>,
	chains: &std::collections::BTreeMap<String, String>,
	now: Timestamp,
) -> Check {
	let graded: Vec<&String> = subjects.iter().filter(|name| app.covers(name)).collect();
	if graded.is_empty() {
		return Check::skip(
			NAME,
			"no name to collect for",
			"no name this host serves sits within the domains this application's group controls",
		);
	}

	let mut failures: Vec<String> = Vec::new();
	let mut details: Vec<Value> = Vec::new();
	let mut stats: Vec<Stat> = Vec::new();

	for name in &graded {
		let held = chains.get(name.as_str());
		let canopy_says = app.certificate(name);
		// While canopy is still retrying, it reports why the last attempt
		// failed. Surfacing it is what shows an operator why issuance is stuck
		// rather than only that nothing arrived.
		let last_error = canopy_says.and_then(|held| held.last_error_reason());

		let remaining_days = match held.and_then(|chain| certs::chain_validity(chain)) {
			None => {
				let mut reason = format!("{name}: no chain collected");
				if let Some(err) = &last_error {
					reason.push_str(&format!(" ({err})"));
				}
				failures.push(reason);
				None
			}
			Some((not_before, not_after)) => {
				let lifetime = (not_after - not_before).max(1);
				let remaining = not_after - now.as_second();
				// A renewal under way is not a failure: the chain in hand stays
				// valid until the new one lands, so a name holding a usable
				// chain passes whatever canopy is doing behind it.
				if (remaining as f64) < lifetime as f64 * RUNDOWN_FRACTION {
					let days = remaining as f64 / 86400.0;
					let mut reason =
						format!("{name}: chain expires in {days:.1}d (collection has stopped)");
					if let Some(err) = &last_error {
						reason.push_str(&format!(" ({err})"));
					}
					failures.push(reason);
				}
				Some(remaining as f64 / 86400.0)
			}
		};

		if let Some(days) = remaining_days {
			stats.push(
				Stat::gauge("days_remaining", (days * 10.0).round() / 10.0)
					.label("name", (*name).clone())
					.help("Days until a collected chain expires"),
			);
		}
		details.push(json!({
			"name": name,
			"collected": held.is_some(),
			"daysRemaining": remaining_days.map(|d| (d * 10.0).round() / 10.0),
			"canopyHolds": canopy_says.is_some(),
			"lastError": last_error,
		}));
		if last_error.is_some() {
			debug!(name = %name, "canopy reports this name's order failing");
		}
	}

	let n = graded.len();
	let check = if failures.is_empty() {
		Check::pass(NAME, format!("{n} name(s) collected"))
	} else {
		Check::fail(NAME, format!("{n} name(s) checked"), failures.join("; "))
	};
	check
		.with_detail("names", Value::Array(details))
		.with_stat(Stat::gauge("count", n as f64).help("Names graded for collection"))
		.with_stats(stats)
}

/// What canopy reported about a held certificate, where it is not simply fine.
trait HeldReason {
	fn last_error_reason(&self) -> Option<String>;
}

impl HeldReason for bestool_canopy::schema::HeldCertificate {
	fn last_error_reason(&self) -> Option<String> {
		if self.revoked {
			Some("canopy reports this certificate revoked".into())
		} else if self.key_must_be_replaced {
			Some("canopy condemned the key; a replacement is being obtained".into())
		} else if !self.usable {
			Some("canopy reports this certificate not servable".into())
		} else {
			None
		}
	}
}

#[cfg(test)]
mod tests {
	use std::collections::BTreeMap;

	use super::*;
	use crate::check::CheckStatus;

	const D: i64 = 86400;

	fn app(domains: &[&str], holds: &[&str]) -> AppEntitlement {
		AppEntitlement {
			type_slug: Some("tamanu-central".into()),
			domains: domains.iter().map(|d| (*d).to_string()).collect(),
			may_manage_dns: false,
			may_manage_tls: true,
			paused: false,
			registered_names: Vec::new(),
			certificates: holds
				.iter()
				.map(|name| {
					bestool_canopy::schema::HeldCertificate::builder()
						.key_fingerprint("ff".to_string())
						.key_must_be_replaced(false)
						.name((*name).to_string())
						.revoked(false)
						.usable(true)
						.build()
				})
				.collect(),
		}
	}

	fn subjects(names: &[&str]) -> BTreeSet<String> {
		names.iter().map(|n| (*n).to_string()).collect()
	}

	fn now() -> Timestamp {
		Timestamp::from_second(1_700_000_000).unwrap()
	}

	/// A self-signed chain whose leaf runs from `now - elapsed` to
	/// `now + remaining`, so the grading reads real dates off real bytes rather
	/// than numbers a helper made up.
	fn chain(remaining_days: i64, lifetime_days: i64) -> String {
		use rcgen::{CertificateParams, DistinguishedName};

		let key = certs::generate_key().unwrap();
		let mut params = CertificateParams::new(vec!["app.example.com".to_string()]).unwrap();
		params.distinguished_name = DistinguishedName::new();
		let not_after = now().as_second() + remaining_days * D;
		let not_before = not_after - lifetime_days * D;
		params.not_before = time::OffsetDateTime::from_unix_timestamp(not_before).unwrap();
		params.not_after = time::OffsetDateTime::from_unix_timestamp(not_after).unwrap();
		params.self_signed(&key).unwrap().pem()
	}

	fn chains(entries: &[(&str, String)]) -> BTreeMap<String, String> {
		entries
			.iter()
			.map(|(name, pem)| ((*name).to_string(), pem.clone()))
			.collect()
	}

	#[test]
	fn a_name_with_no_chain_collected_fails() {
		let check = grade(
			&app(&["example.com"], &[]),
			&subjects(&["app.example.com"]),
			&BTreeMap::new(),
			now(),
		);
		match &check.status {
			CheckStatus::Fail(reason) => assert!(reason.contains("no chain collected"), "{reason}"),
			other => panic!("expected a failure, got {other:?}"),
		}
	}

	#[test]
	fn a_name_holding_a_fresh_chain_passes_whatever_canopy_is_doing_behind_it() {
		// A renewal under way is not a failure: the chain in hand stays valid
		// until the new one lands.
		let check = grade(
			&app(&["example.com"], &["app.example.com"]),
			&subjects(&["app.example.com"]),
			&chains(&[("app.example.com", chain(60, 90))]),
			now(),
		);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	#[test]
	fn a_chain_nearer_expiry_than_renewal_should_have_allowed_fails() {
		let check = grade(
			&app(&["example.com"], &["app.example.com"]),
			&subjects(&["app.example.com"]),
			&chains(&[("app.example.com", chain(2, 90))]),
			now(),
		);
		match &check.status {
			CheckStatus::Fail(reason) => {
				assert!(reason.contains("collection has stopped"), "{reason}")
			}
			other => panic!("expected a failure, got {other:?}"),
		}
	}

	/// The threshold scales with each chain's own lifetime, since canopy chooses
	/// it: a fixed one would fire far too late for a short-lived chain and far
	/// too early for a long-lived one.
	#[test]
	fn the_rundown_threshold_scales_with_the_chains_own_lifetime() {
		// Two chains with the same time left and opposite verdicts, which is
		// what no fixed duration can produce. Two days left of six is a third of
		// the chain's life, which is when a renewal is merely due; two days left
		// of a year means nothing has been collected in months.
		let short = grade(
			&app(&["example.com"], &["app.example.com"]),
			&subjects(&["app.example.com"]),
			&chains(&[("app.example.com", chain(2, 6))]),
			now(),
		);
		assert!(matches!(short.status, CheckStatus::Pass), "{short:?}");

		let long = grade(
			&app(&["example.com"], &["app.example.com"]),
			&subjects(&["app.example.com"]),
			&chains(&[("app.example.com", chain(2, 365))]),
			now(),
		);
		assert!(matches!(long.status, CheckStatus::Fail(_)), "{long:?}");
	}

	/// A name Caddy serves that no entitlement covers is not this check's
	/// business: nothing should be collecting a chain for it.
	#[test]
	fn a_subject_the_entitlement_does_not_cover_is_not_graded() {
		let check = grade(
			&app(&["example.com"], &[]),
			&subjects(&["app.elsewhere.test"]),
			&BTreeMap::new(),
			now(),
		);
		assert!(matches!(check.status, CheckStatus::Skip(_)), "{check:?}");

		// And a covered name beside it is graded on its own.
		let check = grade(
			&app(&["example.com"], &["app.example.com"]),
			&subjects(&["app.example.com", "app.elsewhere.test"]),
			&chains(&[("app.example.com", chain(60, 90))]),
			now(),
		);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
		assert_eq!(check.details["names"].as_array().unwrap().len(), 1);
	}

	/// An operator seeing only that nothing arrived cannot tell why issuance is
	/// stuck, so what canopy said comes through with the failure.
	#[test]
	fn the_reason_canopy_gave_is_reported() {
		let mut entitlement = app(&["example.com"], &["app.example.com"]);
		entitlement.certificates[0].key_must_be_replaced = true;

		let check = grade(
			&entitlement,
			&subjects(&["app.example.com"]),
			&BTreeMap::new(),
			now(),
		);
		match &check.status {
			CheckStatus::Fail(reason) => assert!(reason.contains("condemned the key"), "{reason}"),
			other => panic!("expected a failure, got {other:?}"),
		}
	}
}
