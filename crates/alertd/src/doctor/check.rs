use serde_json::{Map, Value, json};

/// Outcome of a single healthcheck.
#[derive(Debug, Clone)]
pub enum CheckStatus {
	/// All good.
	Pass,
	/// The check couldn't be run — either the platform doesn't support it, the
	/// caller lacked the privilege to query the underlying source, or the
	/// upstream (e.g. caddy `/metrics`) wasn't reachable. Reported as
	/// `healthy: true` on the wire (we have no evidence of unhealth) but does
	/// NOT count as "passing" in human-readable output.
	Skip(String),
	/// Non-fatal degradation. Reported as `healthy: false` per-check on the
	/// canopy wire format, but does NOT flip the top-level `healthy` flag.
	Warning(String),
	/// Fatal failure. Sets `healthy: false` per-check AND flips top-level to
	/// `healthy: false`.
	Fail(String),
}

impl CheckStatus {
	/// Whether this status maps to `healthy: true` in the per-check wire format.
	pub fn is_healthy_on_wire(&self) -> bool {
		matches!(self, CheckStatus::Pass | CheckStatus::Skip(_))
	}

	/// Whether this status is fatal (flips top-level `healthy` to false).
	pub fn is_fatal(&self) -> bool {
		matches!(self, CheckStatus::Fail(_))
	}

	/// Whether this status is a skip — useful for rendering and accounting.
	pub fn is_skip(&self) -> bool {
		matches!(self, CheckStatus::Skip(_))
	}
}

/// Result of one healthcheck.
#[derive(Debug, Clone)]
pub struct Check {
	/// Stable identifier, also used as the `check` field on the wire.
	pub name: &'static str,
	pub status: CheckStatus,
	/// Short human-readable description for the CLI output.
	pub summary: String,
	/// Extra JSON fields merged into the per-check wire payload.
	pub details: Map<String, Value>,
	/// Fields a check wants to attach to the *top-level* status payload
	/// (alongside `osTimezone` etc.), rather than to its own `health[]`
	/// entry. Lifted by `build_payload` and never serialised into
	/// per-check wire output. Used for bulky data (raw service inventory,
	/// for instance) that belongs with server facts, not with
	/// diagnostics.
	pub payload_extras: Map<String, Value>,
}

impl Check {
	pub fn pass(name: &'static str, summary: impl Into<String>) -> Self {
		Self {
			name,
			status: CheckStatus::Pass,
			summary: summary.into(),
			details: Map::new(),
			payload_extras: Map::new(),
		}
	}

	/// Build a Skip result. The `reason` is recorded in `details.reason` (or
	/// kept on the status) so the operator sees *why* the check couldn't be
	/// run; the summary is the short headline shown alongside `SKIP`.
	pub fn skip(name: &'static str, summary: impl Into<String>, reason: impl Into<String>) -> Self {
		let mut details = Map::new();
		details.insert("skipped".into(), Value::Bool(true));
		Self {
			name,
			status: CheckStatus::Skip(reason.into()),
			summary: summary.into(),
			details,
			payload_extras: Map::new(),
		}
	}

	pub fn warning(
		name: &'static str,
		summary: impl Into<String>,
		reason: impl Into<String>,
	) -> Self {
		Self {
			name,
			status: CheckStatus::Warning(reason.into()),
			summary: summary.into(),
			details: Map::new(),
			payload_extras: Map::new(),
		}
	}

	pub fn fail(name: &'static str, summary: impl Into<String>, reason: impl Into<String>) -> Self {
		Self {
			name,
			status: CheckStatus::Fail(reason.into()),
			summary: summary.into(),
			details: Map::new(),
			payload_extras: Map::new(),
		}
	}

	pub fn with_detail(mut self, key: &str, value: impl Into<Value>) -> Self {
		self.details.insert(key.to_string(), value.into());
		self
	}

	pub fn with_details(mut self, details: Map<String, Value>) -> Self {
		self.details = details;
		self
	}

	/// Attach a key/value to the top-level status payload (alongside server
	/// facts like `osTimezone`) rather than this check's own `health[]`
	/// entry. See [`Self::payload_extras`].
	pub fn with_payload_extra(mut self, key: &str, value: impl Into<Value>) -> Self {
		self.payload_extras.insert(key.to_string(), value.into());
		self
	}

	/// Build the per-check entry for the canopy `health[]` array.
	pub fn to_wire(&self) -> Value {
		let mut obj = Map::new();
		obj.insert("check".into(), self.name.into());
		obj.insert("healthy".into(), self.status.is_healthy_on_wire().into());
		for (k, v) in &self.details {
			obj.insert(k.clone(), v.clone());
		}
		Value::Object(obj)
	}

	/// Encode this Check for streaming over the daemon's task endpoint.
	///
	/// Distinct from [`Self::to_wire`]: that one is the canopy-bound payload
	/// (which collapses Warning/Fail status to a bare `healthy: false`); this
	/// one preserves the full `CheckStatus` enum so consumers can render the
	/// same colours and reason lines as a local sweep.
	pub fn to_streaming_json(&self) -> Value {
		let (status, reason) = match &self.status {
			CheckStatus::Pass => ("pass", None),
			CheckStatus::Skip(r) => ("skip", Some(r.as_str())),
			CheckStatus::Warning(r) => ("warning", Some(r.as_str())),
			CheckStatus::Fail(r) => ("fail", Some(r.as_str())),
		};
		let mut obj = json!({
			"name": self.name,
			"status": status,
			"summary": self.summary,
			"details": Value::Object(self.details.clone()),
		});
		if let Some(r) = reason {
			obj["reason"] = Value::String(r.to_string());
		}
		obj
	}

