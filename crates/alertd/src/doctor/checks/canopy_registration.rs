//! Canopy enrolment healthcheck.
//!
//! Grades this host's Canopy registration so an older registration format
//! surfaces before the work that depends on it — most relevantly the backups of
//! a deployment that has Canopy backups configured, which tag a snapshot with
//! the device id. A missing server id or device id fails, and updating bestool
//! usually migrates an older format to the current one; a missing device key
//! warns (the host authenticates over the tailscale path rather than by mTLS);
//! the API URL is not required, as a registration without one uses the default
//! Canopy URL.
//!
//! spec: REG

use bestool_canopy::{
	CanopyHttpError,
	registration::{self, Registration},
};
use tracing::{debug, info, warn};

use super::SweepContext;
use crate::doctor::{check::Check, heal::HealOutcome};

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

/// Recover a missing server id or device id from Canopy and write it back into
/// the registration, so a later sweep sees a complete enrolment and passes.
///
/// spec: REG#recovering-a-missing-identity
pub async fn heal(ctx: SweepContext) -> HealOutcome {
	let Some(canopy) = ctx.canopy.as_deref() else {
		// No canopy connectivity to recover from on this sweep.
		return HealOutcome::Deferred;
	};

	let reg = match registration::load().await {
		Ok(Some(reg)) => reg,
		// Nothing enrolled to complete: recovering a full identity from scratch
		// is `bestool canopy register`'s job, not the healer's.
		Ok(None) => return HealOutcome::Deferred,
		Err(err) => {
			warn!(%err, "canopy_registration heal: could not read the registration");
			return HealOutcome::Failed;
		}
	};

	if reg.server_id.is_some() && reg.device_id.is_some() {
		// Both identifiers already present; the check fails for some other
		// reason the healer can't address.
		return HealOutcome::Deferred;
	}

	// `GET /servers/self` resolves the caller from its tailnet identity or mTLS
	// certificate and returns the pair assigned at enrolment (canopy spec DID).
	match canopy.servers_self().await {
		Ok(identity) => {
			let recovered = merge_identity(
				reg,
				&identity.server_id.to_string(),
				&identity.device_id.to_string(),
			);
			match registration::store(&recovered).await {
				Ok(()) => {
					info!("recovered Canopy identity from GET /servers/self");
					HealOutcome::Healed
				}
				Err(err) => {
					warn!(%err, "canopy_registration heal: could not store the recovered identity");
					HealOutcome::Failed
				}
			}
		}
		Err(err) => {
			// A recognised HTTP status means Canopy answered but had no identity
			// to give: unknown device, not yet attached, or attached to several.
			// Back off and retry later.
			if let Some(http) = err.downcast_ref::<CanopyHttpError>() {
				debug!(status = %http.status, "canopy_registration heal: /servers/self gave no identity");
				HealOutcome::Deferred
			} else {
				warn!(%err, "canopy_registration heal: /servers/self request failed");
				HealOutcome::Failed
			}
		}
	}
}

/// Fill a missing server id or device id from the recovered pair, leaving any
/// value already present — and the device key and API URL — untouched.
fn merge_identity(mut reg: Registration, server_id: &str, device_id: &str) -> Registration {
	if reg.server_id.is_none() {
		reg.server_id = Some(server_id.to_owned());
	}
	if reg.device_id.is_none() {
		reg.device_id = Some(device_id.to_owned());
	}
	reg
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

	// Fatal: an older registration format that the current bestool auto-migrates.
	let mut fatal: Vec<&str> = Vec::new();
	if !has_server_id {
		fatal.push("no server id, so this host is on an older Canopy registration format; updating bestool usually migrates it");
	}
	if !has_device_id {
		fatal.push(
			"no device id, so this host is on an older Canopy registration format; updating bestool usually migrates it (a manual `bestool canopy register` is only needed if that doesn't resolve it), and this affects backups only where Canopy backups are configured",
		);
	}

	// Soft: works today, but a registration detail worth flagging.
	let mut soft: Vec<&str> = Vec::new();
	if !has_device_key {
		soft.push(
			"no device key, so this host authenticates to Canopy over the tailscale path rather than by mTLS",
		);
	}

	let check = if !fatal.is_empty() {
		Check::fail(
			CHECK_NAME,
			format!("{} Canopy registration note(s)", fatal.len() + soft.len()),
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
			format!("{} Canopy registration note(s)", soft.len()),
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
	fn merge_fills_only_the_missing_identifier() {
		// A registration missing just the device id: recovery fills the device
		// id and leaves the present server id, device key, and API URL as they
		// were.
		let reg = Registration {
			device_id: None,
			..full()
		};
		let merged = merge_identity(reg, "recovered-server", "recovered-device");
		assert_eq!(merged.server_id.as_deref(), Some("server-1"));
		assert_eq!(merged.device_id.as_deref(), Some("recovered-device"));
		assert_eq!(
			merged.device_key.as_deref(),
			Some("-----BEGIN PRIVATE KEY-----")
		);
		assert_eq!(merged.api_url.as_deref(), Some("https://canopy.example/"));
	}

	#[test]
	fn merge_fills_a_missing_server_id() {
		let reg = Registration {
			server_id: None,
			..full()
		};
		let merged = merge_identity(reg, "recovered-server", "recovered-device");
		// The device id was present, so it is left; the server id is filled.
		assert_eq!(merged.server_id.as_deref(), Some("recovered-server"));
		assert_eq!(merged.device_id.as_deref(), Some("device-1"));
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
