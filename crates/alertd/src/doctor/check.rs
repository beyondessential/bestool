use bestool_canopy::schema::CheckSeverity;
use serde_json::{Map, Value, json};

use crate::doctor::stat::Stat;

/// Outcome of a single healthcheck.
///
/// Serialised on the wire as the per-check `result` field, a proper sum type
/// exhaustively matchable on both sides:
/// `passed | warning | failed | broken | skipped`.
#[derive(Debug, Clone)]
pub enum CheckStatus {
	/// Check ran, system OK. `result: "passed"`.
	Pass,
	/// Precondition not met; the check didn't run — either the platform
	/// doesn't support it, the caller lacked the privilege to query the
	/// underlying source, or the check doesn't apply to this server kind.
	/// Says nothing about the system. `result: "skipped"`.
	Skip(String),
	/// Check ran, system degraded but not fatally. `result: "warning"`.
	Warning(String),
	/// Check ran, system under test is unhealthy. `result: "failed"`.
	Fail(String),
	/// The check itself errored or is misconfigured (e.g. its SQL no longer
	/// matches the schema); says nothing about the system. `result: "broken"`.
	Broken(String),
}

impl CheckStatus {
	/// The `result` value for this status in the per-check wire format.
	pub fn wire_result(&self) -> &'static str {
		match self {
			CheckStatus::Pass => "passed",
			CheckStatus::Skip(_) => "skipped",
			CheckStatus::Warning(_) => "warning",
			CheckStatus::Fail(_) => "failed",
			CheckStatus::Broken(_) => "broken",
		}
	}

	/// Whether this status is fatal (the system under test is unhealthy).
	pub fn is_fatal(&self) -> bool {
		matches!(self, CheckStatus::Fail(_))
	}

	/// Whether this status is a skip — useful for rendering and accounting.
	pub fn is_skip(&self) -> bool {
		matches!(self, CheckStatus::Skip(_))
	}

	/// Apply canopy's effective-severity ceiling to this status.
	///
	/// The severity canopy reports for a check is a *ceiling*, never a floor: a
	/// computed status is only ever lowered towards it, never raised. So a check
	/// that passes still passes even when its ceiling is `warn`, and a `warn`
	/// finding is never promoted to a `fail` just because the ceiling allows it.
	///
	/// The ceiling only bites on the two states that would otherwise alert:
	/// * `fail` ceiling — leaves everything as computed.
	/// * `warn` ceiling — a computed [`Fail`](Self::Fail) drops to
	///   [`Warning`](Self::Warning), keeping its reason.
	/// * `skip` ceiling — the check is silenced for this server, so a computed
	///   [`Warning`](Self::Warning) or [`Fail`](Self::Fail) drops to
	///   [`Skip`](Self::Skip), keeping its reason.
	///
	/// [`Broken`](Self::Broken) is left untouched: it reports that the check
	/// itself errored (bad SQL, a missing column), which is a fault in our own
	/// diagnostics rather than a severity finding about the system, and shouldn't
	/// be silenced by an operator muting the check's *result*.
	pub fn cap_to(self, ceiling: CheckSeverity) -> Self {
		match (ceiling, self) {
			(CheckSeverity::Skip, CheckStatus::Warning(reason) | CheckStatus::Fail(reason)) => {
				CheckStatus::Skip(reason)
			}
			(CheckSeverity::Warn, CheckStatus::Fail(reason)) => CheckStatus::Warning(reason),
			(_, status) => status,
		}
	}

	/// The explanatory reason carried by every non-pass status, if any.
	pub fn reason(&self) -> Option<&str> {
		match self {
			CheckStatus::Pass => None,
			CheckStatus::Skip(r)
			| CheckStatus::Warning(r)
			| CheckStatus::Fail(r)
			| CheckStatus::Broken(r) => Some(r),
		}
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
	/// Typed numeric metrics this check declares for the alertd `/metrics`
	/// endpoint. Independent of `details`: the same number may be attached to
	/// both. Never posted to canopy; rendered to munin/prometheus text only.
	pub stats: Vec<Stat>,
}

impl Check {
	pub fn pass(name: &'static str, summary: impl Into<String>) -> Self {
		Self {
			name,
			status: CheckStatus::Pass,
			summary: summary.into(),
			details: Map::new(),
			payload_extras: Map::new(),
			stats: Vec::new(),
		}
	}

