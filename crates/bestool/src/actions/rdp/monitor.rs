use std::{path::PathBuf, time::Duration};

use chrono::{DateTime, Utc};
use clap::Parser;
use miette::{Result, WrapErr};
#[cfg(windows)]
use miette::IntoDiagnostic;
use tracing::{debug, info, warn};

use super::{
	RdpArgs,
	audit::AuditLog,
	events::{Event, EventKind, poll_events},
	state::{KickDetection, Tracker},
	tailscale::{active_peers, whois},
};
use crate::actions::Context;

/// Watch RDP sessions and notify on fast user-switch ("kick").
///
/// Runs a long-lived loop that polls the TerminalServices event log for session
/// connect/disconnect events, cross-references source IPs with Tailscale to
/// identify users, and raises a Windows toast on the incoming session when a
/// different user was connected moments before.
///
/// Intended to run as a Windows service or startup task with sufficient
/// privilege to read the TerminalServices-LocalSessionManager log (typically
/// LocalSystem or Administrators).
#[derive(Debug, Clone, Parser)]
pub struct MonitorArgs {
	/// Path to append-only JSONL audit log of every RDP session event.
	#[arg(long, default_value = r"C:\ProgramData\bestool\rdp-audit.jsonl")]
	pub audit_log: PathBuf,

	/// Seconds between event log polls.
	#[arg(long, default_value_t = 3)]
	pub poll_interval: u64,

	/// Max seconds between a disconnect and a new logon to count as a "kick"
	/// and raise a toast.
	#[arg(long, default_value_t = 60)]
	pub kick_window: u64,

	/// Internal: set when launched by the Windows Service Control Manager.
	/// Routes through the service dispatcher so the SCM sees a properly
	/// lifecycled process. Do not set this by hand — use `rdp service install`.
	#[arg(long, hide = true, default_value_t = false)]
	pub service: bool,
}

pub async fn run(ctx: Context<RdpArgs, MonitorArgs>) -> Result<()> {
	let args = ctx.args_sub;

	if args.service {
		#[cfg(windows)]
		return tokio::task::spawn_blocking(move || super::service::dispatch_service_mode(args))
			.await
			.into_diagnostic()
			.wrap_err("service dispatcher task panicked")?;
		#[cfg(not(windows))]
		return Err(miette::miette!(
			"--service is only valid on Windows; run without it or use `rdp service install`"
		));
	}

	let mut audit = AuditLog::open(&args.audit_log)
		.await
		.wrap_err("opening audit log")?;
	let mut tracker = Tracker::new(Duration::from_secs(args.kick_window));
	let mut since: DateTime<Utc> =
		Utc::now() - chrono::Duration::seconds(args.poll_interval as i64);
	let mut last_record_id: u64 = 0;
	let mut interval = tokio::time::interval(Duration::from_secs(args.poll_interval));
	interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

	info!(
		poll_interval = args.poll_interval,
		kick_window = args.kick_window,
		"starting RDP session monitor"
	);

	loop {
		interval.tick().await;
		let now = Utc::now();
		let events = match poll_events(since).await {
			Ok(ev) => ev,
			Err(err) => {
				warn!(?err, "failed to poll event log; will retry");
				continue;
			}
		};
		since = now;

		for ev in events {
			if ev.record_id <= last_record_id {
				continue;
			}
			last_record_id = ev.record_id;
			handle_event(ev, &mut tracker, &mut audit).await;
		}
	}
}

pub(super) async fn handle_event(ev: Event, tracker: &mut Tracker, audit: &mut AuditLog) {
	match ev.kind {
		EventKind::Logon | EventKind::Reconnect => {
			// Resolve Tailscale identity *now*: at logon time the most recent
			// wireguard handshake on the tailnet is the incoming user's. The
			// identity is then stashed in the tracker so the matching
			// disconnect (e.g. the old user getting kicked) can retrieve it.
			let (tailscale_user, tailscale_source) = resolve_tailscale(&ev).await;
			write_audit(audit, &ev, tailscale_user.as_deref(), tailscale_source).await;

			// Only raise a toast when the *incoming* user is identifiable on
			// Tailscale. A connection from console / LAN / unknown source
			// can't be addressed back to a person, so the notification would
			// have nowhere useful to go.
			if let Some(kick) = tracker.on_connect(&ev, tailscale_user.clone())
				&& tailscale_user.is_some()
			{
				emit_kick(&ev, kick);
			}
		}
		EventKind::Disconnect | EventKind::Logoff => {
			// Don't query Tailscale here: at disconnect time the freshest
			// handshake belongs to the *incoming* user whose RDP connection
			// triggered the kick, not the departing user. Use the identity we
			// stored when this session logged on.
			let stored = tracker.on_disconnect(&ev);
			let source = stored.as_ref().map(|_| "session_tracker");
			write_audit(audit, &ev, stored.as_deref(), source).await;
		}
		EventKind::ShellStart => {}
	}
}

async fn write_audit(
	audit: &mut AuditLog,
	ev: &Event,
	tailscale_user: Option<&str>,
	tailscale_source: Option<&str>,
) {
	if let Err(err) = audit.append(ev, tailscale_user, tailscale_source).await {
		warn!(?err, "audit log write failed");
	}
}

/// Identify the Tailscale user behind an RDP event. Prefers `tailscale whois`
/// on the reported address; falls back to the peer with the most recent
/// wireguard handshake if whois can't resolve the address (common for the
/// Tailscale-over-IPv6 endpoint Windows logs on some configurations).
async fn resolve_tailscale(ev: &Event) -> (Option<String>, Option<&'static str>) {
	if let Some(addr) = &ev.address {
		match whois(addr).await {
			Ok(Some(user)) => return (Some(user), Some("whois")),
			Ok(None) => debug!(addr = %addr, "tailscale whois: peer not found; will try handshake fallback"),
			Err(err) => debug!(?err, addr = %addr, "tailscale whois failed"),
		}
	}

	match active_peers().await {
		Ok(peers) => {
			if let Some(peer) = peers.into_iter().next() {
				let age = (ev.time - peer.last_handshake).num_seconds().abs();
				if age <= HANDSHAKE_WINDOW_SECS {
					debug!(
						peer = %peer.login,
						host = %peer.host_name,
						age_secs = age,
						"tailscale fallback: most-recent peer handshake"
					);
					return (Some(peer.login), Some("peer_handshake"));
				} else {
					debug!(age_secs = age, "most-recent handshake too old for fallback");
				}
			}
		}
		Err(err) => debug!(?err, "tailscale status failed"),
	}

	(None, None)
}

const HANDSHAKE_WINDOW_SECS: i64 = 300;

fn emit_kick(ev: &Event, kick: KickDetection) {
	info!(
		new_user = %ev.user,
		kicked = %kick.kicked_user,
		duration_secs = kick.duration.as_secs(),
		"kick detected; raising toast"
	);

	#[cfg(windows)]
	if let Err(err) =
		super::notify::toast_kick(&kick.kicked_user, kick.kicked_tailscale.as_deref(), kick.duration)
	{
		warn!(?err, "toast failed");
	}

	#[cfg(not(windows))]
	let _ = kick;
}
