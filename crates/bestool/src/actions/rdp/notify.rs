use std::time::Duration;

use miette::{IntoDiagnostic, Result};
use tauri_winrt_notification::{Duration as ToastDuration, Toast};

/// Raise a Windows toast notification telling the current user that they may
/// have kicked someone out of the server.
///
/// Uses the PowerShell AUMID so no app-registration is required; this is a
/// common pattern for admin tooling that doesn't ship a Start menu shortcut.
pub fn toast_kick(
	kicked_user: &str,
	kicked_tailscale: Option<&str>,
	connected_for: Duration,
) -> Result<()> {
	let who = kicked_tailscale.unwrap_or(kicked_user);
	let mins = connected_for.as_secs() / 60;
	let detail = if mins >= 1 {
		format!("{who} was active for {mins} minute{} — consider letting them know.", plural(mins))
	} else {
		let secs = connected_for.as_secs();
		format!("{who} was active for {secs} seconds — consider letting them know.")
	};

	Toast::new(Toast::POWERSHELL_APP_ID)
		.title("You may have kicked someone off this server")
		.text1(&detail)
		.duration(ToastDuration::Long)
		.show()
		.into_diagnostic()
}

fn plural(n: u64) -> &'static str {
	if n == 1 { "" } else { "s" }
}
