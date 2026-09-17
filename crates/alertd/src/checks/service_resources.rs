//! Per-service memory and processor usage.
//!
//! Telemetry first: an application's resource usage is reported whether or not
//! anything grades it, dimensioned by duty and service so two services doing the
//! same job stay distinguishable.
//!
//! A service is graded only against a ceiling declared for it — a container's
//! memory limit, a Kubernetes container limit, the memory bounds configured on a
//! supervised unit. Never against the machine's total, which is shared with
//! everything else on it and says nothing about whether a service is near its
//! own limit. Where no ceiling is declared there is no denominator to take a
//! percentage of, so usage is reported and the grading skips for that service.
//!
//! spec: SUB#resource-usage-per-service

use serde_json::{Value, json};

use crate::Stat;
use crate::check::Check;
use crate::runtime::{Duty, ServiceFacts, ServiceId, ServiceRuntime};

const NAME: &str = "service_resources";

/// Share of its declared ceiling at which a service is worth a warning. Above
/// this there is little headroom left for a spike.
const WARN_FRACTION: f64 = 0.85;
/// Share at which it is worth an alert: a service this close to a hard limit is
/// about to be killed rather than merely short of room.
const FAIL_FRACTION: f64 = 0.95;

/// A Tamanu deployment's services.
pub mod tamanu {
	use super::super::TamanuCx;
	use crate::check::Check;

	pub async fn run(ctx: TamanuCx) -> Check {
		super::report(ctx.runtime.as_ref()).await
	}
}

/// The service running a Postgres installation.
pub mod postgres {
	use super::super::PgCx;
	use crate::check::Check;

	pub async fn run(ctx: PgCx) -> Check {
		super::report(ctx.runtime.as_ref()).await
	}
}

/// One service's usage as this check needs it.
struct Reading {
	duty: Duty,
	id: ServiceId,
	facts: ServiceFacts,
}

impl Reading {
	/// What fraction of its declared ceiling this service is using, where both
	/// a usage and a ceiling were read.
	fn fraction(&self) -> Option<f64> {
		let used = self.facts.memory_bytes?;
		let ceiling = self.facts.memory_ceiling_bytes?;
		(ceiling > 0).then(|| used as f64 / ceiling as f64)
	}
}

async fn report(runtime: &dyn ServiceRuntime) -> Check {
	let services = match runtime.services().await {
		Ok(services) => services,
		Err(unavailable) => {
			return Check::skip(NAME, "the workload could not be read", unavailable.reason());
		}
	};

	let mut readings = Vec::with_capacity(services.len());
	for service in services {
		let Ok(facts) = runtime.service_facts(&service.id).await else {
			continue;
		};
		readings.push(Reading {
			duty: service.duty,
			id: service.id,
			facts,
		});
	}

	grade(&readings)
}

fn grade(readings: &[Reading]) -> Check {
	let mut rows: Vec<Value> = Vec::new();
	let mut findings: Vec<String> = Vec::new();
	let mut worst: Option<f64> = None;
	let mut graded = 0usize;

	for reading in readings {
		let fraction = reading.fraction();
		rows.push(json!({
			"duty": reading.duty.to_string(),
			"service": reading.id.to_string(),
			"memory_bytes": reading.facts.memory_bytes,
			"memory_ceiling_bytes": reading.facts.memory_ceiling_bytes,
			"processor_seconds": reading.facts.processor_seconds,
			"memory_pct_of_ceiling": fraction.map(|f| (f * 100.0).round()),
		}));

		let Some(fraction) = fraction else {
			continue;
		};
		graded += 1;
		worst = Some(worst.map_or(fraction, |w: f64| w.max(fraction)));
		if fraction >= WARN_FRACTION {
			findings.push(format!(
				"{} ({}) at {:.0}% of its declared memory ceiling",
				reading.duty,
				reading.id,
				fraction * 100.0,
			));
		}
	}

	let summary = if readings.is_empty() {
		"no services to measure".to_string()
	} else if graded == 0 {
		// Every service reported usage but none declares a ceiling, so there is
		// nothing to take a percentage of. The metrics still went out.
		format!(
			"{} service(s) measured, none with a declared ceiling",
			readings.len()
		)
	} else if findings.is_empty() {
		format!(
			"{graded} of {} service(s) within their declared ceilings",
			readings.len()
		)
	} else if findings.len() == 1 {
		findings[0].clone()
	} else {
		format!("{} service(s) near their declared ceilings", findings.len())
	};

	let check = match worst {
		Some(worst) if worst >= FAIL_FRACTION => Check::fail(NAME, summary, findings.join("; ")),
		_ if !findings.is_empty() => Check::warning(NAME, summary, findings.join("; ")),
		_ => Check::pass(NAME, summary),
	};

	with_stats(check.with_detail("services", Value::Array(rows)), readings)
}

