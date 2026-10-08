//! Facility servers whose sync has gone stale.
//!
//! Sync runs about every 60s, so for each device that has synced in the last
//! 48h we compute the minutes since its last successful (errorless, completed)
//! sync and tier: WARN past 10 minutes, FAIL past 30. The 48h-active guard
//! keeps decommissioned servers from flagging.
//!
//! A facility server is the device running it, not the facility it serves: one
//! device can serve several facilities, and a replacement server has a new
//! device id. Each device is one instance, so one known to be offline can be
//! silenced in Canopy without quieting the rest.
//!
//! spec: CHK-SFS

use super::{TamanuCx, query_error_check};
use crate::Stat;
use crate::check::{Check, Instance};

const NAME: &str = "sync_facility_stale";

const WARN_MINUTES: f64 = 10.0;
const FAIL_MINUTES: f64 = 30.0;

// The 30-day bound keeps the jsonb expansion off the full table (millions of
// rows on long-lived centrals). It cannot change any tier: staleness saturates
// at FAIL past 30 minutes, so a last success older than 30 days grades the
// same as none at all — only the reported timestamp saturates at the window.
// A session naming no device is not measured: every sync client sends one.
//
// A device's facilities are those of its most recent session, and a facility
// with no record is named by its id.
const SQL: &str = "WITH device_sessions AS ( \
		SELECT parameters->>'deviceId' AS device_id, \
			CASE WHEN jsonb_typeof(parameters->'facilityIds') = 'array' \
				THEN parameters->'facilityIds' ELSE '[]'::jsonb END AS facility_ids, \
			created_at, completed_at, errors \
		FROM sync_sessions WHERE parameters->>'isMobile' IS DISTINCT FROM 'true' \
			AND parameters->>'deviceId' IS NOT NULL \
			AND created_at > now() - interval '30 days' \
	), active AS ( \
		SELECT DISTINCT ON (device_id) device_id, facility_ids FROM device_sessions \
		WHERE created_at > now() - interval '48 hours' \
		ORDER BY device_id, created_at DESC \
	), last_success AS ( \
		SELECT device_id, max(completed_at) AS last_successful_sync \
		FROM device_sessions WHERE errors IS NULL AND completed_at IS NOT NULL \
		GROUP BY device_id \
	) \
	SELECT a.device_id, \
		ARRAY(SELECT jsonb_array_elements_text(a.facility_ids))::text[] AS facility_ids, \
		ARRAY(SELECT COALESCE(f.name, i.id) \
			FROM jsonb_array_elements_text(a.facility_ids) AS i(id) \
			LEFT JOIN facilities f ON f.id::text = i.id \
			ORDER BY 1)::text[] AS facility_names, \
		ls.last_successful_sync::text AS last_successful_sync, \
		(EXTRACT(EPOCH FROM (now() - ls.last_successful_sync)) / 60)::float8 AS minutes_since_success \
	FROM active a LEFT JOIN last_success ls USING (device_id) \
	ORDER BY minutes_since_success DESC NULLS FIRST";

/// Where a device's last successful sync sits against the thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Staleness {
	Fresh,
	Warn,
	Fail,
}

/// A device that is active but has no successful sync in the window (no
/// reading) is as bad as a very stale one, and fails.
fn grade(minutes: Option<f64>) -> Staleness {
	match minutes {
		Some(m) if m <= WARN_MINUTES => Staleness::Fresh,
		Some(m) if m <= FAIL_MINUTES => Staleness::Warn,
		_ => Staleness::Fail,
	}
}

struct Device {
	id: String,
	facility_ids: Vec<String>,
	facility_names: Vec<String>,
	last_successful_sync: Option<String>,
	minutes_since_success: Option<f64>,
}

impl Device {
	fn instance(&self) -> (Staleness, Instance) {
		let staleness = grade(self.minutes_since_success);
		let reason = match self.minutes_since_success {
			Some(m) => format!("last successful sync {m:.0} minutes ago"),
			None => "no successful sync in the last 30 days".to_string(),
		};
		let instance = match staleness {
			Staleness::Fresh => Instance::pass(&self.id),
			Staleness::Warn => Instance::warning(&self.id, reason),
			Staleness::Fail => Instance::fail(&self.id, reason),
		}
		.with_detail("facility_ids", self.facility_ids.clone())
		.with_detail("last_successful_sync", self.last_successful_sync.clone())
		.with_detail("minutes_since_success", self.minutes_since_success);

		if self.facility_names.is_empty() {
			(staleness, instance)
		} else {
			(
				staleness,
				instance.with_label(self.facility_names.join(", ")),
			)
		}
	}
}

/// Grade every device, passing ones included: the instances are the complete
/// set, and one left out is one that has recovered.
fn report(devices: &[Device]) -> Check {
	let graded: Vec<(Staleness, Instance)> = devices.iter().map(Device::instance).collect();
	let count = |wanted: Staleness| graded.iter().filter(|(s, _)| *s == wanted).count();
	let (fail_n, warn_n) = (count(Staleness::Fail), count(Staleness::Warn));

	let summary = if fail_n + warn_n == 0 {
		format!("{} facility server(s) syncing", graded.len())
	} else {
		format!(
			"stale sync: {fail_n} over {}m, {warn_n} over {}m",
			FAIL_MINUTES as i64, WARN_MINUTES as i64
		)
	};

	Check::instanced(NAME, summary, graded.into_iter().map(|(_, i)| i).collect())
		.with_detail("warn_minutes", WARN_MINUTES)
		.with_detail("fail_minutes", FAIL_MINUTES)
		.with_stat(
			Stat::gauge("fail", fail_n as f64)
				.group("thresholds")
				.help("Facility servers past the fail staleness threshold"),
		)
		.with_stat(
			Stat::gauge("warn", warn_n as f64)
				.group("thresholds")
				.help("Facility servers past the warn staleness threshold"),
		)
}

