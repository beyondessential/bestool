//! TLS certificate expiry for an application.
//!
//! An ACME issuer renews a managed cert at roughly a third of its lifetime
//! before expiry, so a cert getting close to expiry means renewal is failing —
//! exactly the kind of slow-burn problem that's invisible until the site goes
//! down.
//!
//! Which certs are in force, where they came from, and what is actually being
//! served for each are readings the substrate takes, whatever issues and serves
//! them. What is graded here is only the numbers that come back.
//!
//! Two independent signals:
//!
//! - **Expiry**: only evaluated once the cert is inside the renewal window
//!   (~1/3 of lifetime remaining), so we never alert before the issuer would
//!   even have tried. Inside the window the thresholds scale with the cert's own
//!   lifetime, anchored on the 90-day case of warn at 21 days left / fail at 7.
//!   A 45-day cert warns ~10 days out, a 6-day cert ~1.4 days out.
//! - **Served against configured**: a leaf being served that differs from the
//!   one configured means the front end has not picked up a renewal and needs a
//!   reload — a warning. Where the substrate had nothing to compare, that is not
//!   a mismatch and is not graded as one.
//!
//! A certificate collected from canopy and served to the front end is graded
//! like any other, and reported as having come from canopy: the two fail
//! differently, and a host that has quietly fallen back to the front end's own
//! issuance would otherwise present exactly as a healthy canopy-served one.
//!
//! Skips when the substrate cannot serve the reading (e.g. no front end here).
//!
//! The check keeps its name, which still says caddy: it is wire-visible and
//! canopy keys its severity map on it, so renaming it is a change to make
//! deliberately rather than in passing.
//!
//! spec: SUB#http-traffic-and-certificates

use jiff::Timestamp;
use serde_json::{Value, json};

use super::TamanuCx;
use crate::Stat;
use crate::check::Check;
use crate::runtime::Certificate;

const NAME: &str = "caddy_certs";

/// Expiry thresholds as a fraction of each cert's total lifetime, anchored on
/// the 90-day case (warn 21d, fail 7d). Scaling keeps the same safety margin
/// for shorter-lived certs (e.g. 45-day or 6-day profiles).
const WARN_FRACTION: f64 = 21.0 / 90.0;
const FAIL_FRACTION: f64 = 7.0 / 90.0;

/// Caddy/certmagic's default renewal window: a managed cert is renewed once its
/// remaining lifetime drops below this fraction of the total (1/3, i.e. at
/// two-thirds elapsed). Before that point caddy hasn't even attempted renewal,
/// so a low remaining is expected and must not alert.
const RENEWAL_RATIO: f64 = 1.0 / 3.0;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Sev {
	Warn,
	Fail,
}

#[derive(Debug, PartialEq, Eq)]
enum Expiry {
	Ok,
	Warn,
	Fail,
}

/// Classify a cert by remaining lifetime, but only once caddy would have
/// started renewing it. While `remaining` is still above the renewal window
/// (`RENEWAL_RATIO` of the total lifetime) caddy hasn't attempted renewal yet,
/// so even a modest remaining is normal and we stay `Ok`. Inside the window the
/// scaled warn/fail thresholds apply — a cert lingering there means renewal
/// isn't happening.
fn classify_expiry(remaining: i64, lifetime: i64) -> Expiry {
	let renewal_window = lifetime as f64 * RENEWAL_RATIO;
	if remaining as f64 >= renewal_window {
		return Expiry::Ok;
	}
	let warn_at = (lifetime as f64 * WARN_FRACTION) as i64;
	let fail_at = (lifetime as f64 * FAIL_FRACTION) as i64;
	if remaining <= fail_at {
		Expiry::Fail
	} else if remaining <= warn_at {
		Expiry::Warn
	} else {
		Expiry::Ok
	}
}

pub async fn run(ctx: TamanuCx) -> Check {
	let certs = match ctx.traffic.certificates().await {
		Ok(certs) => certs,
		Err(unavailable) => {
			return Check::skip(NAME, "certificates could not be read", unavailable.reason());
		}
	};

	if certs.is_empty() {
		return Check::skip(
			NAME,
			"no certificates in force",
			"the substrate reports no managed or manual certificate for this application",
		);
	}

	grade(&certs, Timestamp::now())
}