/// The telemetry, which goes out whatever the verdict: a service's usage is
/// reported whether or not anything grades it.
fn with_stats(mut check: Check, readings: &[Reading]) -> Check {
	for reading in readings {
		let duty = reading.duty.to_string();
		let service = reading.id.to_string();

		if let Some(bytes) = reading.facts.memory_bytes {
			check = check.with_stat(
				Stat::gauge("memory_bytes", bytes as f64)
					.label("duty", duty.clone())
					.label("service", service.clone())
					.group("memory")
					.help("Memory in use by one service"),
			);
		}
		if let Some(bytes) = reading.facts.memory_ceiling_bytes {
			check = check.with_stat(
				Stat::gauge("memory_ceiling_bytes", bytes as f64)
					.label("duty", duty.clone())
					.label("service", service.clone())
					.group("memory")
					.help("Memory ceiling declared for one service"),
			);
		}
		if let Some(seconds) = reading.facts.processor_seconds {
			check = check.with_stat(
				Stat::counter("processor_seconds", seconds)
					.label("duty", duty)
					.label("service", service)
					.help("Processor time consumed by one service since it started"),
			);
		}
	}
	check
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::check::CheckStatus;
	use crate::runtime::TamanuDuty;

	fn reading(name: &str, used: Option<u64>, ceiling: Option<u64>) -> Reading {
		Reading {
			duty: Duty::from_tamanu_service_name(name),
			id: ServiceId::new(format!("{name}.service")),
			facts: ServiceFacts {
				up: true,
				memory_bytes: used,
				memory_ceiling_bytes: ceiling,
				processor_seconds: Some(12.5),
				..Default::default()
			},
		}
	}

	/// A service with no declared ceiling has no denominator, so its usage is
	/// reported and the grading skips for it. Never graded against the
	/// machine's total, which says nothing about this service's own limit.
	///
	/// spec: SUB#resource-usage-per-service
	#[test]
	fn a_service_with_no_ceiling_is_measured_but_not_graded() {
		let readings = [reading(
			"tamanu-central-api",
			Some(64 * 1024 * 1024 * 1024),
			None,
		)];
		let check = grade(&readings);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
		assert!(
			check
				.stats
				.iter()
				.any(|s| s.name == "memory_bytes" && s.value > 0.0),
			"usage is reported whether or not anything grades it"
		);
		assert!(
			!check.stats.iter().any(|s| s.name == "memory_ceiling_bytes"),
			"no ceiling was declared, so none is reported"
		);
	}

	/// A service with a ceiling is graded against that ceiling.
	///
	/// spec: SUB#resource-usage-per-service
	#[test]
	fn a_service_near_its_declared_ceiling_warns() {
		let readings = [reading("tamanu-central-api", Some(900), Some(1000))];
		let check = grade(&readings);
		assert!(matches!(check.status, CheckStatus::Warning(_)), "{check:?}");
	}

	#[test]
	fn a_service_at_its_declared_ceiling_fails() {
		let readings = [reading("tamanu-central-api", Some(990), Some(1000))];
		let check = grade(&readings);
		assert!(matches!(check.status, CheckStatus::Fail(_)), "{check:?}");
	}

	#[test]
	fn a_service_well_inside_its_ceiling_passes() {
		let readings = [reading("tamanu-central-api", Some(100), Some(1000))];
		let check = grade(&readings);
		assert!(matches!(check.status, CheckStatus::Pass), "{check:?}");
	}

	/// Metrics carry both dimensions, so two services doing the same job stay
	/// distinguishable and one duty's total can still be taken.
	///
	/// spec: SUB#resource-usage-per-service
	#[test]
	fn metrics_are_dimensioned_by_duty_and_service() {
		let readings = [
			reading("tamanu-central-api", Some(100), Some(1000)),
			reading("tamanu-central-tasks", Some(200), Some(1000)),
		];
		let check = grade(&readings);
		let memory: Vec<&Stat> = check
			.stats
			.iter()
			.filter(|s| s.name == "memory_bytes")
			.collect();
		assert_eq!(memory.len(), 2);
		for stat in memory {
			assert!(stat.labels.iter().any(|(k, _)| *k == "duty"));
			assert!(stat.labels.iter().any(|(k, _)| *k == "service"));
		}
		assert!(
			check.stats.iter().any(|s| {
				s.name == "memory_bytes"
					&& s.labels
						.iter()
						.any(|(k, v)| *k == "duty" && v == TamanuDuty::Tasks.as_str())
			}),
			"the tasks duty is named in its own metric"
		);
	}

	/// Processor time is cumulative, so it is a counter: two readings a sweep
	/// apart give the rate, and a restart reads as the reset it is.
	#[test]
	fn processor_time_is_a_counter() {
		let readings = [reading("tamanu-central-api", Some(100), Some(1000))];
		let check = grade(&readings);
		let stat = check
			.stats
			.iter()
			.find(|s| s.name == "processor_seconds")
			.expect("processor time is reported");
		assert_eq!(stat.kind, crate::StatKind::Counter);
	}
}