	/// Decode a [`Self::to_streaming_json`] payload back into a `Check`.
	///
	/// `name_resolver` is called to look the incoming name string back up to
	/// the `&'static str` slot the registry uses, so the rendering code (which
	/// expects `Check.name: &'static str`) keeps working. Returns `None` for
	/// unknown names or malformed payloads — callers should drop those events.
	pub fn from_streaming_json(
		value: &Value,
		name_resolver: impl FnOnce(&str) -> Option<&'static str>,
	) -> Option<Self> {
		let name_str = value.get("name")?.as_str()?;
		let name = name_resolver(name_str)?;
		let status_str = value.get("status")?.as_str()?;
		let reason = value
			.get("reason")
			.and_then(Value::as_str)
			.map(str::to_string);
		let status = match (status_str, reason) {
			("pass", _) => CheckStatus::Pass,
			("skip", Some(r)) => CheckStatus::Skip(r),
			("warning", Some(r)) => CheckStatus::Warning(r),
			("fail", Some(r)) => CheckStatus::Fail(r),
			_ => return None,
		};
		let summary = value.get("summary")?.as_str()?.to_string();
		let details = value
			.get("details")
			.and_then(Value::as_object)
			.cloned()
			.unwrap_or_default();
		Some(Self {
			name,
			status,
			summary,
			details,
			payload_extras: Map::new(),
		})
	}
}

/// Overall result of running all checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverallResult {
	Healthy,
	Degraded,
	Failing,
}

impl OverallResult {
	pub fn from_checks(checks: &[Check]) -> Self {
		if checks.iter().any(|c| c.status.is_fatal()) {
			OverallResult::Failing
		} else if checks
			.iter()
			.any(|c| matches!(c.status, CheckStatus::Warning(_)))
		{
			OverallResult::Degraded
		} else {
			OverallResult::Healthy
		}
	}

	/// Whether the top-level `healthy` flag on the wire is `true`.
	pub fn is_healthy_top_level(self) -> bool {
		!matches!(self, OverallResult::Failing)
	}

	pub fn label(self) -> &'static str {
		match self {
			OverallResult::Healthy => "HEALTHY",
			OverallResult::Degraded => "DEGRADED",
			OverallResult::Failing => "FAILING",
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pass_is_healthy_on_wire() {
		assert!(Check::pass("x", "ok").status.is_healthy_on_wire());
	}

	#[test]
	fn warning_is_unhealthy_on_wire_but_not_fatal() {
		let s = CheckStatus::Warning("w".into());
		assert!(!s.is_healthy_on_wire());
		assert!(!s.is_fatal());
	}

	#[test]
	fn fail_is_unhealthy_and_fatal() {
		let s = CheckStatus::Fail("f".into());
		assert!(!s.is_healthy_on_wire());
		assert!(s.is_fatal());
	}

	#[test]
	fn skip_is_healthy_on_wire_and_not_fatal() {
		// Skip means "we didn't run this check" — it must not fire alerts
		// or flip the top-level healthy flag, since we have no evidence of
		// unhealth either way.
		let s = CheckStatus::Skip("reason".into());
		assert!(s.is_healthy_on_wire());
		assert!(!s.is_fatal());
		assert!(s.is_skip());
	}

	#[test]
	fn skip_does_not_change_overall_result() {
		let with_skip = vec![Check::pass("a", ""), Check::skip("b", "", "r")];
		assert_eq!(
			OverallResult::from_checks(&with_skip),
			OverallResult::Healthy
		);
	}

	#[test]
	fn skip_constructor_marks_skipped_detail() {
		let c = Check::skip("memory", "not available", "platform mismatch");
		assert_eq!(
			c.details.get("skipped").and_then(Value::as_bool),
			Some(true)
		);
		assert!(matches!(c.status, CheckStatus::Skip(_)));
	}

	#[test]
	fn overall_from_checks() {
		let healthy = vec![Check::pass("a", "")];
		assert_eq!(OverallResult::from_checks(&healthy), OverallResult::Healthy);

		let degraded = vec![Check::pass("a", ""), Check::warning("b", "", "x")];
		assert_eq!(
			OverallResult::from_checks(&degraded),
			OverallResult::Degraded
		);

		let failing = vec![Check::warning("a", "", "x"), Check::fail("b", "", "y")];
		assert_eq!(OverallResult::from_checks(&failing), OverallResult::Failing);
	}

	#[test]
	fn overall_top_level_healthy_only_true_when_not_failing() {
		assert!(OverallResult::Healthy.is_healthy_top_level());
		assert!(OverallResult::Degraded.is_healthy_top_level());
		assert!(!OverallResult::Failing.is_healthy_top_level());
	}

	#[test]
	fn check_to_wire_pass() {
		let c = Check::pass("db_connect", "ok").with_detail("latency_ms", 3);
		let v = c.to_wire();
		assert_eq!(v["check"], "db_connect");
		assert_eq!(v["healthy"], true);
		assert_eq!(v["latency_ms"], 3);
	}

	#[test]
	fn check_to_wire_warning_marks_unhealthy() {
		let c = Check::warning("disk_free", "20% used", "below threshold");
		let v = c.to_wire();
		assert_eq!(v["healthy"], false);
	}
}
