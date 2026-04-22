use std::net::IpAddr;

use chrono::{DateTime, Utc};
use miette::{IntoDiagnostic, Result, WrapErr, miette};
use serde::{Deserialize, Serialize};
use tracing::{debug, trace};

/// One decoded TerminalServices-LocalSessionManager event.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
	pub kind: EventKind,
	pub session_id: u32,
	pub user: String,
	pub address: Option<IpAddr>,
	pub time: DateTime<Utc>,
	pub record_id: u64,
}

/// The subset of event IDs we care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
	/// Event ID 21: session logon succeeded.
	Logon,
	/// Event ID 22: shell start notification received.
	ShellStart,
	/// Event ID 23: session logoff succeeded.
	Logoff,
	/// Event ID 24: session has been disconnected.
	Disconnect,
	/// Event ID 25: session reconnection succeeded.
	Reconnect,
}

impl EventKind {
	fn from_id(id: u32) -> Option<Self> {
		Some(match id {
			21 => Self::Logon,
			22 => Self::ShellStart,
			23 => Self::Logoff,
			24 => Self::Disconnect,
			25 => Self::Reconnect,
			_ => return None,
		})
	}

	pub fn as_str(self) -> &'static str {
		match self {
			Self::Logon => "logon",
			Self::ShellStart => "shell_start",
			Self::Logoff => "logoff",
			Self::Disconnect => "disconnect",
			Self::Reconnect => "reconnect",
		}
	}
}

/// Query the Windows event log for TerminalServices session events since
/// `since` (exclusive) and return them parsed and sorted by record id.
///
/// Shells out to `wevtutil qe` — this is the only way to subscribe to the log
/// without unsafe FFI, and is acceptable since we poll on a multi-second cadence.
pub async fn poll_events(since: DateTime<Utc>) -> Result<Vec<Event>> {
	let since_str = since.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
	let query = format!(
		"*[System[(EventID=21 or EventID=22 or EventID=23 or EventID=24 or EventID=25) and TimeCreated[@SystemTime>='{since_str}']]]"
	);

	let xml = run_wevtutil(&query).await?;
	parse_events(&xml)
}

#[cfg(windows)]
async fn run_wevtutil(query: &str) -> Result<String> {
	let log = "Microsoft-Windows-TerminalServices-LocalSessionManager/Operational";
	let query_owned = query.to_owned();
	let output = tokio::task::spawn_blocking(move || {
		duct::cmd!(
			"wevtutil.exe",
			"qe",
			log,
			format!("/q:{query_owned}"),
			"/f:xml",
			"/e:Events",
		)
		.stdout_capture()
		.stderr_capture()
		.run()
	})
	.await
	.into_diagnostic()?
	.into_diagnostic()
	.wrap_err("running wevtutil")?;

	String::from_utf8(output.stdout)
		.into_diagnostic()
		.wrap_err("wevtutil output was not utf-8")
}

#[cfg(not(windows))]
async fn run_wevtutil(_query: &str) -> Result<String> {
	Err(miette!("rdp monitor is only available on Windows"))
}

/// Parse a `<Events>…</Events>` XML blob as produced by `wevtutil qe /e:Events`.
fn parse_events(xml: &str) -> Result<Vec<Event>> {
	#[derive(Debug, Deserialize)]
	struct Events {
		#[serde(default, rename = "Event")]
		event: Vec<RawEvent>,
	}

	let trimmed = xml.trim();
	if trimmed.is_empty() {
		return Ok(Vec::new());
	}

	let events: Events = quick_xml::de::from_str(trimmed)
		.into_diagnostic()
		.wrap_err("parsing event XML")?;

	let mut out = Vec::with_capacity(events.event.len());
	for raw in events.event {
		match decode(raw) {
			Ok(ev) => out.push(ev),
			Err(err) => debug!(?err, "skipping unparseable event"),
		}
	}
	out.sort_by_key(|e| e.record_id);
	trace!(count = out.len(), "decoded events");
	Ok(out)
}

#[derive(Debug, Deserialize)]
struct RawEvent {
	#[serde(rename = "System")]
	system: RawSystem,
	#[serde(rename = "UserData")]
	user_data: Option<RawUserData>,
}

#[derive(Debug, Deserialize)]
struct RawSystem {
	#[serde(rename = "EventID")]
	event_id: RawEventId,
	#[serde(rename = "TimeCreated")]
	time_created: RawTimeCreated,
	#[serde(rename = "EventRecordID")]
	event_record_id: RawScalar<u64>,
}

