//! Facility servers stuck in a sync restart loop.
//!
//! Counts `snapshot-for-pushing` sync errors per device in the last hour, which
//! indicates sync repeatedly restarting rather than progressing. WARN at 5
//! restarts/hr, FAIL at 10.
//!
//! Like `sync_facility_stale`, a facility server is the device running it, and
//! each is one instance keyed by its device id. A session naming no device is
//! not measured. Only devices with a restart in the hour are reported: the rest
//! have nothing to grade.

use super::{TamanuCx, query_error_check};
use crate::Stat;
use crate::check::{Check, Instance};

const NAME: &str = "sync_restart_loop";

const WARN_RESTARTS: i64 = 5;
const FAIL_RESTARTS: i64 = 10;

// A device's facilities are those of its most recent restarting session, and a
// facility with no record is named by its id.
const SQL: &str = "WITH restarts AS ( \
		SELECT parameters->>'deviceId' AS device_id, COUNT(*) AS error_count, \
			(array_agg(CASE WHEN jsonb_typeof(parameters->'facilityIds') = 'array' \
				THEN parameters->'facilityIds' ELSE '[]'::jsonb END \
				ORDER BY created_at DESC))[1] AS facility_ids \
		FROM sync_sessions \
		WHERE created_at > now() - interval '1 hour' AND errors IS NOT NULL \
			AND cardinality(errors) = 1 AND errors[1] LIKE '%snapshot-for-pushing%' \
			AND parameters->>'deviceId' IS NOT NULL \
		GROUP BY 1 \
	) \
	SELECT r.device_id, r.error_count, \
		ARRAY(SELECT jsonb_array_elements_text(r.facility_ids))::text[] AS facility_ids, \
		ARRAY(SELECT COALESCE(f.name, i.id) \
			FROM jsonb_array_elements_text(r.facility_ids) AS i(id) \
			LEFT JOIN facilities f ON f.id::text = i.id \
			ORDER BY 1)::text[] AS facility_names \
	FROM restarts r ORDER BY r.error_count DESC";

struct Device {
	id: String,
	restarts: i64,
	facility_ids: Vec<String>,
	facility_names: Vec<String>,
}

fn instance(device: &Device) -> Instance {
	let reason = format!("{} restarts in the last hour", device.restarts);
	let instance = if device.restarts >= FAIL_RESTARTS {
		Instance::fail(&device.id, reason)
	} else if device.restarts >= WARN_RESTARTS {
		Instance::warning(&device.id, reason)
	} else {
		Instance::pass(&device.id)
	}
	.with_detail("error_count", device.restarts)
	.with_detail("facility_ids", device.facility_ids.clone());

	if device.facility_names.is_empty() {
		instance
	} else {
		instance.with_label(device.facility_names.join(", "))
	}
}

fn report(devices: &[Device]) -> Check {
	let instances: Vec<Instance> = devices.iter().map(instance).collect();
	let over = |threshold: i64| devices.iter().filter(|d| d.restarts >= threshold).count();
	let fail_n = over(FAIL_RESTARTS);
	let warn_n = over(WARN_RESTARTS) - fail_n;

	let summary = if fail_n + warn_n == 0 {
		"no sync restart loops".to_string()
	} else {
		format!(
			"sync restart loops: {fail_n} over {FAIL_RESTARTS}/hr, {warn_n} over {WARN_RESTARTS}/hr"
		)
	};

	Check::instanced(NAME, summary, instances)
		.with_detail("warn_restarts", WARN_RESTARTS)
		.with_detail("fail_restarts", FAIL_RESTARTS)
		.with_stat(
			Stat::gauge("fail", fail_n as f64)
				.group("thresholds")
				.help("Facility servers past the fail restart rate"),
		)
		.with_stat(
			Stat::gauge("warn", warn_n as f64)
				.group("thresholds")
				.help("Facility servers past the warn restart rate"),
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
			restarts: row.try_get("error_count").unwrap_or(0),
			facility_ids: row.try_get("facility_ids").unwrap_or_default(),
			facility_names: row.try_get("facility_names").unwrap_or_default(),
		})
		.collect();

	report(&devices)
}

#[cfg(test)]
mod tests {
	use super::{Device, report};
	use crate::check::CheckStatus;
	use crate::checks::test_support::{central_ctx, facility_ctx};

	fn device(id: &str, name: &str, restarts: i64) -> Device {
		Device {
			id: id.into(),
			restarts,
			facility_ids: vec![format!("id-{name}")],
			facility_names: vec![name.into()],
		}
	}

	#[test]
	fn each_device_is_an_instance_graded_by_its_own_restart_count() {
		let check = report(&[
			device("dev-a", "Apia", 12),
			device("dev-b", "Savaii", 6),
			device("dev-c", "Tonga", 2),
		]);
		let instances = check.instances.as_ref().expect("instanced");
		let status = |key: &str| &instances.iter().find(|i| i.key == key).unwrap().status;

		assert!(matches!(status("dev-a"), CheckStatus::Fail(_)));
		assert!(matches!(status("dev-b"), CheckStatus::Warning(_)));
		assert!(matches!(status("dev-c"), CheckStatus::Pass));
		assert!(matches!(check.status, CheckStatus::Fail(_)));
		assert_eq!(
			check.summary,
			"sync restart loops: 1 over 10/hr, 1 over 5/hr"
		);

		let a = instances.iter().find(|i| i.key == "dev-a").unwrap();
		assert_eq!(a.label.as_deref(), Some("Apia"));
		assert_eq!(a.detail["error_count"], 12);
		assert_eq!(a.detail["facility_ids"], serde_json::json!(["id-Apia"]));
	}

	#[test]
	fn no_restarts_reports_an_empty_passing_set() {
		let check = report(&[]);
		assert!(matches!(check.status, CheckStatus::Pass));
		assert_eq!(check.instances.as_ref().map(Vec::len), Some(0));
		assert_eq!(check.summary, "no sync restart loops");
	}

	#[test]
	fn the_old_fail_and_warn_arrays_are_gone() {
		let wire = report(&[device("dev-a", "Apia", 12)]).to_wire();
		assert!(wire["detail"].get("fail").is_none());
		assert!(wire["detail"].get("warn").is_none());
		assert_eq!(wire["instances"]["dev-a"]["result"], "failed");
	}

	#[tokio::test]
	async fn runs_against_central() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, "sync_restart_loop");
		assert!(matches!(
			check.status,
			CheckStatus::Pass | CheckStatus::Warning(_) | CheckStatus::Fail(_)
		));
	}

	#[tokio::test]
	async fn skips_on_facility() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
