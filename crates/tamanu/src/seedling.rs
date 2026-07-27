//! Recognising a Seedling host, and driving its operator CLI.
//!
//! On a host that runs the Seedling application orchestrator, Tamanu is a
//! Seedling app rather than a set of units or pm2 processes, so the lifecycle,
//! log, and database commands act through the daemon.
//!
//! They reach it by driving `seedling-ctl`, which holds the invoking operator's
//! own identity and its store of known daemon identities. That keeps a
//! command's authority equal to the authority of the person running it, and
//! means nothing here needs an identity of its own.
//!
//! spec: SEED

use std::path::{Path, PathBuf};

use miette::{IntoDiagnostic, Result, bail, miette};
use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;
use tracing::debug;

use crate::systemd;

/// Where the Seedling package installs the operator CLI.
const CTL_BIN: &str = "/usr/bin/seedling-ctl";

/// The daemon's service unit, installed and enabled by the Seedling package.
const DAEMON_UNIT: &str = "seedling.service";

/// Which runtime owns Tamanu on this host.
#[derive(Debug)]
pub enum Reach {
	/// No Seedling here: act through the host service manager.
	Host,
	/// Act through the Seedling daemon.
	Seedling(Ctl),
	/// A Seedling host whose daemon cannot be driven, and why.
	///
	/// Callers must report this and stop. Falling back to the host service
	/// manager would act on services that aren't the ones the operator means,
	/// reporting success while leaving the running system untouched.
	Unreachable(String),
}

/// Resolve which runtime owns Tamanu on this host.
///
/// Recognition keys off the daemon's installed service rather than off
/// reaching the daemon, so a host whose daemon is stopped or broken is still
/// recognised as a Seedling host.
pub async fn reach() -> Reach {
	if !systemd::unit_file_exists(DAEMON_UNIT)
		.await
		.unwrap_or(false)
	{
		debug!("{DAEMON_UNIT} not installed, using the host service manager");
		return Reach::Host;
	}

	match Ctl::install() {
		Some(ctl) => Reach::Seedling(ctl),
		None => Reach::Unreachable(format!(
			"{DAEMON_UNIT} is installed but {CTL_BIN} is missing, so the daemon can't be driven"
		)),
	}
}

/// A located `seedling-ctl`.
#[derive(Debug, Clone)]
pub struct Ctl {
	bin: PathBuf,
}

/// One entry of the daemon's app list.
#[derive(Debug, Clone, Deserialize)]
pub struct App {
	pub name: String,
	pub status: String,
	/// Active, uncleared faults filed against the app.
	#[serde(default)]
	pub fault_count: u64,
}

impl App {
	/// Whether the app is in a steady state with everything at its desired
	/// lifecycle state.
	///
	/// An app's status arrives lower-cased and underscored (`not_installed`)
	/// while a resource instance's lifecycle arrives capitalised (`Running`),
	/// so this compares without regard to case rather than picking one of the
	/// two spellings and being wrong about the other.
	pub fn running(&self) -> bool {
		self.status.eq_ignore_ascii_case("running")
	}
}

/// What the daemon knows about one app.
///
/// Only the fields the commands report on are modelled; a resource's full
/// definition and the app's parameters are left alone.
#[derive(Debug, Clone, Deserialize)]
pub struct Show {
	pub status: String,
	/// Faults not tied to any one resource, such as script evaluation errors.
	#[serde(default)]
	pub faults: Vec<Value>,
	#[serde(default)]
	pub resources: Vec<Resource>,
}

/// One resource of an app: a deployment, job, ingress, service, or volume.
#[derive(Debug, Clone, Deserialize)]
pub struct Resource {
	pub name: String,
	#[serde(rename = "type")]
	pub kind: String,
	#[serde(default)]
	pub instances: Vec<Instance>,
	#[serde(default)]
	pub faults: Vec<Value>,
}

/// One instance of a resource.
#[derive(Debug, Clone, Deserialize)]
pub struct Instance {
	pub display_name: String,
	pub lifecycle: String,
}

/// The resource type that carries a scale and an update strategy, and so is
/// the unit a restart rolls.
const DEPLOYMENT: &str = "deployment";

/// Resource kinds the daemon can stop and bring back. Services and volumes
/// carry no lifecycle of their own to stop.
const STOPPABLE: [&str; 3] = [DEPLOYMENT, "job", "ingress"];

impl Show {
	/// The app's deployments, which are what a restart rolls: jobs, ingresses,
	/// services, and volumes have no update strategy to follow.
	pub fn deployments(&self) -> impl Iterator<Item = &Resource> {
		self.resources.iter().filter(|r| r.kind == DEPLOYMENT)
	}

	/// The resources a stop acts on, which are those the daemon can later bring
	/// back where it left them.
	pub fn stoppable(&self) -> impl Iterator<Item = &Resource> {
		self.resources
			.iter()
			.filter(|r| STOPPABLE.contains(&r.kind.as_str()))
	}
}

