use std::{
	collections::{HashMap, VecDeque},
	time::Duration,
};

use chrono::{DateTime, Utc};

use super::events::Event;

/// In-memory tracker of RDP session state used to detect "kick" events
/// (where a new user's logon happens within `kick_window` of a different
/// user's disconnect).
pub struct Tracker {
	kick_window: Duration,
	sessions: HashMap<u32, SessionState>,
	recent_disconnects: VecDeque<RecentDisconnect>,
}

#[derive(Debug, Clone)]
struct SessionState {
	user: String,
	tailscale: Option<String>,
	connect_time: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct RecentDisconnect {
	when: DateTime<Utc>,
	user: String,
	tailscale: Option<String>,
	connected_for: Duration,
}

/// Result of detecting a fast switch between two users.
#[derive(Debug, Clone)]
pub struct KickDetection {
	pub kicked_user: String,
	#[cfg_attr(not(windows), allow(dead_code))]
	pub kicked_tailscale: Option<String>,
	pub duration: Duration,
}

impl Tracker {
	pub fn new(kick_window: Duration) -> Self {
		Self {
			kick_window,
			sessions: HashMap::new(),
			recent_disconnects: VecDeque::new(),
		}
	}

	/// Process a disconnect/logoff event. Records session duration (when known)
	/// so a subsequent logon can be identified as a kick.
	///
	/// Returns the Tailscale identity that was recorded for this session at
	/// logon time, so the caller can emit it into the audit log. We look it up
	/// here (rather than re-querying Tailscale on the disconnect event) because
	/// at disconnect time the freshest handshake on the tailnet is typically
	/// the *incoming* user whose RDP connection just triggered the kick.
	pub fn on_disconnect(&mut self, ev: &Event) -> Option<String> {
		let prior = self.sessions.remove(&ev.session_id);
		let (user, tailscale, connected_for) = match prior {
			Some(s) => {
				let dur = (ev.time - s.connect_time)
					.to_std()
					.unwrap_or(Duration::ZERO);
				(s.user, s.tailscale, dur)
			}
			None => (ev.user.clone(), None, Duration::ZERO),
		};

		self.recent_disconnects.push_back(RecentDisconnect {
			when: ev.time,
			user,
			tailscale: tailscale.clone(),
			connected_for,
		});
		self.prune(ev.time);
		tailscale
	}

	/// Process a logon/reconnect event with the Tailscale identity resolved for
	/// the incoming user. If a *different* user disconnected within the kick
	/// window, returns a [`KickDetection`] carrying the previously-stored
	/// identity of the kicked user.
	pub fn on_connect(&mut self, ev: &Event, tailscale: Option<String>) -> Option<KickDetection> {
		self.prune(ev.time);

		self.sessions.insert(
			ev.session_id,
			SessionState {
				user: ev.user.clone(),
				tailscale,
				connect_time: ev.time,
			},
		);

		// Most recent *different* user is considered the kicked user.
		let kick = self
			.recent_disconnects
			.iter()
			.rev()
			.find(|d| !same_user(&d.user, &ev.user))
			.map(|d| KickDetection {
				kicked_user: d.user.clone(),
				kicked_tailscale: d.tailscale.clone(),
				duration: d.connected_for,
			});

		if kick.is_some() {
			self.recent_disconnects.clear();
		}

		kick
	}

	fn prune(&mut self, now: DateTime<Utc>) {
		while let Some(front) = self.recent_disconnects.front() {
			if (now - front.when)
				.to_std()
				.map(|d| d > self.kick_window)
				.unwrap_or(false)
			{
				self.recent_disconnects.pop_front();
			} else {
				break;
			}
		}
	}
}

/// RDP user strings are often `DOMAIN\user`; consider two users the same if
/// their username component matches (case-insensitive), since the same human
/// may log in via domain or local account.
fn same_user(a: &str, b: &str) -> bool {
	fn tail(s: &str) -> &str {
		s.rsplit_once('\\').map(|(_, t)| t).unwrap_or(s)
	}
	tail(a).eq_ignore_ascii_case(tail(b))
}

#[cfg(test)]
mod tests {
	use super::{super::events::EventKind, *};

	fn ev(kind: EventKind, session: u32, user: &str, t: &str) -> Event {
		Event {
			kind,
			session_id: session,
			user: user.to_owned(),
			address: Some("100.64.0.1".into()),
			time: t.parse().unwrap(),
			record_id: 0,
		}
	}

	#[test]
	fn detects_kick_within_window() {
		let mut tr = Tracker::new(Duration::from_secs(60));
		tr.on_connect(
			&ev(EventKind::Logon, 2, r"CORP\alice", "2026-04-22T10:00:00Z"),
			Some("alice@bes.au".into()),
		);
		let stored = tr.on_disconnect(&ev(
			EventKind::Disconnect,
			2,
			r"CORP\alice",
			"2026-04-22T10:24:00Z",
		));
		assert_eq!(stored.as_deref(), Some("alice@bes.au"));

		let kick = tr
			.on_connect(
				&ev(EventKind::Logon, 3, r"CORP\bob", "2026-04-22T10:24:10Z"),
				Some("bob@bes.au".into()),
			)
			.expect("should detect kick");
		assert_eq!(kick.kicked_user, r"CORP\alice");
		assert_eq!(kick.kicked_tailscale.as_deref(), Some("alice@bes.au"));
		assert_eq!(kick.duration, Duration::from_secs(24 * 60));
	}

	#[test]
	fn no_kick_when_same_user_reconnects() {
		let mut tr = Tracker::new(Duration::from_secs(60));
		tr.on_connect(
			&ev(EventKind::Logon, 2, r"CORP\alice", "2026-04-22T10:00:00Z"),
			None,
		);
		tr.on_disconnect(&ev(
			EventKind::Disconnect,
			2,
			r"CORP\alice",
			"2026-04-22T10:10:00Z",
		));
		assert!(
			tr.on_connect(
				&ev(EventKind::Reconnect, 2, r"CORP\alice", "2026-04-22T10:10:05Z"),
				None,
			)
			.is_none()
		);
	}

	#[test]
	fn no_kick_outside_window() {
		let mut tr = Tracker::new(Duration::from_secs(30));
		tr.on_connect(
			&ev(EventKind::Logon, 2, r"CORP\alice", "2026-04-22T10:00:00Z"),
			None,
		);
		tr.on_disconnect(&ev(
			EventKind::Disconnect,
			2,
			r"CORP\alice",
			"2026-04-22T10:10:00Z",
		));
		// New user logs in 45s later — outside 30s window.
		assert!(
			tr.on_connect(
				&ev(EventKind::Logon, 3, r"CORP\bob", "2026-04-22T10:10:45Z"),
				None,
			)
			.is_none()
		);
	}

	#[test]
	fn disconnect_of_untracked_session_yields_none() {
		let mut tr = Tracker::new(Duration::from_secs(60));
		// Monitor started mid-session; we never saw the logon.
		let stored = tr.on_disconnect(&ev(
			EventKind::Disconnect,
			2,
			r"CORP\alice",
			"2026-04-22T10:00:00Z",
		));
		assert!(stored.is_none());
	}

	#[test]
	fn matches_same_user_across_domains() {
		assert!(same_user(r"CORP\alice", r"WORKGROUP\alice"));
		assert!(same_user(r"alice", r"CORP\alice"));
		assert!(!same_user(r"CORP\alice", r"CORP\bob"));
	}
}
