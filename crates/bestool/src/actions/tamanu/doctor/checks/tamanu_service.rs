use std::process::Command;

use serde_json::{Value, json};

use super::CheckContext;
use crate::actions::tamanu::doctor::check::Check;

pub async fn run(ctx: CheckContext) -> Check {
	if cfg!(target_os = "linux") {
		check_systemd(ctx.config.is_facility())
	} else if cfg!(target_os = "windows") {
		check_pm2(ctx.config.is_facility())
	} else {
		Check::pass(
			"tamanu_service",
			"service check skipped on this platform",
		)
		.with_detail("skipped", true)
	}
}

fn check_systemd(is_facility: bool) -> Check {
	// Enumerate all loaded tamanu-* units. Tamanu uses systemd template units
	// (`tamanu-facility-api@0.service`) for multi-instance services, so we ask
	// for the wildcard and let systemd expand it.
	let output = match Command::new("systemctl")
		.args([
			"list-units",
			"--type=service",
			"--all",
			"--no-legend",
			"--plain",
			"--no-pager",
			"tamanu-*.service",
		])
		.output()
	{
		Ok(o) => o,
		Err(err) => {
			return Check::fail(
				"tamanu_service",
				"systemctl unavailable",
				err.to_string(),
			)
			.with_detail("supervisor", "systemd");
		}
	};

	let stdout = String::from_utf8_lossy(&output.stdout);
	let mut services: Vec<Value> = Vec::new();
	let mut active_count = 0;
	let mut total_count = 0;

	for line in stdout.lines() {
		// `list-units --plain --no-legend` columns: UNIT LOAD ACTIVE SUB DESCRIPTION...
		let mut parts = line.split_whitespace();
		let (Some(unit), Some(load), Some(active), Some(sub)) = (
			parts.next(),
			parts.next(),
			parts.next(),
			parts.next(),
		) else {
			continue;
		};
		// Skip "not-found" rows that systemctl emits for wildcards with no match.
		if load == "not-found" {
			continue;
		}
		total_count += 1;
		let healthy = active == "active" && (sub == "running" || sub == "exited");
		if healthy {
			active_count += 1;
		}
		services.push(json!({
			"name": unit,
			"load": load,
			"active": active,
			"sub": sub,
		}));
	}

	let expected = expected_systemd_units(is_facility);

	if total_count == 0 {
		return Check::fail(
			"tamanu_service",
			"no tamanu-* units found",
			format!(
				"expected at least one of: {}",
				expected.join(", ")
			),
		)
		.with_detail("supervisor", "systemd")
		.with_detail("expected", Value::Array(
			expected.iter().map(|s| Value::String((*s).into())).collect(),
		));
	}

	// Identify missing expected units. For template-style entries ending in
	// `@`, count any instance as fulfilling the expectation.
	let mut missing: Vec<&str> = Vec::new();
	for exp in &expected {
		let matched = if exp.ends_with('@') {
			services.iter().any(|s| {
				s["name"]
					.as_str()
					.map(|n| n.starts_with(exp))
					.unwrap_or(false)
			})
		} else {
			let full = format!("{exp}.service");
			services
				.iter()
				.any(|s| s["name"].as_str() == Some(full.as_str()))
		};
		if !matched {
			missing.push(exp);
		}
	}

	let summary = format!("{active_count}/{total_count} units active");
	let check = if !missing.is_empty() {
		Check::fail(
			"tamanu_service",
			summary,
			format!("missing expected unit(s): {}", missing.join(", ")),
		)
	} else if active_count < total_count {
		let unhealthy: Vec<String> = services
			.iter()
			.filter(|s| s["active"].as_str() != Some("active"))
			.filter_map(|s| s["name"].as_str().map(String::from))
			.collect();
		Check::fail(
			"tamanu_service",
			format!("{active_count}/{total_count} units active"),
			format!("not active: {}", unhealthy.join(", ")),
		)
	} else {
		Check::pass("tamanu_service", format!("{total_count} units active"))
	};

	check.with_detail("supervisor", "systemd")
		.with_detail("services", Value::Array(services))
		.with_detail(
			"expected",
			Value::Array(
				expected
					.iter()
					.map(|s| Value::String((*s).into()))
					.collect(),
			),
		)
}

