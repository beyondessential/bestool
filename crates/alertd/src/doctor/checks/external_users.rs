//! Doctor check: report interactive (non-system) login sessions and warn when
//! someone's been connected a long time.
//!
//! "External" here means SSH, RDP, or local console logins — real humans, as
//! opposed to systemd services, cron jobs, or sshd worker processes.
//!
//! What we actually care about is *how long a person has been continuously
//! connected*, not how long the OS-level session has existed. On Windows in
//! particular, `quser` reports the *original* logon time of a session, even
//! if it was disconnected for hours then reconnected — leading to spurious
//! "session over 12h" warnings the moment someone reopens an old RDP client.
//!
//! To track presence rather than session age, we observe the currently-active
//! sessions on each doctor run and persist a `first_seen` timestamp per
//! presence key in a small state file. Keys prefer the Tailscale identity (so
//! "is this *person* connected" is what's measured, even if their Windows
//! session ID changes across reconnects); fall back to user@line otherwise.
//!
//! A single threshold: sessions of 12h+ produce a warning (`result: "warning"`
//! on the wire for this check, which never flips the overall result to FAILING).
//! A long-lived session can only ever warn, not fail — so a forgotten RDP
//! session won't take the whole doctor result down.
//!
//! On Linux/macOS we shell out to `who`; on Windows to `quser`. The Tailscale
//! login for each session's source address is looked up via `tailscale whois`
//! so the operator can see which person is behind the IP.

use std::{collections::HashMap, path::PathBuf, time::Duration};

use jiff::{Timestamp, civil::DateTime, tz::TimeZone};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::task::spawn_blocking;
use tracing::{debug, trace, warn};

use super::CheckContext;
use crate::doctor::check::Check;

/// Sessions older than this trigger a check warning (degrades doctor's
/// `external_users` line but does not flip the top-level result).
const WARN_AGE: Duration = Duration::from_secs(12 * 3600);

#[derive(Debug, Clone)]
struct ExternalUser {
	name: String,
	line: String,
	/// Logon time as reported by `who` / `quser`. On Windows this may be the
	/// *original* logon for a session that's since been disconnected and
	/// reconnected, so it isn't a reliable measure of how long the human has
	/// been connected — see the `connected_since` field for that.
	login: Timestamp,
	source: Option<String>,
	tailscale_login: Option<String>,
	/// Windows session ID (the `ID` column from `quser`). `None` outside Windows.
	/// Kept for diagnostics and as a fallback presence-key component.
	session_id: Option<u32>,
	/// When this presence was first observed by us — i.e. the earliest doctor
	/// run that saw this person/session continuously up to and including this
	/// one. Populated after consulting the state file.
	connected_since: Timestamp,
}

pub async fn run(_ctx: CheckContext) -> Check {
	let mut users = match collect_users().await {
		Ok(CollectOutcome::Users(u)) => u,
		Ok(CollectOutcome::Unavailable(reason)) => {
			return Check::skip("external_users", "could not enumerate logins", reason);
		}
		Err(err) => {
			return Check::skip(
				"external_users",
				"could not enumerate logins",
				err.to_string(),
			);
		}
	};

	for user in &mut users {
		if let Some(src) = user.source.as_deref()
			&& looks_like_tailscale_ip(src)
		{
			match tailscale_whois(src).await {
				Ok(login) => user.tailscale_login = login,
				Err(err) => trace!(?err, ip=%src, "tailscale whois failed"),
			}
		}
	}

	let now = Timestamp::now();
	let state_path = state_file_path();
	let prior = state_path.as_deref().map(load_state).unwrap_or_default();
	apply_presence_state(&mut users, &prior, now);
	if let Some(path) = &state_path {
		save_state(path, &snapshot_state(&users));
	}

	if users.is_empty() {
		return Check::pass("external_users", "no interactive users connected")
			.with_detail("count", 0)
			.with_detail("users", Value::Array(Vec::new()));
	}

	let oldest_age = users
		.iter()
		.map(|u| session_age(now, u.connected_since))
		.max()
		.unwrap_or(Duration::ZERO);

	let summary = format!(
		"{} user{}; oldest session {}",
		users.len(),
		if users.len() == 1 { "" } else { "s" },
		humanise_age(oldest_age),
	);
	let user_details = serde_json::Value::Array(users.iter().map(user_to_json).collect());

	let check = if oldest_age >= WARN_AGE {
		Check::warning(
			"external_users",
			summary,
			format!(
				"a session has been connected for over {}h",
				WARN_AGE.as_secs() / 3600
			),
		)
	} else {
		Check::pass("external_users", summary)
	};

	check
		.with_detail("count", users.len())
		.with_detail("users", user_details)
}

