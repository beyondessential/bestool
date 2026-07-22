//! Canopy enrolment healthcheck.
//!
//! Grades this host's Canopy registration so an incomplete enrolment surfaces
//! before the work that depends on it — most importantly before backups are
//! enabled, since Canopy rejects a backup snapshot that carries no device id.
//! A missing server id or device id fails; a missing device key warns (the host
//! can still authenticate over the tailscale path, but has no mTLS fallback);
//! the API URL is not required, as a registration without one uses the default
//! Canopy URL.
//!
//! spec: REG

use bestool_canopy::registration::{self, Registration};

use super::SweepContext;
use crate::doctor::check::Check;

const CHECK_NAME: &str = "canopy_registration";

pub async fn run(_ctx: SweepContext) -> Check {
	match registration::load().await {
		Ok(reg) => grade(reg.as_ref()),
		Err(err) => Check::broken(
			CHECK_NAME,
			"could not read the Canopy registration",
			err.to_string(),
		),
	}
}

/// Grade a loaded registration (or its absence) into a check outcome.
///
/// Split out from [`run`] so the state-to-outcome mapping is unit-testable
/// without touching the on-disk registration.
fn grade(reg: Option<&Registration>) -> Check {
	let Some(reg) = reg else {
		return Check::fail(
			CHECK_NAME,
			"not enrolled with Canopy",
			"no registration record on this host; run `bestool canopy register`",
		)
		.with_detail("registered", false);
	};

	let has_server_id = reg.server_id.is_some();
	let has_device_id = reg.device_id.is_some();
	let has_device_key = reg.device_key.is_some();
	let has_api_url = reg.api_url.is_some();

	// Fatal: the host can't be identified to Canopy, or its backups are rejected.
	let mut fatal: Vec<&str> = Vec::new();
	if !has_server_id {
		fatal.push("no server id, so the host cannot identify itself to Canopy");
	}
	if !has_device_id {
		fatal.push(
			"no device id, so backups are rejected until the host is re-enrolled with `bestool canopy register`",
		);
	}

	// Soft: works today, but a degraded enrolment worth flagging.
	let mut soft: Vec<&str> = Vec::new();
	if !has_device_key {
		soft.push(
			"no device key, so the host has no mTLS identity and depends on the tailscale path for authentication",
		);
	}

	let check = if !fatal.is_empty() {
		Check::fail(
			CHECK_NAME,
			format!("{} enrolment issue(s)", fatal.len() + soft.len()),
			fatal
				.iter()
				.chain(soft.iter())
				.copied()
				.collect::<Vec<_>>()
				.join("; "),
		)
	} else if !soft.is_empty() {
		Check::warning(
			CHECK_NAME,
			format!("{} enrolment issue(s)", soft.len()),
			soft.join("; "),
		)
	} else {
		Check::pass(CHECK_NAME, "enrolled with Canopy")
	};

	check
		.with_detail("registered", true)
		.with_detail("hasServerId", has_server_id)
		.with_detail("hasDeviceId", has_device_id)
		.with_detail("hasDeviceKey", has_device_key)
		.with_detail("hasApiUrl", has_api_url)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::check::CheckStatus;

	/// A fully-enrolled registration; individual tests clear the field they probe.
	fn full() -> Registration {
		Registration {
			server_id: Some("server-1".into()),
			device_key: Some("-----BEGIN PRIVATE KEY-----".into()),
			device_id: Some("device-1".into()),
			api_url: Some("https://canopy.example/".into()),
			..Registration::default()
		}
	}

	#[test]
	fn no_record_fails() {
		let check = grade(None);
		assert!(matches!(check.status, CheckStatus::Fail(_)));
		assert_eq!(check.details["registered"], false);
	}

	#[test]
	fn full_registration_passes() {
		let check = grade(Some(&full()));
		assert!(matches!(check.status, CheckStatus::Pass));
		assert_eq!(check.details["hasDeviceId"], true);
	}

	#[test]
	fn missing_device_id_fails() {
		let reg = Registration {
			device_id: None,
			..full()
		};
		let check = grade(Some(&reg));
		assert!(check.status.is_fatal());
		assert!(check.status.reason().unwrap().contains("device id"));
		assert_eq!(check.details["hasDeviceId"], false);
	}

	#[test]
	fn missing_server_id_fails() {
		let reg = Registration {
			server_id: None,
			..full()
		};
		let check = grade(Some(&reg));
		assert!(check.status.is_fatal());
		assert!(check.status.reason().unwrap().contains("server id"));
	}

	#[test]
	fn missing_device_key_warns() {
		let reg = Registration {
			device_key: None,
			..full()
		};
		let check = grade(Some(&reg));
		assert!(matches!(check.status, CheckStatus::Warning(_)));
		assert!(check.status.reason().unwrap().contains("device key"));
	}

	#[test]
	fn missing_api_url_still_passes() {
		let reg = Registration {
			api_url: None,
			..full()
		};
		let check = grade(Some(&reg));
		assert!(matches!(check.status, CheckStatus::Pass));
		assert_eq!(check.details["hasApiUrl"], false);
	}

	#[test]
	fn most_severe_outcome_wins() {
		// A missing device id (fatal) and device key (soft) together fail, and the
		// fatal reason leads while the soft reason is still carried.
		let reg = Registration {
			device_id: None,
			device_key: None,
			..full()
		};
		let check = grade(Some(&reg));
		assert!(check.status.is_fatal());
		let reason = check.status.reason().unwrap();
		assert!(reason.contains("device id"));
		assert!(reason.contains("device key"));
	}
}