pub async fn run(ctx: TamanuCx) -> Check {
	let Some(client) = ctx.db().await else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let rows = match client.query(SQL, &[]).await {
		Ok(r) => r,
		Err(err) => return query_error_check(NAME, &err),
	};

	let devices: Vec<Device> = rows
		.iter()
		.map(|row| Device {
			id: row.try_get("device_id").unwrap_or_default(),
			facility_ids: row.try_get("facility_ids").unwrap_or_default(),
			facility_names: row.try_get("facility_names").unwrap_or_default(),
			last_successful_sync: row.try_get("last_successful_sync").ok().flatten(),
			minutes_since_success: row.try_get("minutes_since_success").ok().flatten(),
		})
		.collect();

	report(&devices)
}

#[cfg(test)]
mod tests {
	use tokio_postgres::types::Type;

	use super::{Device, Staleness, grade, report};
	use crate::check::CheckStatus;
	use crate::checks::test_support::{central_ctx, facility_ctx};

	fn device(id: &str, names: &[&str], minutes: Option<f64>) -> Device {
		Device {
			id: id.into(),
			facility_ids: names.iter().map(|n| format!("id-{n}")).collect(),
			facility_names: names.iter().map(|n| n.to_string()).collect(),
			last_successful_sync: minutes.map(|_| "2026-01-01 00:00:00+00".into()),
			minutes_since_success: minutes,
		}
	}

	#[test]
	fn staleness_is_tiered_at_ten_and_thirty_minutes() {
		assert_eq!(grade(Some(0.5)), Staleness::Fresh);
		assert_eq!(grade(Some(10.0)), Staleness::Fresh);
		assert_eq!(grade(Some(10.1)), Staleness::Warn);
		assert_eq!(grade(Some(30.0)), Staleness::Warn);
		assert_eq!(grade(Some(30.1)), Staleness::Fail);
	}

	#[test]
	fn an_active_device_with_no_successful_sync_fails() {
		assert_eq!(grade(None), Staleness::Fail);
	}

	#[test]
	fn each_device_is_an_instance_keyed_by_its_id_passing_ones_included() {
		let check = report(&[
			device("dev-a", &["Apia"], Some(2.0)),
			device("dev-b", &["Savaii", "Upolu"], Some(45.0)),
			device("dev-c", &["Tonga"], Some(15.0)),
		]);
		let instances = check.instances.as_ref().expect("instanced");
		assert_eq!(instances.len(), 3);

		let find = |key: &str| instances.iter().find(|i| i.key == key).unwrap();
		assert!(matches!(find("dev-a").status, CheckStatus::Pass));
		assert!(matches!(find("dev-b").status, CheckStatus::Fail(_)));
		assert!(matches!(find("dev-c").status, CheckStatus::Warning(_)));

		assert_eq!(find("dev-b").label.as_deref(), Some("Savaii, Upolu"));
		assert_eq!(
			find("dev-b").detail["facility_ids"],
			serde_json::json!(["id-Savaii", "id-Upolu"])
		);
		assert_eq!(find("dev-b").detail["minutes_since_success"], 45.0);
		assert!(find("dev-b").detail.contains_key("last_successful_sync"));

		assert!(matches!(check.status, CheckStatus::Fail(_)));
		assert_eq!(check.details["warn_minutes"], 10.0);
		assert_eq!(check.details["fail_minutes"], 30.0);
	}

	#[test]
	fn the_check_wire_form_has_no_fail_or_warn_arrays() {
		let wire = report(&[device("dev-a", &["Apia"], Some(45.0))]).to_wire();
		assert!(wire["detail"].get("fail").is_none());
		assert!(wire["detail"].get("warn").is_none());
		assert_eq!(wire["instances"]["dev-a"]["result"], "failed");
	}

	#[test]
	fn all_fresh_devices_pass() {
		let check = report(&[device("dev-a", &["Apia"], Some(1.0))]);
		assert!(matches!(check.status, CheckStatus::Pass));
	}

	#[test]
	fn no_active_device_reports_an_empty_set() {
		let check = report(&[]);
		assert!(matches!(check.status, CheckStatus::Pass));
		assert_eq!(check.instances.as_ref().map(Vec::len), Some(0));
		assert_eq!(check.to_wire()["instances"], serde_json::json!({}));
	}

	#[test]
	fn a_device_with_no_named_facility_is_labelled_by_its_key() {
		let check = report(&[device("dev-a", &[], Some(1.0))]);
		assert_eq!(check.instances.unwrap()[0].label, None);
	}

	#[tokio::test]
	async fn runs_against_central() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, "sync_facility_stale");
		assert!(matches!(
			check.status,
			CheckStatus::Pass | CheckStatus::Warning(_) | CheckStatus::Fail(_)
		));
	}

	// A numeric column fails to decode as f64, which the tiering reads as "no
	// successful sync" and grades every active facility as FAIL.
	#[tokio::test]
	async fn minutes_column_decodes_as_f64() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let client = ctx.db().await.expect("central connection");
		let stmt = client.prepare(super::SQL).await.expect("prepare");
		let col = stmt
			.columns()
			.iter()
			.find(|c| c.name() == "minutes_since_success")
			.expect("minutes_since_success column");
		assert_eq!(col.type_(), &Type::FLOAT8);
	}

	#[tokio::test]
	async fn skips_on_facility() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