fn user_to_json(u: &ExternalUser) -> Value {
	let mut obj = json!({
		"name": u.name,
		"line": u.line,
		"login": u.login.to_string(),
		"connected_since": u.connected_since.to_string(),
	});
	if let Some(src) = &u.source {
		obj["source"] = Value::String(src.clone());
	}
	if let Some(login) = &u.tailscale_login {
		obj["tailscale"] = Value::String(login.clone());
	}
	if let Some(sid) = u.session_id {
		obj["session_id"] = Value::from(sid);
	}
	obj
}

/// State file format: a map from presence key to first-observed timestamp.
///
/// Saved to disk after each check run so subsequent runs can compute "how long
/// has this person been continuously connected" without trusting the
/// upstream-reported logon time.
#[derive(Debug, Default, Serialize, Deserialize)]
struct PresenceState {
	#[serde(default)]
	entries: HashMap<String, Timestamp>,
}

fn presence_key(u: &ExternalUser) -> String {
	if let Some(login) = &u.tailscale_login {
		// Preferred: a person's tailscale identity. This is stable across
		// session reconnects, RDP↔SSH switches, source-IP changes, etc., so
		// the duration we report tracks "how long has this human been
		// connected" rather than "how long has this Windows session existed".
		format!("ts:{login}")
	} else if let Some(sid) = u.session_id {
		// Windows fallback: by session ID. Disc sessions are filtered out
		// upstream, so a disc-then-reconnect produces a *different* session
		// (often) or the same one — we accept the rare false-continuity in
		// that case; better than the always-stale quser LOGON TIME.
		format!("winsid:{}:{}", u.name, sid)
	} else {
		// Unix fallback: by login + tty. Source IP would be more precise but
		// can rotate (e.g. mobile carriers); user+tty pair is what who(1)
		// gives us reliably.
		format!("tty:{}:{}", u.name, u.line)
	}
}

fn apply_presence_state(users: &mut [ExternalUser], prior: &PresenceState, now: Timestamp) {
	for user in users {
		let key = presence_key(user);
		user.connected_since = match prior.entries.get(&key) {
			Some(&earlier) if earlier <= now => earlier,
			// State has a future timestamp (clock skew, manual edit) — fall
			// back to now rather than reporting a negative age.
			Some(_) => now,
			// Either first ever observation of this key, or it dropped out
			// previously and just reappeared (disconnect → reconnect from
			// our point of view). Treat as a fresh connection.
			None => now,
		};
	}
}

fn snapshot_state(users: &[ExternalUser]) -> PresenceState {
	let mut entries = HashMap::with_capacity(users.len());
	for user in users {
		entries.insert(presence_key(user), user.connected_since);
	}
	PresenceState { entries }
}

fn state_file_path() -> Option<PathBuf> {
	dirs::cache_dir().map(|d| d.join("bestool").join("doctor-external-users.json"))
}

fn load_state(path: &std::path::Path) -> PresenceState {
	match std::fs::read(path) {
		Ok(bytes) => match serde_json::from_slice::<PresenceState>(&bytes) {
			Ok(v) => v,
			Err(err) => {
				debug!(%err, ?path, "ignoring unparseable external_users state");
				PresenceState::default()
			}
		},
		Err(err) if err.kind() == std::io::ErrorKind::NotFound => PresenceState::default(),
		Err(err) => {
			debug!(%err, ?path, "could not read external_users state");
			PresenceState::default()
		}
	}
}

