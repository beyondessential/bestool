//! Tailscale boot / startup configuration healthcheck.
//!
//! Tailscale is the remote-access path to these hosts, so it has to come back
//! on its own after a reboot. This check verifies that:
//!   * Linux: `tailscaled.service` is enabled to start at boot.
//!   * Windows: the Tailscale service is set to start automatically, is set to
//!     "Automatic (Delayed Start)", and Tailscale is configured to run
//!     unattended (the `ForceDaemon` preference, surfaced as "Run unattended"
//!     in the GUI) so the tunnel stays up with no user logged in.
//!
//! When Tailscale isn't installed the check skips. A configuration that would
//! leave the host unreachable over tailscale after a reboot — not starting at
//! boot, or (Windows) not running unattended so the tunnel waits for a login —
//! fails, because a reboot would strand the box. The Windows delayed-start
//! preference is a warning instead: a non-delayed automatic service still comes
//! up at boot, so it doesn't cost us access.

#[cfg(any(target_os = "windows", test))]
use serde_json::Value;
#[cfg(target_os = "windows")]
use tokio::process::Command;

use super::SweepContext;
use crate::doctor::check::Check;

const CHECK_NAME: &str = "tailscale_config";

pub async fn run(_ctx: SweepContext) -> Check {
	#[cfg(target_os = "linux")]
	{
		run_linux().await
	}
	#[cfg(target_os = "windows")]
	{
		run_windows().await
	}
	#[cfg(not(any(target_os = "linux", target_os = "windows")))]
	{
		Check::skip(
			CHECK_NAME,
			"tailscale config check skipped on this platform",
			"boot-configuration checks only apply to Linux and Windows",
		)
	}
}

#[cfg(target_os = "linux")]
async fn run_linux() -> Check {
	use bestool_tamanu::systemd;

	const UNIT: &str = "tailscaled.service";

	match systemd::unit_file_exists(UNIT).await {
		Ok(false) => {
			return Check::skip(
				CHECK_NAME,
				"tailscale not installed",
				format!("no {UNIT} unit on this host"),
			);
		}
		Err(err) => {
			return Check::broken(
				CHECK_NAME,
				"could not query systemd",
				format!("checking {UNIT}: {err}"),
			);
		}
		Ok(true) => {}
	}

	match systemd::is_enabled(UNIT).await {
		Ok(true) => {
			Check::pass(CHECK_NAME, format!("{UNIT} enabled at boot")).with_detail("enabled", true)
		}
		Ok(false) => Check::fail(
			CHECK_NAME,
			format!("{UNIT} not enabled at boot"),
			format!(
				"{UNIT} will not start after a reboot, leaving this host unreachable over tailscale; run `systemctl enable {UNIT}`"
			),
		)
		.with_detail("enabled", false),
		Err(err) => Check::broken(
			CHECK_NAME,
			"could not query systemd",
			format!("is-enabled {UNIT}: {err}"),
		),
	}
}

#[cfg(target_os = "windows")]
async fn run_windows() -> Check {
	// The Tailscale installer registers its daemon under this service name.
	const SERVICE: &str = "Tailscale";

	let start = match query_service_start(SERVICE).await {
		ServiceStartQuery::NotInstalled => {
			return Check::skip(
				CHECK_NAME,
				"tailscale not installed",
				format!("no {SERVICE} service registered"),
			);
		}
		ServiceStartQuery::Error(err) => {
			return Check::broken(CHECK_NAME, "could not query tailscale service", err);
		}
		ServiceStartQuery::Config(cfg) => cfg,
	};

	let force_daemon = query_force_daemon().await;

	// Fatal: the host would be unreachable over tailscale after a reboot.
	let mut fatal: Vec<String> = Vec::new();
	// Soft: worth flagging, but doesn't cost us access on reboot.
	let mut soft: Vec<String> = Vec::new();

	if !start.is_auto_start() {
		fatal.push(format!(
			"service is not set to start at boot (START_TYPE={}); a reboot leaves this host unreachable over tailscale",
			start.code
		));
	} else if !start.delayed {
		soft.push("service is not set to Automatic (Delayed Start)".into());
	}
	match force_daemon {
		Some(true) => {}
		Some(false) => fatal.push(
			"not configured to run unattended (ForceDaemon is off); after a reboot the tunnel stays down until a user logs in"
				.into(),
		),
		None => soft.push("could not determine whether it runs unattended".into()),
	}

	let check = if !fatal.is_empty() {
		let reason = fatal
			.iter()
			.chain(soft.iter())
			.cloned()
			.collect::<Vec<_>>()
			.join("; ");
		Check::fail(
			CHECK_NAME,
			format!("{} blocking issue(s)", fatal.len()),
			reason,
		)
	} else if !soft.is_empty() {
		Check::warning(
			CHECK_NAME,
			format!("{} issue(s)", soft.len()),
			soft.join("; "),
		)
	} else {
		Check::pass(
			CHECK_NAME,
			"tailscale starts at boot (delayed) and runs unattended",
		)
	};

	check
		.with_detail("startType", start.code)
		.with_detail("autoStart", start.is_auto_start())
		.with_detail("delayed", start.delayed)
		.with_detail(
			"unattended",
			force_daemon.map(Value::Bool).unwrap_or(Value::Null),
		)
}

