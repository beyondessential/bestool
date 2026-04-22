use std::{net::IpAddr, path::PathBuf, time::Duration};

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
	tailscale::whois,
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

	/// Only consider Tailscale source IPs (100.64.0.0/10) for kick detection.
	/// When false, any source IP can trigger the notification.
	#[arg(long, default_value_t = false)]
	pub tailscale_only: bool,

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
			handle_event(ev, &mut tracker, &mut audit, args.tailscale_only).await;
		}
	}
}

pub(super) async fn handle_event(
	ev: Event,
	tracker: &mut Tracker,
	audit: &mut AuditLog,
	tailscale_only: bool,
) {
	let tailscale_user = match &ev.address {
		Some(ip) => whois(ip).await.unwrap_or_else(|err| {
			debug!(?err, ip=%ip, "tailscale whois failed");
			None
		}),
		None => None,
	};

	if let Err(err) = audit.append(&ev, tailscale_user.as_deref()).await {
		warn!(?err, "audit log write failed");
	}

	let is_tailscale = ev.address.map(is_tailscale_ip).unwrap_or(false);

	match ev.kind {
		EventKind::Logon | EventKind::Reconnect => {
			if let Some(kick) = tracker.on_connect(&ev)
				&& (!tailscale_only || is_tailscale)
			{
				emit_kick(&ev, kick);
			}
		}
		EventKind::Disconnect | EventKind::Logoff => {
			tracker.on_disconnect(&ev, tailscale_user);
		}
		EventKind::ShellStart => {}
	}
}

/// Tailscale's CGNAT range is `100.64.0.0/10`.
fn is_tailscale_ip(ip: IpAddr) -> bool {
	match ip {
		IpAddr::V4(v4) => {
			let [a, b, ..] = v4.octets();
			a == 100 && (64..128).contains(&b)
		}
		IpAddr::V6(_) => false,
	}
}

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