fn save_state(path: &std::path::Path, state: &PresenceState) {
	if let Some(parent) = path.parent()
		&& let Err(err) = std::fs::create_dir_all(parent)
	{
		warn!(%err, ?parent, "could not create external_users state dir");
		return;
	}
	let json = match serde_json::to_vec(state) {
		Ok(b) => b,
		Err(err) => {
			warn!(%err, "could not serialise external_users state");
			return;
		}
	};
	let tmp = path.with_extension("json.tmp");
	if let Err(err) = std::fs::write(&tmp, &json) {
		warn!(%err, ?tmp, "could not write external_users state");
		return;
	}
	if let Err(err) = std::fs::rename(&tmp, path) {
		warn!(%err, ?path, "could not rename external_users state");
	}
}

fn session_age(now: Timestamp, login: Timestamp) -> Duration {
	let secs = now.as_second().saturating_sub(login.as_second());
	Duration::from_secs(secs.max(0) as u64)
}

fn humanise_age(d: Duration) -> String {
	let secs = d.as_secs();
	let h = secs / 3600;
	let m = (secs % 3600) / 60;
	if h > 0 {
		format!("{h}h {m}m")
	} else {
		format!("{m}m")
	}
}

/// Outcome of trying to enumerate sessions.
///
/// `Unavailable` is distinct from `Users(empty)`: on Windows, `quser` exits
/// non-zero (typically with "No User exists for *") both when there genuinely
/// are no sessions *and* when the caller doesn't have the privilege to list
/// them. Treating both the same way silently turned a permission failure into
/// a falsely cheerful PASS, which is the opposite of what the operator needs.
enum CollectOutcome {
	Users(Vec<ExternalUser>),
	Unavailable(String),
}

#[cfg(unix)]
async fn collect_users() -> miette::Result<CollectOutcome> {
	let output = spawn_blocking(|| {
		duct::cmd!("who")
			.stdout_capture()
			.stderr_capture()
			.unchecked()
			.run()
	})
	.await
	.map_err(|e| miette::miette!("running who: {e}"))?
	.map_err(|e| miette::miette!("running who: {e}"))?;

	if !output.status.success() {
		let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
		debug!(status = ?output.status, %stderr, "who returned non-zero");
		return Ok(CollectOutcome::Unavailable(format!(
			"who returned non-zero{}",
			if stderr.is_empty() {
				String::new()
			} else {
				format!(": {stderr}")
			}
		)));
	}

	let text = String::from_utf8_lossy(&output.stdout);
	Ok(CollectOutcome::Users(parse_who(&text)))
}

#[cfg(windows)]
async fn collect_users() -> miette::Result<CollectOutcome> {
	let output = spawn_blocking(|| {
		duct::cmd!("quser")
			.stdout_capture()
			.stderr_capture()
			.unchecked()
			.run()
	})
	.await
	.map_err(|e| miette::miette!("running quser: {e}"))?
	.map_err(|e| miette::miette!("running quser: {e}"))?;

	let stderr = String::from_utf8_lossy(&output.stderr);
	// `quser` returns exit code 1 + "No User exists for *" when the session
	// list is empty, but it *also* returns non-zero when access is denied.
	// We treat the "empty list" message as a real empty, and anything else
	// non-zero as a skip-worthy "we couldn't determine the list".
	if !output.status.success() {
		debug!(
			status = ?output.status,
			%stderr,
			"quser returned non-zero"
		);
		if stderr.to_lowercase().contains("no user exists for *") {
			return Ok(CollectOutcome::Users(Vec::new()));
		}
		return Ok(CollectOutcome::Unavailable(format!(
			"quser returned non-zero{}",
			if stderr.trim().is_empty() {
				String::new()
			} else {
				format!(": {}", stderr.trim())
			}
		)));
	}

	let text = String::from_utf8_lossy(&output.stdout);
	Ok(CollectOutcome::Users(parse_quser(&text)))
}