impl Ctl {
	fn install() -> Option<Self> {
		let bin = Path::new(CTL_BIN);
		bin.is_file().then(|| Self {
			bin: bin.to_owned(),
		})
	}

	/// The CLI itself, for callers that need to build their own invocation
	/// (a blocking command, or one that hands over the terminal).
	pub fn bin(&self) -> &Path {
		&self.bin
	}

	/// A command ready to spawn, for callers that stream output or hand the
	/// terminal over rather than collecting a result.
	pub fn command(&self, args: &[&str]) -> Command {
		let mut cmd = Command::new(&self.bin);
		cmd.args(args);
		cmd
	}

	/// Run to completion, collecting stdout.
	async fn output(&self, args: &[&str]) -> Result<Vec<u8>> {
		debug!(?args, "driving seedling-ctl");
		let out = self.command(args).output().await.into_diagnostic()?;
		if !out.status.success() {
			let stderr = String::from_utf8_lossy(&out.stderr);
			bail!(
				"cannot reach the Seedling daemon: `seedling-ctl {}` failed: {}",
				args.join(" "),
				stderr.trim()
			);
		}
		Ok(out.stdout)
	}

	async fn json(&self, args: &[&str]) -> Result<Value> {
		serde_json::from_slice(&self.output(args).await?).into_diagnostic()
	}

	/// Every app the daemon manages.
	pub async fn apps(&self) -> Result<Vec<App>> {
		serde_json::from_value(self.json(&["apps", "list"]).await?).into_diagnostic()
	}

	/// Everything the daemon knows about one app, including its resources.
	pub async fn show(&self, app: &str) -> Result<Show> {
		serde_json::from_value(self.json(&["apps", "show", app]).await?).into_diagnostic()
	}

	/// Bring every stopped resource of an app back to its desired state.
	pub async fn unstop(&self, app: &str) -> Result<()> {
		self.output(&["apps", "unstop", app]).await.map(drop)
	}

	/// Stop one resource, leaving it where [`Ctl::unstop`] can bring it back.
	///
	/// A stop is built from these rather than from uninstalling the app: an
	/// uninstalled app is recovered by installing it again, and an install takes
	/// the parameters the app was installed with, which a stop has no business
	/// knowing or re-supplying.
	pub async fn stop_resource(&self, app: &str, kind: &str, name: &str) -> Result<()> {
		self.output(&["apps", "stop-resource", app, kind, name])
			.await
			.map(drop)
	}

	/// Roll one deployment, following the update strategy it declares.
	pub async fn restart(&self, app: &str, deployment: &str) -> Result<()> {
		self.output(&["apps", "restart", app, deployment])
			.await
			.map(drop)
	}
}

/// Pick the app a command acts on.
///
/// A named app must exist. Without a name, exactly one Tamanu app must be
/// present: none leaves nothing to act on, and several are ambiguous enough
/// that guessing could act on the wrong deployment.
///
/// spec: SEED#targeting-an-application
pub fn target<'a>(apps: &'a [App], named: Option<&str>) -> Result<&'a App> {
	if let Some(name) = named {
		return apps.iter().find(|app| app.name == name).ok_or_else(|| {
			miette!(
				"the Seedling daemon manages no app named {name}{}",
				listing(apps)
			)
		});
	}

	let mut tamanu = apps.iter().filter(|app| app.name.contains("tamanu"));
	match (tamanu.next(), tamanu.next()) {
		(Some(only), None) => Ok(only),
		(None, _) => bail!("the Seedling daemon manages no Tamanu app{}", listing(apps)),
		(Some(one), Some(two)) => bail!(
			"the Seedling daemon manages more than one Tamanu app ({}, {}); name the one to act on",
			one.name,
			two.name
		),
	}
}