#[cfg(target_os = "windows")]
async fn query_service_start(service: &str) -> ServiceStartQuery {
	let output = match Command::new("sc").arg("qc").arg(service).output().await {
		Ok(o) => o,
		Err(err) => {
			return ServiceStartQuery::Error(format!("could not run `sc qc {service}`: {err}"));
		}
	};

	let stdout = String::from_utf8_lossy(&output.stdout);
	if !output.status.success() {
		// 1060 = ERROR_SERVICE_DOES_NOT_EXIST.
		let stderr = String::from_utf8_lossy(&output.stderr);
		if stdout.contains("1060") || stderr.contains("1060") {
			return ServiceStartQuery::NotInstalled;
		}
		return ServiceStartQuery::Error(format!(
			"`sc qc {service}` exited {}: {}",
			output.status,
			stdout.trim()
		));
	}

	match parse_sc_start_type(&stdout) {
		Some(cfg) => ServiceStartQuery::Config(cfg),
		None => ServiceStartQuery::Error(format!(
			"could not parse START_TYPE from `sc qc {service}` output"
		)),
	}
}

#[cfg(target_os = "windows")]
async fn query_force_daemon() -> Option<bool> {
	let output = Command::new("tailscale")
		.arg("debug")
		.arg("prefs")
		.output()
		.await
		.ok()?;
	if !output.status.success() {
		return None;
	}
	parse_force_daemon(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(target_os = "windows")]
enum ServiceStartQuery {
	Config(StartConfig),
	NotInstalled,
	Error(String),
}

#[cfg(any(target_os = "windows", test))]
struct StartConfig {
	/// Numeric `START_TYPE` code from `sc qc`: 2 = automatic, 3 = manual (on
	/// demand), 4 = disabled. The code is locale-independent, unlike the
	/// mnemonic that follows it.
	code: u32,
	/// Whether the service is configured as Automatic (Delayed Start).
	delayed: bool,
}

#[cfg(any(target_os = "windows", test))]
impl StartConfig {
	fn is_auto_start(&self) -> bool {
		self.code == 2
	}
}

/// Parse the `START_TYPE` line of `sc qc <service>` output.
///
/// The line looks like `START_TYPE : 2   AUTO_START  (DELAYED)`. The numeric
/// code is read for the start type (locale-independent), and the `(DELAYED)`
/// marker denotes Automatic (Delayed Start).
#[cfg(any(target_os = "windows", test))]
fn parse_sc_start_type(output: &str) -> Option<StartConfig> {
	let line = output
		.lines()
		.find(|l| l.trim_start().starts_with("START_TYPE"))?;
	let after = line.split(':').nth(1)?;
	let code: u32 = after.split_whitespace().next()?.parse().ok()?;
	let delayed = after.to_ascii_uppercase().contains("DELAYED");
	Some(StartConfig { code, delayed })
}

/// Extract the `ForceDaemon` preference (the "Run unattended" setting) from
/// `tailscale debug prefs` JSON output.
#[cfg(any(target_os = "windows", test))]
fn parse_force_daemon(json: &str) -> Option<bool> {
	let value: Value = serde_json::from_str(json).ok()?;
	value.get("ForceDaemon")?.as_bool()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parse_start_type_auto_delayed() {
		let out = "\
[SC] QueryServiceConfig SUCCESS

SERVICE_NAME: Tailscale
        TYPE               : 10  WIN32_OWN_PROCESS
        START_TYPE         : 2   AUTO_START  (DELAYED)
        ERROR_CONTROL      : 1   NORMAL
";
		let cfg = parse_sc_start_type(out).expect("should parse");
		assert_eq!(cfg.code, 2);
		assert!(cfg.is_auto_start());
		assert!(cfg.delayed);
	}

	#[test]
	fn parse_start_type_auto_not_delayed() {
		let out = "        START_TYPE         : 2   AUTO_START";
		let cfg = parse_sc_start_type(out).expect("should parse");
		assert!(cfg.is_auto_start());
		assert!(!cfg.delayed);
	}

	#[test]
	fn parse_start_type_manual_and_disabled() {
		let manual = parse_sc_start_type("        START_TYPE         : 3   DEMAND_START")
			.expect("should parse");
		assert_eq!(manual.code, 3);
		assert!(!manual.is_auto_start());

		let disabled =
			parse_sc_start_type("        START_TYPE         : 4   DISABLED").expect("should parse");
		assert_eq!(disabled.code, 4);
		assert!(!disabled.is_auto_start());
	}

	#[test]
	fn parse_start_type_missing_or_garbage() {
		assert!(parse_sc_start_type("").is_none());
		assert!(parse_sc_start_type("SERVICE_NAME: Tailscale\n").is_none());
		assert!(parse_sc_start_type("        START_TYPE         : xx").is_none());
	}

	#[test]
	fn parse_force_daemon_reads_bool() {
		assert_eq!(
			parse_force_daemon(r#"{"ForceDaemon": true, "WantRunning": true}"#),
			Some(true)
		);
		assert_eq!(parse_force_daemon(r#"{"ForceDaemon": false}"#), Some(false));
	}

	#[test]
	fn parse_force_daemon_missing_or_invalid() {
		assert_eq!(parse_force_daemon(r#"{"WantRunning": true}"#), None);
		assert_eq!(parse_force_daemon("not json"), None);
		assert_eq!(parse_force_daemon(r#"{"ForceDaemon": "yes"}"#), None);
	}
}