#[cfg(not(any(unix, windows)))]
async fn collect_users() -> miette::Result<CollectOutcome> {
	Ok(CollectOutcome::Unavailable(
		"session enumeration not implemented for this platform".into(),
	))
}

/// Parse `who` output into structured sessions.
///
/// `who` is GNU coreutils' default-flag output, which is whitespace-aligned
/// columns of:
///
/// ```text
/// NAME    LINE         YYYY-MM-DD HH:MM (COMMENT)?
/// ```
///
/// Some lines may have an `-u`-style idle/pid pair in the middle; we tolerate
/// that by walking tokens and locating the date by its `YYYY-MM-DD` shape.
#[cfg(any(test, unix))]
fn parse_who(text: &str) -> Vec<ExternalUser> {
	let mut out = Vec::new();
	for line in text.lines() {
		let line = line.trim_end();
		if line.is_empty() {
			continue;
		}

		// Pull off the trailing "(...)" comment, if any, so it doesn't get
		// caught up in whitespace splitting (it can contain `(tmux(123).%1)`
		// style nested parens — take the *first* '(' on the line).
		let (head, comment) = if let Some(open) = line.find('(')
			&& line.ends_with(')')
		{
			(
				line[..open].trim_end(),
				Some(&line[open + 1..line.len() - 1]),
			)
		} else {
			(line, None)
		};

		let tokens: Vec<&str> = head.split_whitespace().collect();
		if tokens.len() < 4 {
			trace!(?line, "skipping unparseable who line");
			continue;
		}

		let name = tokens[0].to_string();
		let tty = tokens[1].to_string();
		// Find the YYYY-MM-DD token (allow a possible "+" idle prefix etc.).
		let date_idx = tokens
			.iter()
			.skip(2)
			.position(|t| looks_like_iso_date(t))
			.map(|i| i + 2);
		let Some(date_idx) = date_idx else {
			trace!(?line, "no date column in who line");
			continue;
		};
		if date_idx + 1 >= tokens.len() {
			trace!(?line, "no time column in who line");
			continue;
		}
		let date_str = tokens[date_idx];
		let time_str = tokens[date_idx + 1];

		let Some(login) = parse_local_datetime(date_str, time_str) else {
			trace!(?date_str, ?time_str, "could not parse login time");
			continue;
		};

		// Treat "(local)" or empty comments as "no remote source"; otherwise
		// the comment is typically the IP or hostname of an SSH client (or
		// the tmux session ID, which we keep so the operator can see it).
		let source = comment
			.map(str::trim)
			.filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("local"))
			.map(str::to_owned);

		out.push(ExternalUser {
			name,
			line: tty,
			login,
			source,
			tailscale_login: None,
			session_id: None,
			// Placeholder; populated later by `apply_presence_state`.
			connected_since: login,
		});
	}
	out
}

#[cfg(any(test, unix))]
fn looks_like_iso_date(tok: &str) -> bool {
	let bytes = tok.as_bytes();
	bytes.len() == 10
		&& bytes[4] == b'-'
		&& bytes[7] == b'-'
		&& bytes[..4].iter().all(u8::is_ascii_digit)
		&& bytes[5..7].iter().all(u8::is_ascii_digit)
		&& bytes[8..10].iter().all(u8::is_ascii_digit)
}

#[cfg(any(test, unix))]
fn parse_local_datetime(date: &str, time: &str) -> Option<Timestamp> {
	// `who` emits `HH:MM`; some platforms include `:SS`. Try both.
	for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
		if let Ok(dt) = DateTime::strptime(fmt, format!("{date} {time}"))
			&& let Ok(zoned) = dt.to_zoned(TimeZone::system())
		{
			return Some(zoned.timestamp());
		}
	}
	None
}

