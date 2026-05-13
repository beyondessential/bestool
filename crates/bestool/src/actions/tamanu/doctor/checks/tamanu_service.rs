use std::process::Command;

use super::CheckContext;
use crate::actions::tamanu::doctor::check::Check;

pub async fn run(ctx: CheckContext) -> Check {
	let unit = if ctx.config.is_facility() {
		"tamanu-facility"
	} else {
		"tamanu-central"
	};

	if cfg!(target_os = "linux") {
		check_systemd(unit)
	} else if cfg!(target_os = "windows") {
		check_pm2(unit)
	} else {
		Check::pass(
			"tamanu_service",
			"service check skipped on this platform",
		)
		.with_detail("skipped", true)
	}
}

fn check_systemd(unit: &str) -> Check {
	let output = match Command::new("systemctl")
		.args(["is-active", unit])
		.output()
	{
		Ok(o) => o,
		Err(err) => {
			return Check::fail(
				"tamanu_service",
				"systemctl unavailable",
				err.to_string(),
			)
			.with_detail("supervisor", "systemd")
			.with_detail("unit_or_process_name", unit.to_string());
		}
	};

	let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
	let healthy = state == "active";

	let check = if healthy {
		Check::pass("tamanu_service", format!("{unit} active"))
	} else {
		Check::fail(
			"tamanu_service",
			format!("{unit} state: {state}"),
			format!("systemctl reports {state}"),
		)
	};

	check.with_detail("supervisor", "systemd")
		.with_detail("unit_or_process_name", unit.to_string())
		.with_detail("state", state)
}

fn check_pm2(process: &str) -> Check {
	let output = match Command::new("pm2").arg("jlist").output() {
		Ok(o) if o.status.success() => o,
		Ok(o) => {
			return Check::fail(
				"tamanu_service",
				"pm2 jlist failed",
				String::from_utf8_lossy(&o.stderr).trim().to_string(),
			)
			.with_detail("supervisor", "pm2")
			.with_detail("unit_or_process_name", process.to_string());
		}
		Err(err) => {
			return Check::fail("tamanu_service", "pm2 unavailable", err.to_string())
				.with_detail("supervisor", "pm2")
				.with_detail("unit_or_process_name", process.to_string());
		}
	};

	let parsed: serde_json::Value = match serde_json::from_slice(&output.stdout) {
		Ok(v) => v,
		Err(err) => {
			return Check::fail(
				"tamanu_service",
				"pm2 jlist returned invalid JSON",
				err.to_string(),
			)
			.with_detail("supervisor", "pm2")
			.with_detail("unit_or_process_name", process.to_string());
		}
	};

	let entry = parsed
		.as_array()
		.and_then(|procs| procs.iter().find(|p| p["name"] == process));

	let Some(entry) = entry else {
		return Check::fail(
			"tamanu_service",
			format!("pm2 process {process} not found"),
			"missing from pm2 jlist",
		)
		.with_detail("supervisor", "pm2")
		.with_detail("unit_or_process_name", process.to_string());
	};

	let state = entry["pm2_env"]["status"]
		.as_str()
		.unwrap_or("unknown")
		.to_string();
	let healthy = state == "online";

	let check = if healthy {
		Check::pass("tamanu_service", format!("{process} online"))
	} else {
		Check::fail(
			"tamanu_service",
			format!("{process} state: {state}"),
			format!("pm2 reports {state}"),
		)
	};
	check.with_detail("supervisor", "pm2")
		.with_detail("unit_or_process_name", process.to_string())
		.with_detail("state", state)
}