	/// Build a Skip result. The `reason` is kept on the status so the operator
	/// sees *why* the check couldn't be run; the summary is the short headline
	/// shown alongside `SKIP`.
	pub fn skip(name: &'static str, summary: impl Into<String>, reason: impl Into<String>) -> Self {
		Self {
			name,
			status: CheckStatus::Skip(reason.into()),
			summary: summary.into(),
			details: Map::new(),
			payload_extras: Map::new(),
			stats: Vec::new(),
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
			stats: Vec::new(),
		}
	}

	pub fn fail(name: &'static str, summary: impl Into<String>, reason: impl Into<String>) -> Self {
		Self {
			name,
			status: CheckStatus::Fail(reason.into()),
			summary: summary.into(),
			details: Map::new(),
			payload_extras: Map::new(),
			stats: Vec::new(),
		}
	}

	/// Build a Broken result: the check itself errored or is misconfigured,
	/// which says nothing about the system under test.
	pub fn broken(
		name: &'static str,
		summary: impl Into<String>,
		reason: impl Into<String>,
	) -> Self {
		Self {
			name,
			status: CheckStatus::Broken(reason.into()),
			summary: summary.into(),
			details: Map::new(),
			payload_extras: Map::new(),
			stats: Vec::new(),
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

	/// Declare a numeric metric for the alertd `/metrics` endpoint. See
	/// [`Self::stats`] and [`Stat`].
	pub fn with_stat(mut self, stat: Stat) -> Self {
		self.stats.push(stat);
		self
	}

	/// Declare several metrics at once. See [`Self::with_stat`].
	pub fn with_stats(mut self, stats: impl IntoIterator<Item = Stat>) -> Self {
		self.stats.extend(stats);
		self
	}

	/// Build the per-check entry for the canopy `health[]` array.
	pub fn to_wire(&self) -> Value {
		let mut obj = Map::new();
		obj.insert("check".into(), self.name.into());
		obj.insert("result".into(), self.status.wire_result().into());
		for (k, v) in &self.details {
			obj.insert(k.clone(), v.clone());
		}
		// Carry the human summary and (for non-pass) the reason so an operator can
		// see *why* a check warned/failed from canopy, without shelling into the
		// box. Inserted after the details so the reserved keys always win.
		obj.insert("summary".into(), self.summary.clone().into());
		if let Some(reason) = self.status.reason() {
			obj.insert("reason".into(), reason.into());
		}
		Value::Object(obj)
	}

	/// Encode this Check for streaming over the daemon's task endpoint.
	///
	/// Distinct from [`Self::to_wire`]: that one is the canopy-bound payload
	/// (which drops the reason); this one preserves the full `CheckStatus`
	/// enum including reasons so consumers can render the same colours and
	/// reason lines as a local sweep.
	pub fn to_streaming_json(&self) -> Value {
		let (status, reason) = match &self.status {
			CheckStatus::Pass => ("pass", None),
			CheckStatus::Skip(r) => ("skip", Some(r.as_str())),
			CheckStatus::Warning(r) => ("warning", Some(r.as_str())),
			CheckStatus::Fail(r) => ("fail", Some(r.as_str())),
			CheckStatus::Broken(r) => ("broken", Some(r.as_str())),
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
			("broken", Some(r)) => CheckStatus::Broken(r),
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
			stats: Vec::new(),
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
			.any(|c| matches!(c.status, CheckStatus::Warning(_) | CheckStatus::Broken(_)))
		{
			OverallResult::Degraded
		} else {
			OverallResult::Healthy
		}
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
	fn wire_results() {
		assert_eq!(CheckStatus::Pass.wire_result(), "passed");
		assert_eq!(CheckStatus::Skip("r".into()).wire_result(), "skipped");
		assert_eq!(CheckStatus::Warning("r".into()).wire_result(), "warning");
		assert_eq!(CheckStatus::Fail("r".into()).wire_result(), "failed");
		assert_eq!(CheckStatus::Broken("r".into()).wire_result(), "broken");
	}

	#[test]
	fn warning_is_not_fatal() {
		assert!(!CheckStatus::Warning("w".into()).is_fatal());
	}

	#[test]
	fn fail_is_fatal() {
		assert!(CheckStatus::Fail("f".into()).is_fatal());
	}

	#[test]
	fn skip_is_not_fatal() {
		// Skip means "we didn't run this check" — it must not fire alerts,
		// since we have no evidence of unhealth either way.
		let s = CheckStatus::Skip("reason".into());
		assert!(!s.is_fatal());
		assert!(s.is_skip());
	}

	#[test]
	fn broken_is_not_fatal() {
		// Broken means the check itself errored — it says nothing about the
		// system under test, so it must not flag the deployment as failing.
		assert!(!CheckStatus::Broken("reason".into()).is_fatal());
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
	fn overall_from_checks() {
		let healthy = vec![Check::pass("a", "")];
		assert_eq!(OverallResult::from_checks(&healthy), OverallResult::Healthy);

		let degraded = vec![Check::pass("a", ""), Check::warning("b", "", "x")];
		assert_eq!(
			OverallResult::from_checks(&degraded),
			OverallResult::Degraded
		);

		let broken = vec![Check::pass("a", ""), Check::broken("b", "", "x")];
		assert_eq!(OverallResult::from_checks(&broken), OverallResult::Degraded);

		let failing = vec![Check::warning("a", "", "x"), Check::fail("b", "", "y")];
		assert_eq!(OverallResult::from_checks(&failing), OverallResult::Failing);
	}

	#[test]
	fn check_to_wire_pass() {
		let c = Check::pass("db_connect", "ok").with_detail("latency_ms", 3);
		let v = c.to_wire();
		assert_eq!(v["check"], "db_connect");
		assert_eq!(v["result"], "passed");
		assert_eq!(v["latency_ms"], 3);
		assert_eq!(v["summary"], "ok");
		// A pass carries no reason.
		assert!(v.get("reason").is_none());
	}

	#[test]
	fn check_to_wire_statuses() {
		let warn = Check::warning("disk_free", "20% used", "below threshold");
		let v = warn.to_wire();
		assert_eq!(v["result"], "warning");
		// The reason and summary travel to canopy so the *why* is visible off-box.
		assert_eq!(v["summary"], "20% used");
		assert_eq!(v["reason"], "below threshold");
		let fail = Check::fail("disk_free", "1% free", "out of space");
		assert_eq!(fail.to_wire()["result"], "failed");
		assert_eq!(fail.to_wire()["reason"], "out of space");
		let broken = Check::broken("x", "query broken", "no such column");
		assert_eq!(broken.to_wire()["result"], "broken");
		let skip = Check::skip("x", "n/a", "central-only");
		assert_eq!(skip.to_wire()["result"], "skipped");
		assert_eq!(skip.to_wire()["reason"], "central-only");
	}

	#[test]
	fn cap_to_never_raises_severity() {
		// A pass stays a pass regardless of the ceiling.
		assert!(matches!(
			CheckStatus::Pass.cap_to(CheckSeverity::Warn),
			CheckStatus::Pass
		));
		assert!(matches!(
			CheckStatus::Pass.cap_to(CheckSeverity::Fail),
			CheckStatus::Pass
		));
		// A warning is not promoted to a failure by a fail ceiling.
		assert!(matches!(
			CheckStatus::Warning("w".into()).cap_to(CheckSeverity::Fail),
			CheckStatus::Warning(_)
		));
	}

	#[test]
	fn cap_to_lowers_to_the_ceiling() {
		// fail capped at warn becomes a warning, keeping its reason.
		match CheckStatus::Fail("disk full".into()).cap_to(CheckSeverity::Warn) {
			CheckStatus::Warning(r) => assert_eq!(r, "disk full"),
			other => panic!("expected Warning, got {other:?}"),
		}
		// warn and fail capped at skip are silenced, keeping their reason.
		match CheckStatus::Warning("noisy".into()).cap_to(CheckSeverity::Skip) {
			CheckStatus::Skip(r) => assert_eq!(r, "noisy"),
			other => panic!("expected Skip, got {other:?}"),
		}
		match CheckStatus::Fail("noisy".into()).cap_to(CheckSeverity::Skip) {
			CheckStatus::Skip(r) => assert_eq!(r, "noisy"),
			other => panic!("expected Skip, got {other:?}"),
		}
	}

	#[test]
	fn cap_to_leaves_broken_and_skip_untouched() {
		// Broken is a fault in the check itself, not a severity finding, so an
		// operator silencing the check's result must not hide it.
		assert!(matches!(
			CheckStatus::Broken("bad sql".into()).cap_to(CheckSeverity::Skip),
			CheckStatus::Broken(_)
		));
		assert!(matches!(
			CheckStatus::Skip("n/a".into()).cap_to(CheckSeverity::Fail),
			CheckStatus::Skip(_)
		));
	}

	#[test]
	fn broken_round_trips_through_streaming_json() {
		let c = Check::broken("x", "query broken", "no such column");
		let v = c.to_streaming_json();
		assert_eq!(v["status"], "broken");
		assert_eq!(v["reason"], "no such column");
		let back = Check::from_streaming_json(&v, |_| Some("x")).unwrap();
		assert!(matches!(back.status, CheckStatus::Broken(r) if r == "no such column"));
	}
}