fn expected_systemd_units(is_facility: bool) -> Vec<&'static str> {
	if is_facility {
		vec![
			"tamanu-facility-api@",
			"tamanu-facility-sync",
			"tamanu-facility-tasks",
			"tamanu-frontend@",
		]
	} else {
		// Central Linux deployments aren't standardised in the same way; check
		// what's loaded but don't fail purely on unit absence.
		Vec::new()
	}
}

fn check_pm2(is_facility: bool) -> Check {
	let output = match Command::new("pm2").arg("jlist").output() {
		Ok(o) if o.status.success() => o,
		Ok(o) => {
			return Check::fail(
				"tamanu_service",
				"pm2 jlist failed",
				String::from_utf8_lossy(&o.stderr).trim().to_string(),
			)
			.with_detail("supervisor", "pm2");
		}
		Err(err) => {
			return Check::fail("tamanu_service", "pm2 unavailable", err.to_string())
				.with_detail("supervisor", "pm2");
		}
	};

	let parsed: Value = match serde_json::from_slice(&output.stdout) {
		Ok(v) => v,
		Err(err) => {
			return Check::fail(
				"tamanu_service",
				"pm2 jlist returned invalid JSON",
				err.to_string(),
			)
			.with_detail("supervisor", "pm2");
		}
	};

	let mut services: Vec<Value> = Vec::new();
	let mut online_count = 0;
	let mut total_count = 0;

	if let Some(procs) = parsed.as_array() {
		for p in procs {
			let Some(name) = p["name"].as_str() else {
				continue;
			};
			if !name.starts_with("tamanu-") {
				continue;
			}
			total_count += 1;
			let state = p["pm2_env"]["status"].as_str().unwrap_or("unknown");
			let pm_id = p["pm_id"].as_i64();
			if state == "online" {
				online_count += 1;
			}
			let mut entry = json!({
				"name": name,
				"state": state,
			});
			if let Some(id) = pm_id
				&& let Some(o) = entry.as_object_mut()
			{
				o.insert("pm_id".into(), id.into());
			}
			services.push(entry);
		}
	}

	let expected = expected_pm2_processes(is_facility);

	if total_count == 0 {
		return Check::fail(
			"tamanu_service",
			"no tamanu-* pm2 processes found",
			format!("expected: {}", expected.join(", ")),
		)
		.with_detail("supervisor", "pm2")
		.with_detail(
			"expected",
			Value::Array(expected.iter().map(|s| Value::String((*s).into())).collect()),
		);
	}

	let mut missing: Vec<&str> = Vec::new();
	for exp in &expected {
		if !services
			.iter()
			.any(|s| s["name"].as_str() == Some(*exp))
		{
			missing.push(exp);
		}
	}

	let summary = format!("{online_count}/{total_count} processes online");
	let check = if !missing.is_empty() {
		Check::fail(
			"tamanu_service",
			summary,
			format!("missing expected process(es): {}", missing.join(", ")),
		)
	} else if online_count < total_count {
		let offline: Vec<String> = services
			.iter()
			.filter(|s| s["state"].as_str() != Some("online"))
			.filter_map(|s| {
				let name = s["name"].as_str()?;
				let pm_id = s["pm_id"].as_i64();
				Some(match pm_id {
					Some(id) => format!("{name}#{id}"),
					None => name.to_string(),
				})
			})
			.collect();
		Check::fail(
			"tamanu_service",
			summary,
			format!("not online: {}", offline.join(", ")),
		)
	} else {
		Check::pass("tamanu_service", format!("{total_count} processes online"))
	};

	check.with_detail("supervisor", "pm2")
		.with_detail("services", Value::Array(services))
		.with_detail(
			"expected",
			Value::Array(
				expected
					.iter()
					.map(|s| Value::String((*s).into()))
					.collect(),
			),
		)
}

fn expected_pm2_processes(is_facility: bool) -> Vec<&'static str> {
	if is_facility {
		vec!["tamanu-api", "tamanu-sync", "tamanu-tasks"]
	} else {
		vec!["tamanu-api", "tamanu-tasks"]
	}
}