/// Parse `quser` (Windows Terminal Services) output.
///
/// Sample:
///
/// ```text
///  USERNAME              SESSIONNAME        ID  STATE   IDLE TIME  LOGON TIME
/// >administrator         console             1  Active  none       5/21/2026 4:00 AM
///  bob                   rdp-tcp#0           2  Active  10:30      5/21/2026 3:55 AM
/// ```
///
/// `quser` doesn't expose the client IP for RDP sessions, so `source` is left
/// empty here; the operator can still see the SESSIONNAME (e.g. `rdp-tcp#0`)
/// in the `line` column to tell RDP from console sessions.
#[cfg(any(test, windows))]
fn parse_quser(text: &str) -> Vec<ExternalUser> {
	let mut out = Vec::new();
	let mut lines = text.lines();

	// First non-empty line is the header. Skip it.
	let Some(_header) = lines.find(|l| !l.trim().is_empty()) else {
		return out;
	};

	for line in lines {
		let trimmed = line.trim_start_matches('>').trim_end();
		if trimmed.is_empty() {
			continue;
		}

		let tokens: Vec<&str> = trimmed.split_whitespace().collect();
		if tokens.len() < 6 {
			trace!(?line, "skipping unparseable quser line");
			continue;
		}
		// Two formats: with a SESSIONNAME column (e.g. "rdp-tcp#0"), or
		// without one (disconnected sessions). Detect by whether the second
		// column parses as a session id (integer): if so, SESSIONNAME is
		// absent; otherwise SESSIONNAME is in the second column and ID is
		// in the third. STATE follows ID.
		let (name, sessionname, sid, state) = if let Ok(sid) = tokens[1].parse::<u32>() {
			(tokens[0], None, sid, tokens[2])
		} else if tokens.len() >= 7
			&& let Ok(sid) = tokens[2].parse::<u32>()
		{
			(tokens[0], Some(tokens[1]), sid, tokens[3])
		} else {
			trace!(?line, "could not locate session id column in quser line");
			continue;
		};
		// Skip non-active sessions: disconnected ones are stale login state
		// the OS hasn't cleaned up yet, not a real connected user.
		if !state.eq_ignore_ascii_case("Active") {
			trace!(?line, ?state, "skipping non-active quser session");
			continue;
		}
		let logon_tokens = &tokens[tokens.len().saturating_sub(3)..];
		if logon_tokens.len() != 3 {
			continue;
		}
		let logon_str = format!(
			"{} {} {}",
			logon_tokens[0], logon_tokens[1], logon_tokens[2]
		);
		let Some(login) = parse_quser_logon(&logon_str) else {
			trace!(logon_str, "could not parse quser logon time");
			continue;
		};

		out.push(ExternalUser {
			name: name.to_string(),
			line: sessionname.unwrap_or("(disconnected)").to_string(),
			login,
			source: None,
			tailscale_login: None,
			session_id: Some(sid),
			// Placeholder; populated later by `apply_presence_state`.
			connected_since: login,
		});
	}
	out
}

#[cfg(any(test, windows))]
fn parse_quser_logon(s: &str) -> Option<Timestamp> {
	// `quser` formats as "M/D/YYYY H:MM AM/PM" — no leading zero on month,
	// day, or hour. jiff's `strptime` accepts those without zero-padding via
	// `%-m`/`%-d`/`%-H`.
	for fmt in [
		"%-m/%-d/%Y %-I:%M %p",
		"%m/%d/%Y %I:%M %p",
		"%-m/%-d/%Y %-H:%M",
		"%Y-%m-%d %H:%M",
	] {
		if let Ok(dt) = DateTime::strptime(fmt, s)
			&& let Ok(zoned) = dt.to_zoned(TimeZone::system())
		{
			return Some(zoned.timestamp());
		}
	}
	None
}

/// Recognise the address ranges Tailscale issues to peers:
///   * IPv4 CGNAT: `100.64.0.0/10`
///   * IPv6 ULA:   `fd7a:115c:a1e0::/48`
///
/// Outside those ranges, `tailscale whois` wouldn't return a user, so the
/// subprocess cost isn't worth paying.
fn looks_like_tailscale_ip(s: &str) -> bool {
	use std::net::IpAddr;

	let s = s.split('%').next().unwrap_or(s);
	let Ok(ip) = s.parse::<IpAddr>() else {
		return false;
	};
	match ip {
		IpAddr::V4(v4) => is_tailscale_v4(v4),
		IpAddr::V6(v6) => is_tailscale_v6(v6),
	}
}