fn listing(apps: &[App]) -> String {
	if apps.is_empty() {
		return ", and manages no apps at all".into();
	}
	format!(
		"; it manages: {}",
		apps.iter()
			.map(|a| a.name.as_str())
			.collect::<Vec<_>>()
			.join(", ")
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn app(name: &str) -> App {
		App {
			name: name.into(),
			status: "Running".into(),
			fault_count: 0,
		}
	}

	#[test]
	fn picks_the_only_tamanu_app() {
		let apps = [app("tamanu"), app("caddy")];
		assert_eq!(target(&apps, None).unwrap().name, "tamanu");
	}

	#[test]
	fn picks_a_prefixed_tamanu_app() {
		let apps = [app("tamanu-central"), app("grafana")];
		assert_eq!(target(&apps, None).unwrap().name, "tamanu-central");
	}

	#[test]
	fn no_tamanu_app_lists_what_is_there() {
		let apps = [app("caddy"), app("grafana")];
		let err = format!("{}", target(&apps, None).unwrap_err());
		assert!(err.contains("no Tamanu app"), "{err}");
		assert!(err.contains("caddy"), "{err}");
	}

	#[test]
	fn no_apps_at_all_says_so() {
		let err = format!("{}", target(&[], None).unwrap_err());
		assert!(err.contains("no apps at all"), "{err}");
	}

	#[test]
	fn several_tamanu_apps_require_a_name() {
		let apps = [app("tamanu-central"), app("tamanu-facility")];
		let err = format!("{}", target(&apps, None).unwrap_err());
		assert!(err.contains("more than one"), "{err}");
		assert!(err.contains("name the one"), "{err}");
	}

	#[test]
	fn several_tamanu_apps_resolve_once_named() {
		let apps = [app("tamanu-central"), app("tamanu-facility")];
		assert_eq!(
			target(&apps, Some("tamanu-facility")).unwrap().name,
			"tamanu-facility"
		);
	}

	#[test]
	fn a_named_app_need_not_look_like_tamanu() {
		let apps = [app("tamanu"), app("caddy")];
		assert_eq!(target(&apps, Some("caddy")).unwrap().name, "caddy");
	}

	#[test]
	fn an_absent_name_is_an_error() {
		let apps = [app("tamanu")];
		let err = format!("{}", target(&apps, Some("nope")).unwrap_err());
		assert!(err.contains("no app named nope"), "{err}");
		assert!(err.contains("tamanu"), "{err}");
	}

	#[test]
	fn show_parses_the_daemon_shape_and_picks_deployments() {
		let show: Show = serde_json::from_value(serde_json::json!({
			"status": "Degraded",
			"faults": [],
			"resources": [
				{
					"name": "api",
					"type": "deployment",
					"scale": 2,
					"instances": [
						{ "id": "1", "display_name": "api-1", "lifecycle": "Running" },
						{ "id": "2", "display_name": "api-2", "lifecycle": "Terminating" },
					],
					"faults": [],
					"def": { "container": { "image": "tamanu:latest" } },
				},
				{ "name": "migrate", "type": "job", "instances": [], "faults": [] },
				{ "name": "web", "type": "ingress", "instances": [], "faults": [] },
			],
			"params": [{ "name": "version", "value": "2.60.0", "is_set": true, "secret": false }],
		}))
		.unwrap();

		assert_eq!(show.status, "Degraded");
		let deployments: Vec<&str> = show.deployments().map(|r| r.name.as_str()).collect();
		assert_eq!(deployments, vec!["api"], "jobs and ingresses aren't rolled");

		let api = show.deployments().next().unwrap();
		assert_eq!(api.instances.len(), 2);
		assert_eq!(api.instances[0].display_name, "api-1");
		assert_eq!(api.instances[1].lifecycle, "Terminating");
	}

	#[test]
	fn stoppable_covers_the_kinds_with_a_lifecycle() {
		let show: Show = serde_json::from_value(serde_json::json!({
			"status": "Running",
			"resources": [
				{ "name": "api", "type": "deployment" },
				{ "name": "migrate", "type": "job" },
				{ "name": "web", "type": "ingress" },
				{ "name": "api-svc", "type": "service" },
				{ "name": "data", "type": "volume" },
			],
		}))
		.unwrap();

		let stoppable: Vec<&str> = show.stoppable().map(|r| r.name.as_str()).collect();
		assert_eq!(
			stoppable,
			vec!["api", "migrate", "web"],
			"services and volumes have no lifecycle to stop"
		);
	}

	#[test]
	fn show_tolerates_an_app_with_no_resources() {
		let show: Show =
			serde_json::from_value(serde_json::json!({ "status": "NotInstalled" })).unwrap();
		assert!(show.resources.is_empty());
		assert_eq!(show.deployments().count(), 0);
	}

	#[test]
	fn app_list_parses_the_daemon_shape() {
		// Statuses as the daemon actually sends them: lower-cased and
		// underscored, unlike the capitalised lifecycle on a resource instance.
		let apps: Vec<App> = serde_json::from_value(serde_json::json!([
			{ "name": "postgres", "status": "installing", "fault_count": 2 },
			{ "name": "tamanu-facility", "status": "not_installed", "fault_count": 0 },
			{ "name": "tamanu-central", "status": "running", "fault_count": 0 },
		]))
		.unwrap();
		assert_eq!(apps.len(), 3);
		assert!(!apps[0].running());
		assert!(!apps[1].running());
		assert!(apps[2].running());
		assert_eq!(apps[0].fault_count, 2);
	}

	#[test]
	fn running_ignores_the_casing_difference() {
		let mut app = app("tamanu");
		app.status = "running".into();
		assert!(app.running(), "the daemon lower-cases an app's status");
		app.status = "Running".into();
		assert!(app.running(), "an instance lifecycle is capitalised");
		app.status = "degraded".into();
		assert!(!app.running());
	}
}