#[derive(Debug, Deserialize)]
struct RawEventId {
	#[serde(rename = "$text")]
	text: u32,
}

#[derive(Debug, Deserialize)]
struct RawScalar<T> {
	#[serde(rename = "$text")]
	text: T,
}

#[derive(Debug, Deserialize)]
struct RawTimeCreated {
	#[serde(rename = "@SystemTime")]
	system_time: String,
}

#[derive(Debug, Deserialize)]
struct RawUserData {
	#[serde(rename = "EventXML")]
	event_xml: RawEventXml,
}

#[derive(Debug, Deserialize)]
struct RawEventXml {
	#[serde(rename = "User")]
	user: Option<RawScalar<String>>,
	#[serde(rename = "SessionID")]
	session_id: Option<RawScalar<u32>>,
	#[serde(rename = "Address")]
	address: Option<RawScalar<String>>,
}

fn decode(raw: RawEvent) -> Result<Event> {
	let id = raw.system.event_id.text;
	let kind = EventKind::from_id(id).ok_or_else(|| miette!("unexpected event id {id}"))?;
	let inner = raw
		.user_data
		.ok_or_else(|| miette!("event {id} missing UserData"))?
		.event_xml;
	let session_id = inner
		.session_id
		.ok_or_else(|| miette!("event {id} missing SessionID"))?
		.text;
	let user = inner.user.map(|u| u.text).unwrap_or_default();
	let address = inner
		.address
		.map(|a| a.text)
		.filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("LOCAL"))
		.and_then(|s| s.parse::<IpAddr>().ok());
	let time = raw
		.system
		.time_created
		.system_time
		.parse::<DateTime<Utc>>()
		.into_diagnostic()
		.wrap_err("parsing event timestamp")?;
	Ok(Event {
		kind,
		session_id,
		user,
		address,
		time,
		record_id: raw.system.event_record_id.text,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	const SAMPLE: &str = r#"<Events>
<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'>
  <System>
    <Provider Name='Microsoft-Windows-TerminalServices-LocalSessionManager'/>
    <EventID>24</EventID>
    <TimeCreated SystemTime='2026-04-22T10:00:00.000Z'/>
    <EventRecordID>100</EventRecordID>
  </System>
  <UserData>
    <EventXML xmlns='Event_NS'>
      <User>CORP\alice</User>
      <SessionID>2</SessionID>
      <Address>100.64.1.5</Address>
    </EventXML>
  </UserData>
</Event>
<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'>
  <System>
    <Provider Name='Microsoft-Windows-TerminalServices-LocalSessionManager'/>
    <EventID>21</EventID>
    <TimeCreated SystemTime='2026-04-22T10:00:05.000Z'/>
    <EventRecordID>101</EventRecordID>
  </System>
  <UserData>
    <EventXML xmlns='Event_NS'>
      <User>CORP\bob</User>
      <SessionID>3</SessionID>
      <Address>100.64.2.9</Address>
    </EventXML>
  </UserData>
</Event>
</Events>"#;

	#[test]
	fn parses_sample_events() {
		let events = parse_events(SAMPLE).unwrap();
		assert_eq!(events.len(), 2);

		assert_eq!(events[0].kind, EventKind::Disconnect);
		assert_eq!(events[0].session_id, 2);
		assert_eq!(events[0].user, r"CORP\alice");
		assert_eq!(
			events[0].address,
			Some("100.64.1.5".parse::<IpAddr>().unwrap())
		);

		assert_eq!(events[1].kind, EventKind::Logon);
		assert_eq!(events[1].session_id, 3);
		assert_eq!(events[1].user, r"CORP\bob");
	}

	#[test]
	fn empty_output_yields_no_events() {
		assert!(parse_events("").unwrap().is_empty());
	}

	#[test]
	fn logoff_without_address() {
		let xml = r#"<Events>
<Event>
  <System>
    <EventID>23</EventID>
    <TimeCreated SystemTime='2026-04-22T10:00:00.000Z'/>
    <EventRecordID>50</EventRecordID>
  </System>
  <UserData>
    <EventXML>
      <User>CORP\alice</User>
      <SessionID>2</SessionID>
    </EventXML>
  </UserData>
</Event>
</Events>"#;
		let events = parse_events(xml).unwrap();
		assert_eq!(events.len(), 1);
		assert_eq!(events[0].kind, EventKind::Logoff);
		assert!(events[0].address.is_none());
	}
}