fn is_tailscale_v4(ip: std::net::Ipv4Addr) -> bool {
	let [a, b, _, _] = ip.octets();
	a == 100 && (64..=127).contains(&b)
}

fn is_tailscale_v6(ip: std::net::Ipv6Addr) -> bool {
	let s = ip.segments();
	s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
}

async fn tailscale_whois(addr: &str) -> miette::Result<Option<String>> {
	let stripped = addr.split('%').next().unwrap_or(addr).to_owned();
	let output = spawn_blocking(move || {
		duct::cmd!("tailscale", "whois", "--json", &stripped)
			.stdout_capture()
			.stderr_capture()
			.unchecked()
			.run()
	})
	.await
	.map_err(|e| miette::miette!("running tailscale whois: {e}"))?
	.map_err(|e| miette::miette!("running tailscale whois: {e}"))?;

	if !output.status.success() {
		return Ok(None);
	}

	let parsed: serde_json::Value = match serde_json::from_slice(&output.stdout) {
		Ok(v) => v,
		Err(err) => {
			warn!(?err, "failed to parse tailscale whois JSON");
			return Ok(None);
		}
	};

	// Skip the `tagged-devices` synthetic login — it's not a human and not
	// useful in the user-facing report.
	let login = parsed["UserProfile"]["LoginName"]
		.as_str()
		.or_else(|| parsed["UserProfile"]["DisplayName"].as_str())
		.map(str::to_owned)
		.filter(|l| !l.eq_ignore_ascii_case("tagged-devices"));
	Ok(login)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parses_simple_who_line() {
		let out = parse_who(
			"alice    pts/0        2026-05-20 12:34 (203.0.113.5)\n\
			 bob      tty1         2026-05-20 17:18 (local)\n",
		);
		assert_eq!(out.len(), 2);
		assert_eq!(out[0].name, "alice");
		assert_eq!(out[0].line, "pts/0");
		assert_eq!(out[0].source.as_deref(), Some("203.0.113.5"));
		assert!(out[1].source.is_none(), "(local) should drop source");
	}

	#[test]
	fn parses_who_dash_u_line() {
		let out =
			parse_who("felix    pts/4        2026-05-20 17:24 10:05       15891 (tmux(6522).%1)\n");
		assert_eq!(out.len(), 1);
		assert_eq!(out[0].name, "felix");
		assert_eq!(out[0].line, "pts/4");
		assert_eq!(out[0].source.as_deref(), Some("tmux(6522).%1"));
	}

	#[test]
	fn parses_who_without_comment() {
		let out = parse_who("felix    tty1         2026-05-20 17:18\n");
		assert_eq!(out.len(), 1);
		assert!(out[0].source.is_none());
	}

	#[test]
	fn skips_malformed_who_lines() {
		let out = parse_who("garbage\n\n   \nstill garbage\n");
		assert!(out.is_empty());
	}

	#[test]
	fn parses_quser_console_and_rdp() {
		let text = " USERNAME              SESSIONNAME        ID  STATE   IDLE TIME  LOGON TIME\n\
		            >administrator         console             1  Active  none       5/21/2026 4:00 AM\n\
		             bob                   rdp-tcp#0           2  Active  10:30      5/21/2026 3:55 AM\n";
		let out = parse_quser(text);
		assert_eq!(out.len(), 2);
		assert_eq!(out[0].name, "administrator");
		assert_eq!(out[0].line, "console");
		assert_eq!(out[1].name, "bob");
		assert_eq!(out[1].line, "rdp-tcp#0");
	}

	#[test]
	fn skips_quser_disconnected_session() {
		// `quser` omits SESSIONNAME for disconnected sessions: the ID column
		// then sits where SESSIONNAME used to. These aren't active users —
		// the OS just hasn't cleaned the session up yet.
		let text = " USERNAME              SESSIONNAME        ID  STATE   IDLE TIME  LOGON TIME\n\
		             dave                                      3  Disc    .          5/21/2026 2:00 AM\n";
		let out = parse_quser(text);
		assert!(out.is_empty());
	}

	#[test]
	fn keeps_only_active_quser_sessions() {
		let text = " USERNAME              SESSIONNAME        ID  STATE   IDLE TIME  LOGON TIME\n\
		            >administrator         console             1  Active  none       5/21/2026 4:00 AM\n\
		             bob                   rdp-tcp#0           2  Active  10:30      5/21/2026 3:55 AM\n\
		             besd                                      3  Disc    .          5/22/2026 4:17 PM\n";
		let out = parse_quser(text);
		assert_eq!(out.len(), 2);
		assert!(out.iter().all(|u| u.name != "besd"));
	}

	#[test]
	fn looks_like_tailscale_ip_handles_cgnat_range() {
		assert!(looks_like_tailscale_ip("100.64.0.1"));
		assert!(looks_like_tailscale_ip("100.127.255.255"));
		assert!(looks_like_tailscale_ip("100.100.0.5%eth0"));
		assert!(!looks_like_tailscale_ip("203.0.113.5"));
		assert!(!looks_like_tailscale_ip("100.63.0.1"));
		assert!(!looks_like_tailscale_ip("100.128.0.1"));
		assert!(!looks_like_tailscale_ip("not-an-ip"));
	}

	#[test]
	fn looks_like_tailscale_ip_handles_ipv6_ula() {
		// Standard Tailscale IPv6 ULA prefix is fd7a:115c:a1e0::/48.
		assert!(looks_like_tailscale_ip("fd7a:115c:a1e0::1"));
		assert!(looks_like_tailscale_ip(
			"fd7a:115c:a1e0:ab12:1234:5678:9abc:def0"
		));
		assert!(looks_like_tailscale_ip("fd7a:115c:a1e0::1%eth0"));
		// Different ULA prefix → not Tailscale.
		assert!(!looks_like_tailscale_ip("fd00::1"));
		// Public IPv6 → definitely not.
		assert!(!looks_like_tailscale_ip("2001:db8::1"));
		// Loopback / link-local → not.
		assert!(!looks_like_tailscale_ip("::1"));
		assert!(!looks_like_tailscale_ip("fe80::1%eth0"));
	}

	#[test]
	fn session_age_clamps_at_zero() {
		let now = Timestamp::from_second(1000).unwrap();
		let future = Timestamp::from_second(2000).unwrap();
		assert_eq!(session_age(now, future), Duration::ZERO);
	}

	#[test]
	fn session_age_computes_positive_delta() {
		let earlier = Timestamp::from_second(1000).unwrap();
		let now = Timestamp::from_second(1000 + 3600).unwrap();
		assert_eq!(session_age(now, earlier), Duration::from_secs(3600));
	}

	#[test]
	fn parses_quser_captures_session_id() {
		let text = " USERNAME              SESSIONNAME        ID  STATE   IDLE TIME  LOGON TIME\n\
		            >administrator         console             1  Active  none       5/21/2026 4:00 AM\n\
		             bob                   rdp-tcp#2           2  Active  10:30      5/21/2026 3:55 AM\n";
		let out = parse_quser(text);
		assert_eq!(out.len(), 2);
		assert_eq!(out[0].session_id, Some(1));
		assert_eq!(out[1].session_id, Some(2));
	}

	fn mk_user(name: &str, line: &str, ts: i64) -> ExternalUser {
		let login = Timestamp::from_second(ts).unwrap();
		ExternalUser {
			name: name.into(),
			line: line.into(),
			login,
			source: None,
			tailscale_login: None,
			session_id: None,
			connected_since: login,
		}
	}

	fn mk_user_ts(name: &str, login_ts: &str, ts_login: Option<&str>) -> ExternalUser {
		ExternalUser {
			name: name.into(),
			line: "rdp-tcp#1".into(),
			login: login_ts.parse().unwrap(),
			source: None,
			tailscale_login: ts_login.map(str::to_owned),
			session_id: Some(1),
			connected_since: login_ts.parse().unwrap(),
		}
	}

	#[test]
	fn presence_key_prefers_tailscale_identity() {
		let u = mk_user_ts("besd", "2026-05-22T16:17:00Z", Some("alice@example.com"));
		assert_eq!(presence_key(&u), "ts:alice@example.com");
	}

	#[test]
	fn presence_key_falls_back_to_session_id_on_windows() {
		let u = mk_user_ts("besd", "2026-05-22T16:17:00Z", None);
		assert_eq!(presence_key(&u), "winsid:besd:1");
	}

	#[test]
	fn presence_key_falls_back_to_user_at_line_otherwise() {
		let u = mk_user("alice", "pts/0", 1000);
		assert_eq!(presence_key(&u), "tty:alice:pts/0");
	}

	#[test]
	fn apply_presence_state_preserves_earlier_first_seen() {
		// A user that's been seen before should keep their earlier
		// connected_since — that's the whole point.
		let mut users = vec![mk_user_ts(
			"besd",
			"2026-05-22T16:17:00Z",
			Some("alice@example.com"),
		)];
		let now: Timestamp = "2026-05-23T04:47:00Z".parse().unwrap();
		let earlier: Timestamp = "2026-05-23T04:30:00Z".parse().unwrap();
		let mut state = PresenceState::default();
		state.entries.insert("ts:alice@example.com".into(), earlier);

		apply_presence_state(&mut users, &state, now);
		assert_eq!(users[0].connected_since, earlier);
	}

	#[test]
	fn apply_presence_state_uses_now_for_unseen_keys() {
		// A user not in prior state is brand new; connected_since should be
		// "now", not whatever the upstream-reported logon was.
		let mut users = vec![mk_user_ts(
			"besd",
			"2026-05-22T16:17:00Z",
			Some("alice@example.com"),
		)];
		let now: Timestamp = "2026-05-23T04:47:00Z".parse().unwrap();
		apply_presence_state(&mut users, &PresenceState::default(), now);
		assert_eq!(users[0].connected_since, now);
	}

	#[test]
	fn snapshot_state_only_captures_currently_present_users() {
		let users = vec![mk_user_ts(
			"besd",
			"2026-05-22T16:17:00Z",
			Some("alice@example.com"),
		)];
		// A second, no-longer-present user in the prior state shouldn't be
		// carried forward in the snapshot.
		let snap = snapshot_state(&users);
		assert_eq!(snap.entries.len(), 1);
		assert!(snap.entries.contains_key("ts:alice@example.com"));
	}

	#[test]
	fn apply_presence_state_clamps_future_first_seen_to_now() {
		// Defensive: if the state somehow has a timestamp ahead of now (clock
		// skew, manual edit), don't report a negative age.
		let mut users = vec![mk_user_ts(
			"besd",
			"2026-05-22T16:17:00Z",
			Some("alice@example.com"),
		)];
		let now: Timestamp = "2026-05-23T04:47:00Z".parse().unwrap();
		let future: Timestamp = "2030-01-01T00:00:00Z".parse().unwrap();
		let mut state = PresenceState::default();
		state.entries.insert("ts:alice@example.com".into(), future);
		apply_presence_state(&mut users, &state, now);
		assert_eq!(users[0].connected_since, now);
	}

	#[test]
	fn humanise_age_formats_h_m() {
		assert_eq!(humanise_age(Duration::from_secs(0)), "0m");
		assert_eq!(humanise_age(Duration::from_secs(59)), "0m");
		assert_eq!(humanise_age(Duration::from_secs(60)), "1m");
		assert_eq!(humanise_age(Duration::from_secs(3600)), "1h 0m");
		assert_eq!(humanise_age(Duration::from_secs(3600 * 25 + 60)), "25h 1m");
	}
}
