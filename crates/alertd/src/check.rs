use std::collections::HashMap;

use bestool_canopy::schema::{CheckResult, CheckSeverity, HealthCheck, HealthCheckInstance};
use serde_json::{Map, Value, json};

use crate::{
	stat::Stat,
	subject::{ApplicationKind, ApplicationRef, Subject},
};

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

	/// How urgent this status is, for choosing between two that cannot both be
	/// kept: failed over warning over passed over skipped.
	fn urgency(&self) -> u8 {
		match self {
			CheckStatus::Skip(_) => 0,
			CheckStatus::Pass => 1,
			CheckStatus::Warning(_) | CheckStatus::Broken(_) => 2,
			CheckStatus::Fail(_) => 3,
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

/// One occurrence of the condition an instanced check grades: a device, a
/// resource, a mount.
///
/// Canopy grades, silences and presents an instance by itself, which a field
/// inside a single result's detail cannot be. Brokenness is the whole check's,
/// so an instance is passed, warning, failed or skipped and never broken: there
/// is no constructor for it.
///
/// spec: CHK#instances
#[derive(Debug, Clone)]
pub struct Instance {
	/// Names the occurrence itself, never a value it currently has: unique
	/// within the check and the same on every sweep, since Canopy's silences
	/// are kept against it.
	pub key: String,
	pub status: CheckStatus,
	/// How the occurrence is named to an operator; the key when absent.
	pub label: Option<String>,
	pub detail: Map<String, Value>,
}

impl Instance {
	fn new(key: impl Into<String>, status: CheckStatus) -> Self {
		Self {
			key: key.into(),
			status,
			label: None,
			detail: Map::new(),
		}
	}

	pub fn pass(key: impl Into<String>) -> Self {
		Self::new(key, CheckStatus::Pass)
	}

	pub fn skip(key: impl Into<String>, reason: impl Into<String>) -> Self {
		Self::new(key, CheckStatus::Skip(reason.into()))
	}

	pub fn warning(key: impl Into<String>, reason: impl Into<String>) -> Self {
		Self::new(key, CheckStatus::Warning(reason.into()))
	}

	pub fn fail(key: impl Into<String>, reason: impl Into<String>) -> Self {
		Self::new(key, CheckStatus::Fail(reason.into()))
	}

	pub fn with_label(mut self, label: impl Into<String>) -> Self {
		self.label = Some(label.into());
		self
	}

	pub fn with_detail(mut self, key: &str, value: impl Into<Value>) -> Self {
		self.detail.insert(key.to_string(), value.into());
		self
	}

	/// Attach every field of `detail` (a JSON object; anything else attaches
	/// nothing).
	pub fn with_details(mut self, detail: Value) -> Self {
		if let Value::Object(fields) = detail {
			self.detail.extend(fields);
		}
		self
	}

	/// What this instance is called in local output.
	pub fn name(&self) -> &str {
		self.label.as_deref().unwrap_or(&self.key)
	}

	/// The instance's fields on the wire: its own detail, plus the reason a
	/// non-passing one carries. Inserted after the detail so the reserved key
	/// always wins.
	fn wire_detail(&self) -> Map<String, Value> {
		let mut detail = self.detail.clone();
		if let Some(reason) = self.status.reason() {
			detail.insert("reason".into(), reason.into());
		}
		detail
	}

	fn to_wire(&self) -> HealthCheckInstance {
		let mut instance = HealthCheckInstance::builder()
			.result(match self.status {
				CheckStatus::Pass => CheckResult::Passed,
				CheckStatus::Skip(_) => CheckResult::Skipped,
				CheckStatus::Fail(_) => CheckResult::Failed,
				// Unreachable through the constructors; an instance cannot be
				// broken, and a warning is what brokenness counts as.
				CheckStatus::Warning(_) | CheckStatus::Broken(_) => CheckResult::Warning,
			})
			.build();
		instance.detail = self.wire_detail();
		instance.label = self.label.clone();
		instance
	}

	fn from_wire(key: &str, wire: &HealthCheckInstance) -> Self {
		let reason = wire
			.detail
			.get("reason")
			.and_then(Value::as_str)
			.unwrap_or_default()
			.to_string();
		let status = match wire.result {
			CheckResult::Passed => CheckStatus::Pass,
			CheckResult::Skipped => CheckStatus::Skip(reason),
			CheckResult::Failed => CheckStatus::Fail(reason),
			CheckResult::Warning | CheckResult::Broken => CheckStatus::Warning(reason),
		};
		let mut detail = wire.detail.clone();
		detail.remove("reason");
		Self {
			key: key.to_string(),
			status,
			label: wire.label.clone(),
			detail,
		}
	}

	fn to_streaming_json(&self) -> Value {
		let (status, reason) = status_parts(&self.status);
		let mut obj = json!({
			"key": self.key,
			"status": status,
			"detail": Value::Object(self.detail.clone()),
		});
		if let Some(label) = &self.label {
			obj["label"] = label.clone().into();
		}
		if let Some(reason) = reason {
			obj["reason"] = reason.into();
		}
		obj
	}

	fn from_streaming_json(value: &Value) -> Option<Self> {
		let reason = value
			.get("reason")
			.and_then(Value::as_str)
			.map(str::to_string);
		let status = match status_from_parts(value.get("status")?.as_str()?, reason)? {
			// An instance is never broken, whatever a stream claims.
			CheckStatus::Broken(reason) => CheckStatus::Warning(reason),
			status => status,
		};
		Some(Self {
			key: value.get("key")?.as_str()?.to_string(),
			status,
			label: value
				.get("label")
				.and_then(Value::as_str)
				.map(str::to_string),
			detail: value
				.get("detail")
				.and_then(Value::as_object)
				.cloned()
				.unwrap_or_default(),
		})
	}
}

/// The status an instanced check takes from its instances: the most urgent of
/// those not skipped, skipped when every one is, passed when there are none.
///
/// This is the rule Canopy grades the check by, so the two agree. The reason
/// names the degraded instances, for local output; it is not sent.
fn derive_status(instances: &[Instance]) -> CheckStatus {
	let degraded = |wanted: fn(&CheckStatus) -> bool| {
		instances
			.iter()
			.filter(|instance| wanted(&instance.status))
			.map(|instance| match instance.status.reason() {
				Some(reason) if !reason.is_empty() => format!("{}: {reason}", instance.name()),
				_ => instance.name().to_string(),
			})
			.collect::<Vec<_>>()
			.join("; ")
	};

	if instances.iter().any(|i| i.status.is_fatal()) {
		CheckStatus::Fail(degraded(CheckStatus::is_fatal))
	} else if instances
		.iter()
		.any(|i| matches!(i.status, CheckStatus::Warning(_)))
	{
		CheckStatus::Warning(degraded(|s| matches!(s, CheckStatus::Warning(_))))
	} else if !instances.is_empty() && instances.iter().all(|i| i.status.is_skip()) {
		CheckStatus::Skip("every instance is skipped".into())
	} else {
		CheckStatus::Pass
	}
}

/// A headline for an instanced check that has none of its own to give.
fn instances_summary(instances: &[Instance]) -> String {
	let count =
		|wanted: fn(&CheckStatus) -> bool| instances.iter().filter(|i| wanted(&i.status)).count();
	format!(
		"{} instance(s): {} failed, {} warning",
		instances.len(),
		count(CheckStatus::is_fatal),
		count(|s| matches!(s, CheckStatus::Warning(_))),
	)
}

/// The tag and reason a status travels under in the daemon's task stream.
fn status_parts(status: &CheckStatus) -> (&'static str, Option<&str>) {
	match status {
		CheckStatus::Pass => ("pass", None),
		CheckStatus::Skip(r) => ("skip", Some(r.as_str())),
		CheckStatus::Warning(r) => ("warning", Some(r.as_str())),
		CheckStatus::Fail(r) => ("fail", Some(r.as_str())),
		CheckStatus::Broken(r) => ("broken", Some(r.as_str())),
	}
}

fn status_from_parts(tag: &str, reason: Option<String>) -> Option<CheckStatus> {
	Some(match (tag, reason) {
		("pass", _) => CheckStatus::Pass,
		("skip", Some(r)) => CheckStatus::Skip(r),
		("warning", Some(r)) => CheckStatus::Warning(r),
		("fail", Some(r)) => CheckStatus::Fail(r),
		("broken", Some(r)) => CheckStatus::Broken(r),
		_ => return None,
	})
}

/// Result of one healthcheck.
#[derive(Debug, Clone)]
pub struct Check {
	/// Stable identifier, also used as the `check` field on the wire.
	pub name: &'static str,
	pub status: CheckStatus,
	/// Short human-readable description for the CLI output.
	pub summary: String,
	/// The check's fields, sent as its `detail`. For an instanced check, what
	/// its instances share.
	pub details: Map<String, Value>,
	/// The occurrences this check grades, in place of a result of its own, or
	/// `None` for a check with a single result. `Some` of nothing is a check
	/// with no occurrences, which recovers everything Canopy held for it.
	pub instances: Option<Vec<Instance>>,
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
	/// A check with a single result and nothing attached to it yet.
	pub fn new(name: &'static str, status: CheckStatus, summary: impl Into<String>) -> Self {
		Self {
			name,
			status,
			summary: summary.into(),
			details: Map::new(),
			instances: None,
			payload_extras: Map::new(),
			stats: Vec::new(),
		}
	}

	pub fn pass(name: &'static str, summary: impl Into<String>) -> Self {
		Self::new(name, CheckStatus::Pass, summary)
	}

	/// Build a Skip result. The `reason` is kept on the status so the operator
	/// sees *why* the check couldn't be run; the summary is the short headline
	/// shown alongside `SKIP`.
	pub fn skip(name: &'static str, summary: impl Into<String>, reason: impl Into<String>) -> Self {
		Self::new(name, CheckStatus::Skip(reason.into()), summary)
	}

	pub fn warning(
		name: &'static str,
		summary: impl Into<String>,
		reason: impl Into<String>,
	) -> Self {
		Self::new(name, CheckStatus::Warning(reason.into()), summary)
	}

	pub fn fail(name: &'static str, summary: impl Into<String>, reason: impl Into<String>) -> Self {
		Self::new(name, CheckStatus::Fail(reason.into()), summary)
	}

	/// Build a Broken result: the check itself errored or is misconfigured,
	/// which says nothing about the system under test.
	///
	/// Brokenness is the whole check's, so this carries no instances and
	/// Canopy keeps the ones it held.
	pub fn broken(
		name: &'static str,
		summary: impl Into<String>,
		reason: impl Into<String>,
	) -> Self {
		Self::new(name, CheckStatus::Broken(reason.into()), summary)
	}

	/// Build a check reporting its occurrences as instances, in place of a
	/// result. Its status is that of its most urgent instance that is not
	/// skipped, the rule Canopy grades it by.
	///
	/// The summary is the local headline. It is not sent: Canopy writes the
	/// check's message from the graded instances.
	///
	/// spec: CHK#instances
	pub fn instanced(
		name: &'static str,
		summary: impl Into<String>,
		instances: Vec<Instance>,
	) -> Self {
		let status = derive_status(&instances);
		let mut check = Self::new(name, status, summary);
		check.instances = Some(instances);
		check
	}

	/// [`Self::instanced`] with a headline counting the instances.
	pub fn instanced_summarised(name: &'static str, instances: Vec<Instance>) -> Self {
		let summary = instances_summary(&instances);
		Self::instanced(name, summary, instances)
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

	/// Build the typed per-check entry for a target's `health[]`.
	///
	/// Constructed field by field rather than round-tripped through a `Value`:
	/// a check whose details failed to deserialise would otherwise vanish from
	/// the push with no error, silently ceasing to be monitored.
	///
	/// Every field rides in the nested `detail`, never beside the check's name:
	/// canopy refuses a check carrying fields both ways, and one with
	/// instances carries its fields in `detail` only.
	///
	/// A check with a single result carries its summary and, when it is not a
	/// pass, its reason, so an operator can see *why* it warned or failed from
	/// canopy without shelling into the box. An instanced check carries neither:
	/// canopy writes its message from the graded instances.
	///
	/// spec: CHK#reporting-to-canopy
	pub fn to_health_check(&self) -> HealthCheck {
		let mut health = HealthCheck::builder().check(self.name.to_owned()).build();
		health.detail = self.details.clone();

		match &self.instances {
			Some(instances) if !matches!(self.status, CheckStatus::Broken(_)) => {
				let mut wire: HashMap<String, (u8, HealthCheckInstance)> = HashMap::new();
				for instance in instances {
					// Canopy refuses an empty key, and with it the whole push, so
					// a check that produced one is kept visible under a stand-in
					// rather than taking every other check down with it.
					let key = if instance.key.is_empty() {
						"(unnamed)"
					} else {
						&instance.key
					};
					// Two occurrences can share a key, such as certificates for
					// the same names. Canopy takes one, so the worse is the one
					// kept rather than whichever came last.
					let rank = instance.status.urgency();
					match wire.get(key) {
						Some((kept, _)) if *kept >= rank => {}
						_ => {
							wire.insert(key.to_string(), (rank, instance.to_wire()));
						}
					}
				}
				health.instances = Some(
					wire.into_iter()
						.map(|(key, (_, instance))| (key, instance))
						.collect(),
				);
			}
			_ => {
				// After the details, so the reserved keys always win.
				health
					.detail
					.insert("summary".into(), self.summary.clone().into());
				if let Some(reason) = self.status.reason() {
					health.detail.insert("reason".into(), reason.into());
				}
				health.result = Some(match self.status {
					CheckStatus::Pass => CheckResult::Passed,
					CheckStatus::Warning(_) => CheckResult::Warning,
					CheckStatus::Fail(_) => CheckResult::Failed,
					CheckStatus::Broken(_) => CheckResult::Broken,
					CheckStatus::Skip(_) => CheckResult::Skipped,
				});
			}
		}
		health
	}

	/// The per-check entry for the canopy `health[]` array, as JSON, so tests
	/// can read the wire form by key.
	#[cfg(test)]
	pub fn to_wire(&self) -> Value {
		serde_json::to_value(self.to_health_check())
			.expect("a health check is plain strings, maps and enums, which always serialise")
	}

	/// Read a check back from the wire entry a daemon pushed, so a payload held
	/// by one process can be rendered by another.
	///
	/// `name` is the registry's slot for the entry's name. The summary and
	/// reason are read from `detail`, falling back to the flat fields an older
	/// daemon sent beside the name. The check's own details are not
	/// reconstructed; an instance's are. `None` for an entry with neither a
	/// result nor instances.
	pub fn from_wire(name: &'static str, entry: &HealthCheck) -> Option<Self> {
		let text = |key: &str| {
			entry
				.detail
				.get(key)
				.or_else(|| entry.extra.get(key))
				.and_then(Value::as_str)
				.unwrap_or_default()
				.to_string()
		};

		if let Some(instances) = &entry.instances {
			let mut instances: Vec<Instance> = instances
				.iter()
				.map(|(key, instance)| Instance::from_wire(key, instance))
				.collect();
			instances.sort_by(|a, b| a.key.cmp(&b.key));
			return Some(Self::instanced_summarised(name, instances));
		}

		let status = match entry.result.as_ref()? {
			CheckResult::Passed => CheckStatus::Pass,
			CheckResult::Skipped => CheckStatus::Skip(text("reason")),
			CheckResult::Warning => CheckStatus::Warning(text("reason")),
			CheckResult::Failed => CheckStatus::Fail(text("reason")),
			CheckResult::Broken => CheckStatus::Broken(text("reason")),
		};
		Some(Self::new(name, status, text("summary")))
	}

	/// Encode this Check for streaming over the daemon's task endpoint.
	///
	/// Distinct from [`Self::to_wire`]: that one is the canopy-bound payload,
	/// which flattens the status to a `result` string; this one preserves the
	/// full `CheckStatus` enum so consumers can render the same colours and
	/// reason lines as a local sweep.
	pub fn to_streaming_json(&self) -> Value {
		let (status, reason) = status_parts(&self.status);
		let mut obj = json!({
			"name": self.name,
			"status": status,
			"summary": self.summary,
			"details": Value::Object(self.details.clone()),
		});
		if let Some(r) = reason {
			obj["reason"] = Value::String(r.to_string());
		}
		if let Some(instances) = &self.instances {
			obj["instances"] = instances.iter().map(Instance::to_streaming_json).collect();
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
		let status = status_from_parts(status_str, reason)?;
		let summary = value.get("summary")?.as_str()?.to_string();
		let details = value
			.get("details")
			.and_then(Value::as_object)
			.cloned()
			.unwrap_or_default();
		let instances = match value.get("instances") {
			Some(Value::Array(items)) => Some(
				items
					.iter()
					.map(Instance::from_streaming_json)
					.collect::<Option<Vec<_>>>()?,
			),
			_ => None,
		};
		Some(Self {
			details,
			instances,
			..Self::new(name, status, summary)
		})
	}
}

/// One check's result, together with the subject it was filed against.
///
/// A check's name identifies it only within its subject, so the two travel
/// together from the moment the sweep runs it: a machine `foo` and an
/// application `foo` are different checks and must not be collated.
///
/// spec: SUBJ
#[derive(Debug, Clone)]
pub struct CheckOutcome {
	pub subject: Subject,
	pub check: Check,
	/// Whether this result belongs in the wire `health[]` for its subject.
	pub on_wire: bool,
}

impl CheckOutcome {
	/// `subject:name`, how this check is named and selected.
	pub fn qualified_name(&self) -> String {
		self.subject.qualify(self.check.name)
	}

	/// How this result is identified for display: by the instance it came from,
	/// so two clusters' checks of one name are two rows rather than one.
	pub fn row_id(&self) -> String {
		self.subject.identify(self.check.name)
	}

	/// Encode for streaming over the daemon's task endpoint, carrying the
	/// subject so the receiving CLI files the result where the sweep did rather
	/// than guessing from the name.
	pub fn to_streaming_json(&self) -> Value {
		let mut obj = self.check.to_streaming_json();
		obj["subject"] = Value::String(self.subject.slug().to_string());
		if let Some(key) = self.subject.key() {
			obj["subjectKey"] = Value::String(key.to_string());
		}
		obj
	}

	/// Decode a [`Self::to_streaming_json`] payload. Returns `None` for an
	/// unknown check name or subject, or a malformed payload — callers drop
	/// those events.
	pub fn from_streaming_json(
		value: &Value,
		name_resolver: impl FnOnce(&str) -> Option<&'static str>,
	) -> Option<Self> {
		let slug = value.get("subject")?.as_str()?;
		let subject = match value.get("subjectKey").and_then(Value::as_str) {
			Some(key) => Subject::Application(ApplicationRef {
				kind: ApplicationKind::ALL
					.into_iter()
					.find(|kind| kind.type_slug() == slug)?,
				key: key.to_string(),
			}),
			None if slug == "machine" => Subject::Machine,
			None => return None,
		};
		let check = Check::from_streaming_json(value, name_resolver)?;
		Some(Self {
			subject,
			check,
			on_wire: true,
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
	/// Derive the overall result from statuses alone.
	///
	/// Takes statuses rather than whole checks: the verdict reads nothing else,
	/// and callers were cloning every check's details and stats into a throwaway
	/// vector to call this.
	pub fn from_statuses<'a>(statuses: impl IntoIterator<Item = &'a CheckStatus>) -> Self {
		let mut failing = false;
		let mut degraded = false;
		for status in statuses {
			match status {
				CheckStatus::Fail(_) => failing = true,
				CheckStatus::Warning(_) | CheckStatus::Broken(_) => degraded = true,
				CheckStatus::Pass | CheckStatus::Skip(_) => {}
			}
		}
		if failing {
			Self::Failing
		} else if degraded {
			Self::Degraded
		} else {
			Self::Healthy
		}
	}

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
		assert_eq!(v["detail"]["latency_ms"], 3);
		assert_eq!(v["detail"]["summary"], "ok");
		// A pass carries no reason.
		assert!(v["detail"].get("reason").is_none());
	}

	#[test]
	fn check_to_wire_statuses() {
		let warn = Check::warning("disk_free", "20% used", "below threshold");
		let v = warn.to_wire();
		assert_eq!(v["result"], "warning");
		// The reason and summary travel to canopy so the *why* is visible off-box.
		assert_eq!(v["detail"]["summary"], "20% used");
		assert_eq!(v["detail"]["reason"], "below threshold");
		let fail = Check::fail("disk_free", "1% free", "out of space");
		assert_eq!(fail.to_wire()["result"], "failed");
		assert_eq!(fail.to_wire()["detail"]["reason"], "out of space");
		let broken = Check::broken("x", "query broken", "no such column");
		assert_eq!(broken.to_wire()["result"], "broken");
		let skip = Check::skip("x", "n/a", "central-only");
		assert_eq!(skip.to_wire()["result"], "skipped");
		assert_eq!(skip.to_wire()["detail"]["reason"], "central-only");
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

	#[test]
	fn a_plain_check_carries_its_fields_in_detail() {
		let wire = Check::fail("x", "2 stale", "stale")
			.with_detail("threshold", 30)
			.to_wire();
		assert_eq!(wire["check"], "x");
		assert_eq!(wire["result"], "failed");
		assert_eq!(wire["detail"]["threshold"], 30);
		assert_eq!(wire["detail"]["summary"], "2 stale");
		assert_eq!(wire["detail"]["reason"], "stale");
		// Nothing beside the name and result: canopy refuses a check carrying
		// fields both ways.
		let keys: Vec<_> = wire.as_object().unwrap().keys().cloned().collect();
		assert!(
			keys.iter()
				.all(|k| ["check", "result", "detail"].contains(&k.as_str())),
			"unexpected flat fields: {keys:?}"
		);
	}

	#[test]
	fn an_instanced_check_sends_instances_and_no_result_summary_or_reason() {
		let wire = Check::instanced(
			"x",
			"local headline",
			vec![
				Instance::pass("a").with_label("Alpha"),
				Instance::fail("b", "too old").with_detail("minutes", 41),
				Instance::skip("c", "disabled"),
			],
		)
		.with_detail("warn_minutes", 10)
		.to_wire();

		assert!(wire.get("result").is_none());
		assert_eq!(wire["detail"]["warn_minutes"], 10);
		assert!(wire["detail"].get("summary").is_none());
		assert!(wire["detail"].get("reason").is_none());
		let keys: Vec<_> = wire.as_object().unwrap().keys().cloned().collect();
		assert!(
			keys.iter()
				.all(|k| ["check", "detail", "instances"].contains(&k.as_str())),
			"unexpected flat fields: {keys:?}"
		);

		let instances = &wire["instances"];
		assert_eq!(instances["a"]["result"], "passed");
		assert_eq!(instances["a"]["label"], "Alpha");
		assert_eq!(instances["b"]["result"], "failed");
		assert_eq!(instances["b"]["detail"]["minutes"], 41);
		assert_eq!(instances["b"]["detail"]["reason"], "too old");
		assert_eq!(instances["c"]["result"], "skipped");
	}

	#[test]
	fn an_empty_instance_set_is_sent_as_an_empty_object() {
		let wire = Check::instanced("x", "nothing to grade", Vec::new()).to_wire();
		assert_eq!(wire["instances"], json!({}));
		assert!(wire.get("result").is_none());
	}

	#[test]
	fn a_broken_check_sends_no_instances() {
		let mut check = Check::instanced("x", "s", vec![Instance::pass("a")]);
		check.status = CheckStatus::Broken("query failed".into());
		let wire = check.to_wire();
		assert_eq!(wire["result"], "broken");
		assert!(wire.get("instances").is_none());

		let wire = Check::broken("x", "s", "r").to_wire();
		assert_eq!(wire["result"], "broken");
		assert!(wire.get("instances").is_none());
	}

	#[test]
	fn no_instance_is_ever_sent_as_broken() {
		let mut instance = Instance::pass("a");
		instance.status = CheckStatus::Broken("oops".into());
		let wire = Check::instanced("x", "s", vec![instance]).to_wire();
		assert_eq!(wire["instances"]["a"]["result"], "warning");
	}

	#[test]
	fn an_empty_instance_key_does_not_reach_the_wire_empty() {
		let wire = Check::instanced("x", "s", vec![Instance::pass("")]).to_wire();
		assert!(wire["instances"].get("").is_none());
		assert_eq!(wire["instances"]["(unnamed)"]["result"], "passed");
	}

	#[test]
	fn instances_sharing_a_key_keep_the_worst_whatever_the_order() {
		for instances in [
			vec![Instance::fail("a", "bad"), Instance::pass("a")],
			vec![Instance::pass("a"), Instance::fail("a", "bad")],
		] {
			let wire = Check::instanced("x", "s", instances).to_wire();
			assert_eq!(wire["instances"].as_object().unwrap().len(), 1);
			assert_eq!(wire["instances"]["a"]["result"], "failed");
		}
	}

	#[test]
	fn an_instanced_check_takes_the_status_of_its_most_urgent_instance() {
		let status = |instances| Check::instanced("x", "s", instances).status;

		assert!(matches!(status(Vec::new()), CheckStatus::Pass));
		assert!(matches!(
			status(vec![Instance::pass("a")]),
			CheckStatus::Pass
		));
		assert!(matches!(
			status(vec![Instance::pass("a"), Instance::warning("b", "w")]),
			CheckStatus::Warning(_)
		));
		assert!(matches!(
			status(vec![Instance::warning("a", "w"), Instance::fail("b", "f")]),
			CheckStatus::Fail(_)
		));
	}

	#[test]
	fn skipped_instances_do_not_count_and_all_skipped_is_skipped() {
		let status = |instances| Check::instanced("x", "s", instances).status;

		assert!(matches!(
			status(vec![Instance::skip("a", "off"), Instance::pass("b")]),
			CheckStatus::Pass
		));
		assert!(matches!(
			status(vec![Instance::skip("a", "off"), Instance::skip("b", "off")]),
			CheckStatus::Skip(_)
		));
	}

	#[test]
	fn the_derived_reason_names_the_degraded_instances() {
		let check = Check::instanced(
			"x",
			"s",
			vec![
				Instance::fail("dev-1", "41m").with_label("Apia"),
				Instance::fail("dev-2", "35m"),
				Instance::warning("dev-3", "12m"),
				Instance::pass("dev-4"),
			],
		);
		let CheckStatus::Fail(reason) = check.status else {
			panic!("expected a failure");
		};
		assert_eq!(reason, "Apia: 41m; dev-2: 35m");
	}

	#[test]
	fn instances_round_trip_through_streaming_json() {
		let check = Check::instanced(
			"x",
			"s",
			vec![
				Instance::fail("a", "late")
					.with_label("A")
					.with_detail("n", 1),
				Instance::pass("b"),
			],
		);
		let back = Check::from_streaming_json(&check.to_streaming_json(), |_| Some("x")).unwrap();
		let instances = back.instances.expect("instances survive the stream");
		assert_eq!(instances.len(), 2);
		assert_eq!(instances[0].key, "a");
		assert_eq!(instances[0].label.as_deref(), Some("A"));
		assert_eq!(instances[0].detail["n"], 1);
		assert!(matches!(&instances[0].status, CheckStatus::Fail(r) if r == "late"));
		assert!(matches!(back.status, CheckStatus::Fail(_)));
	}

	#[test]
	fn a_wire_entry_reads_back_to_a_check() {
		let plain = Check::warning("x", "headline", "why").to_health_check();
		let back = Check::from_wire("x", &plain).unwrap();
		assert_eq!(back.summary, "headline");
		assert!(matches!(back.status, CheckStatus::Warning(r) if r == "why"));

		let instanced = Check::instanced(
			"x",
			"s",
			vec![Instance::fail("a", "late"), Instance::pass("b")],
		)
		.to_health_check();
		let back = Check::from_wire("x", &instanced).unwrap();
		assert!(matches!(back.status, CheckStatus::Fail(_)));
		assert_eq!(back.instances.unwrap().len(), 2);
	}

	#[test]
	fn a_flat_entry_from_an_older_daemon_still_reads() {
		let mut flat = HealthCheck::builder().check("x".into()).build();
		flat.result = Some(CheckResult::Failed);
		flat.extra.insert("summary".into(), "old".into());
		flat.extra.insert("reason".into(), "flat".into());
		let back = Check::from_wire("x", &flat).unwrap();
		assert_eq!(back.summary, "old");
		assert!(matches!(back.status, CheckStatus::Fail(r) if r == "flat"));
	}
}