fn grade(certs: &[Certificate], now: Timestamp) -> Check {
	let mut findings: Vec<(Sev, String)> = Vec::new();
	let mut details: Vec<Value> = Vec::new();
	let mut stats: Vec<Stat> = Vec::new();

	for cert in certs {
		let label = if cert.names.is_empty() {
			cert.origin.clone()
		} else {
			cert.names.join(", ")
		};

		// Expiry, scaled to the cert's own lifetime and gated on the issuer's
		// renewal window (see `classify_expiry`).
		let lifetime = cert.lifetime_seconds();
		let remaining = cert.remaining_seconds(now);
		let days = remaining as f64 / 86400.0;
		match classify_expiry(remaining, lifetime) {
			Expiry::Fail => findings.push((
				Sev::Fail,
				format!("{label}: expires in {days:.1}d (renewal is failing)"),
			)),
			Expiry::Warn => findings.push((Sev::Warn, format!("{label}: expires in {days:.1}d"))),
			Expiry::Ok => {}
		}

		// Served against configured. `None` is nothing to compare — a wildcard
		// with no concrete name to dial, or a handshake that did not answer —
		// and must not be graded as a mismatch.
		let served_matches = cert.served_leaf.as_ref().map(|served| *served == cert.leaf);
		if served_matches == Some(false) {
			findings.push((
				Sev::Warn,
				format!(
					"{label}: served cert differs from configured cert (the front end needs a reload?)"
				),
			));
		}

		let cert_id = cert
			.names
			.first()
			.cloned()
			.unwrap_or_else(|| cert.origin.clone());
		stats.push(
			Stat::gauge("days_remaining", (days * 10.0).round() / 10.0)
				.label("cert", cert_id.clone())
				// Which side obtained it: a host that has quietly fallen back to
				// the front end's own issuance is visible here rather than
				// looking the same as one canopy is serving.
				//
				// spec: CHK-CCT#certificates-from-canopy
				.label("source", cert.source.as_str())
				.help("Days until certificate expiry"),
		);
		if let Some(matches) = served_matches {
			stats.push(
				Stat::gauge("served_matches", if matches { 1.0 } else { 0.0 })
					.label("cert", cert_id)
					.help("Served cert matches configured cert (1/0)"),
			);
		}

		details.push(json!({
			"names": cert.names,
			"source": cert.source.as_str(),
			"origin": cert.origin,
			"not_after": cert.not_after.as_second(),
			"days_remaining": (days * 10.0).round() / 10.0,
			"lifetime_days": lifetime / 86400,
			"served_matches": served_matches,
		}));
	}

	let worst = findings.iter().map(|(s, _)| *s).max();
	let reasons = findings
		.iter()
		.map(|(_, m)| m.as_str())
		.collect::<Vec<_>>()
		.join("; ");
	let n = details.len();
	let check = match worst {
		Some(Sev::Fail) => Check::fail(NAME, format!("{n} cert(s) checked"), reasons),
		Some(Sev::Warn) => Check::warning(NAME, format!("{n} cert(s) checked"), reasons),
		None => Check::pass(NAME, format!("{n} cert(s) valid")),
	};
	check
		.with_detail("certificates", Value::Array(details))
		.with_stat(Stat::gauge("count", n as f64).help("Certificates checked"))
		.with_stats(stats)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{check::CheckStatus, runtime::CertificateSource};

	fn classify(remaining: i64, lifetime: i64) -> &'static str {
		match classify_expiry(remaining, lifetime) {
			Expiry::Ok => "pass",
			Expiry::Warn => "warn",
			Expiry::Fail => "fail",
		}
	}

	const D: i64 = 86400;

	#[test]
	fn ninety_day_cert_bands() {
		let life = 90 * D;
		assert_eq!(classify(40 * D, life), "pass");
		assert_eq!(classify(20 * D, life), "warn"); // <21d
		assert_eq!(classify(6 * D, life), "fail"); // <7d
		assert_eq!(classify(-1, life), "fail"); // expired
	}

	#[test]
	fn short_lived_certs_scale() {
		// 45-day cert: warn ~10.5d, fail ~3.5d.
		let life = 45 * D;
		assert_eq!(classify(20 * D, life), "pass");
		assert_eq!(classify(9 * D, life), "warn");
		assert_eq!(classify(2 * D, life), "fail");

		// 6-day cert: warn ~1.4d, fail ~0.47d.
		let life = 6 * D;
		assert_eq!(classify(3 * D, life), "pass");
		assert_eq!(classify(D, life), "warn");
		assert_eq!(classify(D / 4, life), "fail");
	}

	#[test]
	fn manually_issued_long_lived_certs() {
		// Operator-installed certs that aren't ACME-managed can span months or a
		// year. Thresholds scale to the long lifetime, and the renewal-window
		// gate still keeps quiet until ~1/3 remains — giving plenty of human
		// lead time to reissue without nagging a year out.

		// 1-year cert: window ~122d, warn ~85d, fail ~28d.
		let life = 365 * D;
		assert_eq!(classify(200 * D, life), "pass"); // far out
		assert_eq!(classify(130 * D, life), "pass"); // still before the window
		assert_eq!(classify(100 * D, life), "pass"); // in window, above warn
		assert_eq!(classify(80 * D, life), "warn"); // <~85d
		assert_eq!(classify(20 * D, life), "fail"); // <~28d

		// 200-day cert: window ~67d, warn ~47d, fail ~16d.
		let life = 200 * D;
		assert_eq!(classify(100 * D, life), "pass"); // before window
		assert_eq!(classify(60 * D, life), "pass"); // in window, above warn
		assert_eq!(classify(40 * D, life), "warn"); // <~47d
		assert_eq!(classify(10 * D, life), "fail"); // <~16d
	}

	#[test]
	fn no_alert_before_renewal_window() {
		// Caddy renews at 1/3 remaining (30d for a 90d cert); anything above
		// that is normal and must stay OK regardless of the warn threshold.
		let life = 90 * D;
		assert_eq!(classify(60 * D, life), "pass"); // fresh
		assert_eq!(classify(31 * D, life), "pass"); // just before the window
		// Even if the warn threshold were raised past the renewal window, the
		// gate keeps us quiet until caddy has had its chance to renew.
		assert_eq!(classify_expiry(40 * D, life), Expiry::Ok);
	}

	fn cert(names: &[&str], remaining_days: i64, lifetime_days: i64) -> Certificate {
		let now = Timestamp::from_second(1_700_000_000).unwrap();
		let not_after = now.as_second() + remaining_days * D;
		Certificate {
			source: CertificateSource::FrontEnd,
			names: names.iter().map(|n| (*n).to_string()).collect(),
			origin: "/store/example.crt".into(),
			not_before: Timestamp::from_second(not_after - lifetime_days * D).unwrap(),
			not_after: Timestamp::from_second(not_after).unwrap(),
			leaf: b"configured".to_vec(),
			served_leaf: None,
		}
	}

	fn now() -> Timestamp {
		Timestamp::from_second(1_700_000_000).unwrap()
	}

	/// A host that has quietly fallen back to the front end's own issuance must
	/// be tellable from one canopy is serving: without the distinction it
	/// presents as healthy while still depending on the DNS credential that
	/// issuing through canopy exists to remove.
	///
	/// spec: CHK-CCT#certificates-from-canopy
	#[test]
	fn a_certificate_says_which_side_obtained_it() {
		let certs = [
			Certificate {
				source: CertificateSource::Canopy,
				origin: "canopy: a.example.com".into(),
				..cert(&["a.example.com"], 60, 90)
			},
			cert(&["b.example.com"], 60, 90),
		];
		let check = grade(&certs, now());
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");

		let reported = check.details["certificates"].as_array().unwrap();
		let sources: Vec<&str> = reported
			.iter()
			.map(|row| row["source"].as_str().unwrap())
			.collect();
		assert_eq!(sources, vec!["canopy", "front-end"]);
	}

	/// A front end serving something other than what is configured has not
	/// picked up a renewal.
	#[test]
	fn a_served_certificate_differing_from_the_configured_one_warns() {
		let certs = [Certificate {
			served_leaf: Some(b"something else".to_vec()),
			..cert(&["example.com"], 60, 90)
		}];
		let check = grade(&certs, now());
		match &check.status {
			CheckStatus::Warning(reason) => {
				assert!(reason.contains("served cert differs"), "{reason}")
			}
			other => panic!("expected a warning, got {other:?}"),
		}
	}

	/// Nothing to compare against is not a mismatch: a wildcard has no concrete
	/// name to dial, and a handshake that did not answer said nothing.
	#[test]
	fn a_certificate_with_nothing_served_to_compare_is_not_a_mismatch() {
		let certs = [cert(&["*.example.com"], 60, 90)];
		let check = grade(&certs, now());
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
		assert!(
			!check.stats.iter().any(|s| s.name == "served_matches"),
			"a comparison that was never made is not reported as one"
		);
	}

	/// The same certificate serving what it should is a pass, and reports the
	/// match.
	#[test]
	fn a_matching_served_certificate_passes() {
		let certs = [Certificate {
			served_leaf: Some(b"configured".to_vec()),
			..cert(&["example.com"], 60, 90)
		}];
		let check = grade(&certs, now());
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
		let stat = check
			.stats
			.iter()
			.find(|s| s.name == "served_matches")
			.expect("the comparison was made, so it is reported");
		assert_eq!(stat.value, 1.0);
	}

	/// Expiry is graded on the numbers the substrate returned, whatever issued
	/// or served them.
	#[test]
	fn a_certificate_inside_its_renewal_window_fails() {
		let certs = [cert(&["example.com"], 3, 90)];
		let check = grade(&certs, now());
		assert!(matches!(check.status, CheckStatus::Fail(_)), "{check:?}");
	}
}
