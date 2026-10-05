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
//! Which application a DNS name belongs to is the shared ownership function's
//! answer, from the Caddy site that serves it and then from what canopy says the
//! application declares. The daemon orders for every application together, and
//! what it collected is attributable, so asking as the machine and reporting per
//! application are not in tension.
//!
//! A name canopy refused as undeclared or denied is not this host's to report,
//! so it is listed and left ungraded; the daemon says which are which.
//!
//! spec: CHK-CCO

use std::collections::{BTreeMap, BTreeSet};

use bestool_canopy::names::{AppEntitlement, Refusal, RefusalKind};
use jiff::Timestamp;
use serde_json::{Value, json};
use tracing::debug;

use super::HostedCx;
use crate::{
	Stat,
	check::Check,
	ownership::{Ownership, host_covers},
	sweep_cache::DaemonStatus,
};

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

pub async fn run(ctx: HostedCx) -> Check {
	let Some(canopy) = ctx.canopy.as_deref() else {
		return Check::skip(
			NAME,
			"no canopy connectivity",
			"this sweep has no canopy client, so what this server may do could not be asked",
		);
	};

	// Asked once for the machine and shared: the answer covers every application
	// on it, and this check runs once per application.
	let entitlement = match ctx.sweep.entitlement(canopy).await {
		Ok(entitlement) => entitlement,
		Err(err) => {
			return Check::broken(NAME, "could not ask canopy what this server may do", err);
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

	let subjects = match ctx.sweep.caddy_subjects().await {
		Some(subjects) => subjects,
		None => {
			return Check::skip(
				NAME,
				"caddy's configuration could not be read",
				"which names this host answers on could not be established, so what it should be collecting is unknown",
			);
		}
	};

	let Some(ownership) = ctx.sweep.ownership(Some(canopy)).await else {
		return Check::skip(
			NAME,
			"caddy's configuration could not be read",
			"which application each DNS name belongs to could not be established, so what it should be collecting is unknown",
		);
	};
	let owned = owned_names(&subjects, &ownership, ctx.app.kind.type_slug());

	if let Err(err) = ctx.sweep.canopy_chains().await {
		return Check::broken(NAME, "could not read the collected chains", err);
	}

	// Asked once for the machine and shared, like the entitlement above.
	let daemon = ctx.sweep.daemon_status().await;

	// Parsed once for the sweep, on the blocking pool, rather than decoded here
	// per name per application.
	let validity = ctx.sweep.canopy_chain_validity().await;
	grade(
		app,
		&owned,
		&validity,
		daemon.as_deref().map_err(String::as_str),
		Timestamp::now(),
	)
}

/// The certified DNS names that belong to the application of type `type_slug`.
///
/// spec: CHK-CCO#which-dns-names-it-grades
fn owned_names(
	certified: &BTreeSet<String>,
	ownership: &Ownership,
	type_slug: &str,
) -> BTreeSet<String> {
	certified
		.iter()
		.filter(|name| ownership.owner(name) == Some(type_slug))
		.cloned()
		.collect()
}

/// Whether the application's entitlement lists a declared name covering `name`.
fn declares(app: &AppEntitlement, name: &str) -> bool {
	app.registered_names
		.iter()
		.any(|declared| host_covers(&declared.to_ascii_lowercase(), name))
}

/// Grade one application's DNS names: the certified ones it owns, that its
/// entitlement covers.
///
/// A name this application's entitlement does not cover is not this check's
/// business — nothing should be collecting a chain for it. Where the daemon
/// reported Canopy refusing a name as undeclared or denied, the name is listed
/// and left ungraded; where the daemon could not be asked, only the names the
/// application declares are graded, since any other may be waiting on an
/// operator and nothing here can tell.
///
/// spec: CHK-CCO#which-dns-names-it-grades
fn grade(
	app: &AppEntitlement,
	owned: &BTreeSet<String>,
	validity: &BTreeMap<String, (i64, i64)>,
	daemon: Result<&DaemonStatus, &str>,
	now: Timestamp,
) -> Check {
	let mut graded: Vec<&String> = Vec::new();
	let mut undeclared: Vec<Value> = Vec::new();
	let mut denied: Vec<Value> = Vec::new();
	for name in owned.iter().filter(|name| app.covers(name)) {
		match daemon {
			Ok(status) => match status.refusal(name) {
				Some(Refusal { kind, reason }) if *kind == RefusalKind::Undeclared => {
					undeclared.push(json!({"name": name, "reason": reason}));
				}
				Some(Refusal { kind, reason }) if *kind == RefusalKind::Denied => {
					denied.push(json!({"name": name, "reason": reason}));
				}
				_ => graded.push(name),
			},
			Err(_) if declares(app, name) => graded.push(name),
			Err(_) => {}
		}
	}

	let annotate = |check: Check| {
		let mut check = check;
		if !undeclared.is_empty() {
			check = check.with_detail("undeclared", Value::Array(undeclared.clone()));
		}
		if !denied.is_empty() {
			check = check.with_detail("denied", Value::Array(denied.clone()));
		}
		if let Err(err) = daemon {
			check = check.with_detail(
				"daemon",
				json!({"asked": false, "error": err, "note": "only names this application declares were graded"}),
			);
		}
		check
	};

	if graded.is_empty() {
		return annotate(Check::skip(
			NAME,
			"no name to collect for",
			"no DNS name this host serves sits within the domains this application's group controls and is one it is known to need a chain for",
		));
	}

	let mut failures: Vec<String> = Vec::new();
	let mut details: Vec<Value> = Vec::new();
	let mut stats: Vec<Stat> = Vec::new();

	for name in &graded {
		let held = validity.get(name.as_str()).copied();
		let canopy_says = app.certificate(name);
		// A refusal other than undeclared or denied, such as a type mismatch, is a
		// fault on this host's side of the request.
		let refusal = daemon
			.ok()
			.and_then(|status| status.refusal(name))
			.filter(|refusal| refusal.kind == RefusalKind::Other);
		// While canopy is still retrying, it reports why the last attempt
		// failed. Surfacing it is what shows an operator why issuance is stuck
		// rather than only that nothing arrived.
		let last_error = canopy_says.and_then(|held| held.last_error_reason());
		let why = last_error
			.iter()
			.cloned()
			.chain(refusal.map(|refusal| refusal.reason.clone()))
			.collect::<Vec<_>>()
			.join("; ");
		let why = (!why.is_empty()).then_some(why);

		let remaining_days = match held {
			None => {
				let mut reason = format!("{name}: no chain collected");
				if let Some(why) = &why {
					reason.push_str(&format!(" ({why})"));
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
					if let Some(why) = &why {
						reason.push_str(&format!(" ({why})"));
					}
					failures.push(reason);
				} else if let Some(refusal) = refusal {
					failures.push(format!(
						"{name}: canopy refused the last request ({})",
						refusal.reason
					));
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
			"lastError": why,
		}));
		if why.is_some() {
			debug!(name = %name, "canopy reports this name's order failing");
		}
	}

	let n = graded.len();
	let check = if failures.is_empty() {
		Check::pass(NAME, format!("{n} name(s) collected"))
	} else {
		Check::fail(NAME, format!("{n} name(s) checked"), failures.join("; "))
	};
	annotate(check)
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
	use bestool_canopy::{certificates as certs, names::Entitlement};

	use super::*;
	use crate::{
		check::CheckStatus,
		ownership::{CaddySites, HostApplication},
	};

	const D: i64 = 86400;

	/// Grading with a daemon that has refused nothing.
	fn grade(
		app: &AppEntitlement,
		owned: &BTreeSet<String>,
		validity: &BTreeMap<String, (i64, i64)>,
		now: Timestamp,
	) -> Check {
		super::grade(app, owned, validity, Ok(&DaemonStatus::default()), now)
	}

	fn daemon(orders: Value) -> DaemonStatus {
		DaemonStatus::from_json(&json!({ "orders": orders }))
	}

	fn refused(name: &str, refusal: &str, reason: &str) -> Value {
		json!({"name": name, "state": "refused", "refusal": refusal, "reason": reason})
	}

	fn listed(check: &Check, key: &str) -> Vec<String> {
		check
			.details
			.get(key)
			.and_then(Value::as_array)
			.into_iter()
			.flatten()
			.map(|row| row["name"].as_str().unwrap().to_owned())
			.collect()
	}

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

	/// What the sweep's cache would have parsed off these chains.
	fn chains(entries: &[(&str, String)]) -> BTreeMap<String, (i64, i64)> {
		entries
			.iter()
			.map(|(name, pem)| {
				(
					(*name).to_string(),
					certs::chain_validity(pem).expect("the test chains are real certificates"),
				)
			})
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

	fn host(type_slug: &str, service: &str) -> HostApplication {
		HostApplication {
			type_slug: type_slug.into(),
			canonical_hosts: BTreeSet::new(),
			service_names: [service.to_owned()].into(),
			shared_names: BTreeSet::new(),
			local_ports: BTreeSet::new(),
		}
	}

	fn proxying(host: &str, dial: &str) -> Value {
		json!({
			"match": [{"host": [host]}],
			"handle": [{"handler": "reverse_proxy", "upstreams": [{"dial": dial}]}],
		})
	}

	/// A host fronting Tamanu and mSupply, with a site that proxies to neither.
	fn two_applications() -> Ownership {
		let config = json!({"apps": {"http": {"servers": {"s": {"routes": [
			proxying("central.example.com", "api.central.tamanu.internal:80"),
			proxying("supply.example.com", "api.msupply.internal:80"),
			proxying("stray.example.com", "something.else.internal:80"),
		]}}}}});
		Ownership::resolve(
			&CaddySites::from_config(&config),
			&[
				host("tamanu-central", "api.central.tamanu.internal"),
				host("msupply", "api.msupply.internal"),
			],
			&Entitlement::default(),
		)
	}

	/// On a host with Tamanu and mSupply, each application's run grades only the
	/// DNS names belonging to it.
	///
	/// spec: CHK-CCO#which-dns-names-it-grades
	#[test]
	fn each_application_grades_only_the_dns_names_belonging_to_it() {
		let ownership = two_applications();
		let certified = subjects(&[
			"central.example.com",
			"supply.example.com",
			"stray.example.com",
		]);

		assert_eq!(
			owned_names(&certified, &ownership, "tamanu-central"),
			subjects(&["central.example.com"])
		);
		assert_eq!(
			owned_names(&certified, &ownership, "msupply"),
			subjects(&["supply.example.com"])
		);
	}

	/// A type mismatch on an mSupply DNS name fails the mSupply run and leaves
	/// the Tamanu run as it was.
	///
	/// spec: CHK-CCO#outcomes
	#[test]
	fn a_type_mismatch_fails_only_the_applications_run_owning_the_name() {
		let ownership = two_applications();
		let certified = subjects(&["central.example.com", "supply.example.com"]);
		let status = daemon(json!([refused(
			"supply.example.com",
			"other",
			"this machine hosts tamanu-central"
		)]));
		let validity = chains(&[
			("central.example.com", chain(60, 90)),
			("supply.example.com", chain(60, 90)),
		]);
		let entitlement = app(
			&["example.com"],
			&["central.example.com", "supply.example.com"],
		);

		let tamanu = super::grade(
			&entitlement,
			&owned_names(&certified, &ownership, "tamanu-central"),
			&validity,
			Ok(&status),
			now(),
		);
		assert!(matches!(tamanu.status, CheckStatus::Pass), "{tamanu:?}");

		let msupply = super::grade(
			&entitlement,
			&owned_names(&certified, &ownership, "msupply"),
			&validity,
			Ok(&status),
			now(),
		);
		match &msupply.status {
			CheckStatus::Fail(reason) => {
				assert!(reason.contains("supply.example.com"), "{reason}");
				assert!(
					reason.contains("this machine hosts tamanu-central"),
					"{reason}"
				);
			}
			other => panic!("expected a failure, got {other:?}"),
		}
	}

	/// The reason for the refusal rides along with a name that has no chain.
	#[test]
	fn a_refusal_other_than_undeclared_or_denied_explains_a_missing_chain() {
		let status = daemon(json!([refused(
			"app.example.com",
			"other",
			"this machine hosts msupply"
		)]));
		let check = super::grade(
			&app(&["example.com"], &[]),
			&subjects(&["app.example.com"]),
			&BTreeMap::new(),
			Ok(&status),
			now(),
		);
		match &check.status {
			CheckStatus::Fail(reason) => {
				assert!(reason.contains("no chain collected"), "{reason}");
				assert!(reason.contains("this machine hosts msupply"), "{reason}");
			}
			other => panic!("expected a failure, got {other:?}"),
		}
	}

	/// An undeclared name with no chain is listed, and the outcome and summary
	/// are what they were without it.
	///
	/// spec: CHK-CCO#outcomes
	#[test]
	fn an_undeclared_name_is_listed_and_changes_neither_outcome_nor_summary() {
		let validity = chains(&[("app.example.com", chain(60, 90))]);
		let entitlement = app(&["example.com"], &["app.example.com"]);
		let status = daemon(json!([refused(
			"wait.example.com",
			"undeclared",
			"needs declaring"
		)]));

		let with = super::grade(
			&entitlement,
			&subjects(&["app.example.com", "wait.example.com"]),
			&validity,
			Ok(&status),
			now(),
		);
		let without = grade(
			&entitlement,
			&subjects(&["app.example.com"]),
			&validity,
			now(),
		);

		assert!(matches!(with.status, CheckStatus::Pass), "{with:?}");
		assert_eq!(with.summary, without.summary);
		assert_eq!(listed(&with, "undeclared"), vec!["wait.example.com"]);
		assert_eq!(
			with.details["undeclared"][0]["reason"],
			json!("needs declaring")
		);
		assert_eq!(listed(&with, "names"), vec!["app.example.com"]);
	}

	/// A denial is an operator's decision against the name, whether or not a
	/// chain is still held for it.
	///
	/// spec: CHK-CCO#outcomes
	#[test]
	fn a_denied_name_is_listed_whether_or_not_a_chain_is_held() {
		let status = daemon(json!([refused("no.example.com", "denied", "denied")]));
		let entitlement = app(&["example.com"], &["app.example.com"]);
		let names = subjects(&["app.example.com", "no.example.com"]);

		for validity in [
			chains(&[("app.example.com", chain(60, 90))]),
			chains(&[
				("app.example.com", chain(60, 90)),
				("no.example.com", chain(60, 90)),
			]),
			chains(&[
				("app.example.com", chain(60, 90)),
				("no.example.com", chain(1, 90)),
			]),
		] {
			let check = super::grade(&entitlement, &names, &validity, Ok(&status), now());
			assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
			assert_eq!(check.summary, "1 name(s) collected");
			assert_eq!(listed(&check, "denied"), vec!["no.example.com"]);
			assert!(listed(&check, "undeclared").is_empty());
		}
	}

	/// An undeclared name does not shield a name beside it that has no chain.
	///
	/// spec: CHK-CCO#outcomes
	#[test]
	fn an_undeclared_name_beside_a_name_with_no_chain_fails_on_the_other_only() {
		let status = daemon(json!([refused(
			"wait.example.com",
			"undeclared",
			"needs declaring"
		)]));
		let check = super::grade(
			&app(&["example.com"], &[]),
			&subjects(&["app.example.com", "wait.example.com"]),
			&BTreeMap::new(),
			Ok(&status),
			now(),
		);
		match &check.status {
			CheckStatus::Fail(reason) => {
				assert!(
					reason.contains("app.example.com: no chain collected"),
					"{reason}"
				);
				assert!(!reason.contains("wait.example.com"), "{reason}");
			}
			other => panic!("expected a failure, got {other:?}"),
		}
		assert_eq!(listed(&check, "undeclared"), vec!["wait.example.com"]);
		assert_eq!(listed(&check, "names"), vec!["app.example.com"]);
	}

	/// With the daemon out of reach a name that may be waiting on an operator is
	/// not graded, and the detail says why.
	///
	/// spec: CHK-CCO#which-dns-names-it-grades
	#[test]
	fn with_the_daemon_unreachable_only_declared_names_are_graded() {
		let mut entitlement = app(&["example.com"], &[]);
		entitlement.registered_names = vec!["Declared.example.com".into()];

		let check = super::grade(
			&entitlement,
			&subjects(&["declared.example.com", "other.example.com"]),
			&BTreeMap::new(),
			Err("no daemon answered"),
			now(),
		);
		match &check.status {
			CheckStatus::Fail(reason) => {
				assert!(
					reason.contains("declared.example.com: no chain"),
					"{reason}"
				);
				assert!(!reason.contains("other.example.com"), "{reason}");
			}
			other => panic!("expected a failure, got {other:?}"),
		}
		assert_eq!(listed(&check, "names"), vec!["declared.example.com"]);
		assert_eq!(check.details["daemon"]["asked"], json!(false));

		let none_declared = super::grade(
			&app(&["example.com"], &[]),
			&subjects(&["other.example.com"]),
			&BTreeMap::new(),
			Err("no daemon answered"),
			now(),
		);
		assert!(matches!(none_declared.status, CheckStatus::Skip(_)));
		assert_eq!(none_declared.details["daemon"]["asked"], json!(false));
	}
}
