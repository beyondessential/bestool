//! Speaking a Seedling host's operator interface.
//!
//! On a host that runs the Seedling application orchestrator, Tamanu is a
//! Seedling app rather than a set of units or pm2 processes, so the lifecycle,
//! log, and database commands act through the daemon.
//!
//! They present the identity of the operator who invoked them where that
//! operator has one, so a command's authority is the authority of the person
//! running it. Where they don't, they fall back to the identity the Seedling
//! installation authorised for this host, which is what lets them work on a
//! freshly provisioned host with nothing configured for any operator.
//!
//! spec: SEED

use std::{
	io,
	net::{IpAddr, Ipv6Addr, SocketAddr},
	path::{Path, PathBuf},
};

use miette::{IntoDiagnostic, Result, bail, miette};
use seedling_protocol::{
	actor::Actor,
	client::{ClientAuth, OiClient},
	keys::ClientIdentity,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::debug;

use super::{
	DAEMON_UNIT, DATA_DIR, HostIdentity, elevate, fingerprint_path, host_identity, host_key_path,
};
use crate::systemd;

/// The port the daemon's operator interface listens on for local clients.
const OI_PORT: u16 = 7891;

/// Which runtime owns Tamanu on this host.
pub enum Reach {
	/// No Seedling here: act through the host service manager.
	Host,
	/// A Seedling host, connected to its daemon.
	Seedling(Box<Oi>),
	/// A Seedling host whose daemon cannot be reached, and why.
	///
	/// Callers must report this and stop. Falling back to the host service
	/// manager would act on services that aren't the ones the operator means,
	/// reporting success while leaving the running system untouched.
	Unreachable(String),
}

/// Resolve which runtime owns Tamanu on this host.
///
/// Recognition keys off the daemon's installed service rather than off reaching
/// the daemon, so a host whose daemon is stopped or broken is still recognised
/// as a Seedling host.
pub async fn reach() -> Reach {
	if !systemd::unit_file_exists(DAEMON_UNIT)
		.await
		.unwrap_or(false)
	{
		debug!("{DAEMON_UNIT} not installed, using the host service manager");
		return Reach::Host;
	}

	match Oi::open(Path::new(DATA_DIR)).await {
		Ok(oi) => Reach::Seedling(Box::new(oi)),
		Err(err) => Reach::Unreachable(format!("{err}")),
	}
}

/// A connection to the daemon's operator interface.
pub struct Oi {
	client: OiClient,
}

impl Oi {
	/// Connect to the daemon, verifying it against the identity it published in
	/// its data directory.
	pub async fn open(data_dir: &Path) -> Result<Self> {
		let host = host_identity();

		let fingerprint_path = fingerprint_path(data_dir);
		let fingerprint = match std::fs::read_to_string(&fingerprint_path) {
			Ok(published) => published.trim().to_owned(),
			// The daemon's data directory is root-only, so an unprivileged
			// invocation cannot even learn which daemon to trust. Elevating
			// reaches both this and the host identity in one step.
			Err(err) if err.kind() == io::ErrorKind::PermissionDenied && host.needs_elevation() => {
				return Err(elevate(&format!(
					"the Seedling daemon's published identity at {} is not readable unprivileged",
					fingerprint_path.display()
				))
				.await);
			}
			Err(err) => {
				return Err(miette!(
					"cannot read the Seedling daemon's published identity at {}: {err}",
					fingerprint_path.display()
				));
			}
		};

		let (identity, as_host) = Self::identity(&host).await?;

		// bestool links both aws-lc-rs and ring, so rustls cannot pick a
		// provider by itself and panics on first use. Installing is idempotent
		// and only the first caller wins, which is why the result is dropped.
		let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

		let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), OI_PORT);
		let client = OiClient::connect(
			addr,
			ClientAuth::Fingerprint(fingerprint),
			&identity,
			actor(as_host.then(invoking_operator).flatten()),
		)
		.await
		.map_err(|err| refusal(err, &host, as_host))?;

		Ok(Self { client })
	}

	/// The key this command presents to the daemon.
	///
	/// An operator who has an identity of their own presents it, so the
	/// daemon's record of who acted names them rather than the host, and their
	/// authority is exactly the authority the daemon already grants them.
	/// The host identity is the fallback that needs no operator setup at all.
	async fn identity(host: &HostIdentity) -> Result<(ClientIdentity, bool)> {
		let operator = ClientIdentity::default_path();
		if operator.exists() {
			return load(&operator).map(|identity| (identity, false));
		}

		match host {
			HostIdentity::Readable(path) => load(path).map(|identity| (identity, true)),
			HostIdentity::NeedsElevation(path) => Err(elevate(&format!(
				"this host's Seedling identity at {} is not readable unprivileged",
				path.display()
			))
			.await),
			HostIdentity::Absent => bail!(
				"no Seedling identity to present: you have none at {}, and this host has none at {}; \
				 generate yours with `seedling-ctl client fingerprint` and authorise it with `seedling-ctl user add`",
				operator.display(),
				host_key_path().display(),
			),
		}
	}

	async fn request(&self, method: &str, params: Value) -> Result<Value> {
		debug!(method, %params, "operator interface request");
		self.client
			.request(method, params)
			.await
			.map_err(|err| miette!("the Seedling daemon rejected {method}: {err}"))
	}

	/// The underlying client, for callers that need a stream rather than a
	/// request and response.
	pub fn client(&self) -> &OiClient {
		&self.client
	}

	/// Every app the daemon manages.
	pub async fn apps(&self) -> Result<Vec<App>> {
		serde_json::from_value(self.request("/apps/list", json!({})).await?).into_diagnostic()
	}

	/// Everything the daemon knows about one app, including its resources.
	pub async fn show(&self, app: &str) -> Result<Show> {
		serde_json::from_value(self.request("/apps/show", json!({ "app": app })).await?)
			.into_diagnostic()
	}

	/// Bring every stopped resource of an app back to its desired state.
	pub async fn unstop(&self, app: &str) -> Result<()> {
		self.request("/apps/unstop", json!({ "app": app }))
			.await
			.map(drop)
	}

	/// Stop one resource, leaving it where [`Oi::unstop`] can bring it back.
	///
	/// A stop is built from these rather than from uninstalling the app: an
	/// uninstalled app is recovered by installing it again, and an install takes
	/// the parameters the app was installed with, which a stop has no business
	/// knowing or re-supplying.
	pub async fn stop_resource(&self, app: &str, kind: &str, name: &str) -> Result<()> {
		self.request(
			"/apps/resource/stop",
			json!({ "app": app, "kind": kind, "name": name }),
		)
		.await
		.map(drop)
	}

	/// Bring one stopped resource back to its desired state.
	pub async fn unstop_resource(&self, app: &str, kind: &str, name: &str) -> Result<()> {
		self.request(
			"/apps/resource/unstop",
			json!({ "app": app, "kind": kind, "name": name }),
		)
		.await
		.map(drop)
	}

	/// Roll one deployment, following the update strategy it declares.
	pub async fn restart(&self, app: &str, deployment: &str) -> Result<()> {
		self.request(
			"/apps/restart",
			json!({ "app": app, "deployment": deployment }),
		)
		.await
		.map(drop)
	}

	/// Subscribe to a log stream.
	///
	/// The request and its response ride one bidirectional stream; the entries
	/// then arrive on a unidirectional stream the daemon opens in reply.
	pub async fn log_stream(&self, params: Value) -> Result<LogStream> {
		let (mut send, mut recv) = self
			.client
			.open_bi()
			.await
			.map_err(|err| miette!("cannot open a stream to the Seedling daemon: {err}"))?;

		let request = serde_json::to_vec(&json!({ "method": "/logs/stream", "params": params }))
			.into_diagnostic()?;
		send.write_all(&request)
			.await
			.map_err(|err| miette!("cannot send the log request: {err}"))?;
		let _ = send.finish();

		let response = recv
			.read_to_end(64 * 1024)
			.await
			.map_err(|err| miette!("cannot read the log response: {err}"))?;
		if let Ok(value) = serde_json::from_slice::<Value>(&response)
			&& let Some(error) = value.get("error")
		{
			let code = error
				.get("code")
				.and_then(Value::as_str)
				.unwrap_or("unknown");
			let message = error
				.get("message")
				.and_then(Value::as_str)
				.unwrap_or("unknown error");
			bail!("the Seedling daemon refused the log request: [{code}] {message}");
		}

		let stream = self
			.client
			.accept_uni()
			.await
			.map_err(|err| miette!("the Seedling daemon opened no log stream: {err}"))?;
		Ok(LogStream {
			stream,
			buffer: Vec::new(),
		})
	}

	/// Every volume an app exports for other things to use.
	pub async fn exported_volumes(&self) -> Result<Vec<ExportedVolume>> {
		serde_json::from_value(self.request("/volumes/exported/list", json!({})).await?)
			.into_diagnostic()
	}
}

/// Load an identity from a key file that is already known to exist.
fn load(path: &Path) -> Result<ClientIdentity> {
	ClientIdentity::load_or_generate(path)
		.map(|(identity, _)| identity)
		.map_err(|err| {
			miette!(
				"cannot load the Seedling identity at {}: {err}",
				path.display()
			)
		})
}

/// Explain a connection the daemon would not accept.
///
/// A refusal of the operator's own key on a host that carries an identity for
/// us is the one case with an obvious remedy the operator can't be expected to
/// guess: the daemon would accept the host identity, and reaching it only takes
/// running the same command elevated.
fn refusal(err: impl std::fmt::Display, host: &HostIdentity, as_host: bool) -> miette::Report {
	let addr = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), OI_PORT);
	if !as_host && host.needs_elevation() {
		miette!(
			"cannot reach the Seedling daemon at {addr} as you: {err}\n\
			 this host has an identity for bestool at {}, which a privileged run would use",
			host_key_path().display()
		)
	} else {
		miette!("cannot reach the Seedling daemon at {addr}: {err}")
	}
}

/// How this command identifies itself in the daemon's event feed, so an operator
/// reading it can tell which tool acted.
///
/// Acting under the host identity, the key itself says nothing about who ran
/// the command, so the invoking operator is named here instead: the feed should
/// read as the person who acted rather than as the host.
fn actor(operator: Option<String>) -> Actor {
	Actor {
		kind: Some("bestool".into()),
		id: operator,
		display: Some(concat!("bestool ", env!("CARGO_PKG_VERSION")).into()),
		session: None,
	}
}

/// Who ran this command, as the shell that elevated it recorded them.
fn invoking_operator() -> Option<String> {
	["SUDO_USER", "USER", "USERNAME"]
		.into_iter()
		.find_map(|var| std::env::var(var).ok().filter(|user| !user.is_empty()))
}

/// One entry of the daemon's app list.
#[derive(Debug, Clone, Deserialize)]
pub struct App {
	pub name: String,
	pub status: String,
	/// Active, uncleared faults filed against the app.
	#[serde(default)]
	pub fault_count: u64,
	/// Whether any of the app's resources are stopped, which is what a start has
	/// to undo.
	#[serde(default)]
	pub has_stopped_resources: bool,
	#[serde(default)]
	pub description: Option<String>,
}

impl App {
	/// Whether the app is in a steady state with everything at its desired
	/// lifecycle state.
	///
	/// An app's status arrives lower-cased and underscored (`not_installed`)
	/// while a resource instance's lifecycle arrives capitalised (`Running`), so
	/// this compares without regard to case rather than picking one of the two
	/// spellings and being wrong about the other.
	pub fn running(&self) -> bool {
		self.status.eq_ignore_ascii_case("running")
	}
}

/// What the daemon knows about one app.
///
/// Only the fields the commands report on are modelled; a resource's full
/// definition is left alone.
#[derive(Debug, Clone, Deserialize)]
pub struct Show {
	pub status: String,
	/// Faults not tied to any one resource, such as script evaluation errors.
	#[serde(default)]
	pub faults: Vec<Value>,
	#[serde(default)]
	pub resources: Vec<Resource>,
	#[serde(default)]
	pub params: Vec<Param>,
}

/// One of an app's parameters.
#[derive(Debug, Clone, Deserialize)]
pub struct Param {
	pub name: String,
	/// The stored value, absent when the parameter is unset or secret.
	#[serde(default)]
	pub value: Option<String>,
	#[serde(default)]
	pub default_value: Option<String>,
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

/// A volume one app exports for other things to use, and where it lives on the
/// host so a co-located tool can address its contents.
#[derive(Debug, Clone, Deserialize)]
pub struct ExportedVolume {
	pub app: String,
	pub volume_name: String,
	pub host_path: PathBuf,
	#[serde(default)]
	pub description: Option<String>,
}

/// One instance of a resource.
#[derive(Debug, Clone, Deserialize)]
pub struct Instance {
	pub display_name: String,
	pub lifecycle: String,
}

/// A subscription to the daemon's log stream, yielding one entry at a time.
pub struct LogStream {
	stream: quinn::RecvStream,
	buffer: Vec<u8>,
}

impl LogStream {
	/// The next entry, or `None` once the daemon closes the stream.
	///
	/// Entries are newline-delimited, so a read can land mid-entry and the
	/// remainder is held until the rest arrives.
	pub async fn next(&mut self) -> Result<Option<LogEntry>> {
		let mut chunk = [0u8; 4096];
		loop {
			if let Some(end) = self.buffer.iter().position(|&b| b == b'\n') {
				let line: Vec<u8> = self.buffer.drain(..=end).collect();
				let line = &line[..line.len() - 1];
				if line.is_empty() {
					continue;
				}
				return serde_json::from_slice(line)
					.map(Some)
					.into_diagnostic()
					.map_err(|err| err.wrap_err("cannot read a log entry"));
			}

			match self
				.stream
				.read(&mut chunk)
				.await
				.map_err(|err| miette!("cannot read the log stream: {err}"))?
			{
				Some(read) => self.buffer.extend_from_slice(&chunk[..read]),
				None => return Ok(None),
			}
		}
	}
}

/// One entry of a log stream.
#[derive(Debug, Clone, Deserialize)]
pub struct LogEntry {
	pub timestamp: String,
	pub message: String,
	pub unit: String,
}

impl LogEntry {
	/// The emitting unit as the daemon names the thing itself, rather than as
	/// the service wrapping it.
	pub fn source(&self) -> &str {
		self.unit
			.strip_prefix("seedling-")
			.unwrap_or(&self.unit)
			.strip_suffix(".service")
			.unwrap_or_else(|| self.unit.strip_prefix("seedling-").unwrap_or(&self.unit))
	}
}

/// The resource type that carries a scale and an update strategy, and so is the
/// unit a restart rolls.
const DEPLOYMENT: &str = "deployment";

/// Resource kinds the daemon can stop and bring back. Services and volumes carry
/// no lifecycle of their own to stop.
const STOPPABLE: [&str; 3] = [DEPLOYMENT, "job", "ingress"];

impl Show {
	/// The app's deployments, which are what a restart rolls: jobs, ingresses,
	/// services, and volumes have no update strategy to follow.
	pub fn deployments(&self) -> impl Iterator<Item = &Resource> {
		self.resources.iter().filter(|r| r.kind == DEPLOYMENT)
	}

	/// A parameter's effective value: what is stored, or the default it falls
	/// back to. A secret reads as absent, since the daemon does not return one.
	pub fn param(&self, name: &str) -> Option<&str> {
		let param = self.params.iter().find(|p| p.name == name)?;
		param.value.as_deref().or(param.default_value.as_deref())
	}

	/// The resources a stop acts on, which are those the daemon can later bring
	/// back where it left them.
	pub fn stoppable(&self) -> impl Iterator<Item = &Resource> {
		self.resources
			.iter()
			.filter(|r| STOPPABLE.contains(&r.kind.as_str()))
	}
}

/// Pick the app a command acts on.
///
/// A named app must exist. Without a name, exactly one Tamanu app must be
/// present: none leaves nothing to act on, and several are ambiguous enough that
/// guessing could act on the wrong deployment.
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
			status: "running".into(),
			fault_count: 0,
			has_stopped_resources: false,
			description: None,
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
			"status": "degraded",
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
			"params": [{ "name": "db-name", "value": "tamanu", "is_set": true, "secret": false }],
		}))
		.unwrap();

		assert_eq!(show.status, "degraded");
		let deployments: Vec<&str> = show.deployments().map(|r| r.name.as_str()).collect();
		assert_eq!(deployments, vec!["api"], "jobs and ingresses aren't rolled");
		assert_eq!(show.param("db-name"), Some("tamanu"));

		let api = show.deployments().next().unwrap();
		assert_eq!(api.instances.len(), 2);
		assert_eq!(api.instances[0].display_name, "api-1");
		assert_eq!(api.instances[1].lifecycle, "Terminating");
	}

	#[test]
	fn param_falls_back_to_its_default() {
		let show: Show = serde_json::from_value(serde_json::json!({
			"status": "running",
			"params": [
				{ "name": "db-user", "value": null, "default_value": "tamanu" },
				{ "name": "auth-secret", "value": null, "default_value": null },
			],
		}))
		.unwrap();
		assert_eq!(show.param("db-user"), Some("tamanu"));
		assert_eq!(show.param("auth-secret"), None, "a secret reads as absent");
		assert_eq!(show.param("absent"), None);
	}

	#[test]
	fn stoppable_covers_the_kinds_with_a_lifecycle() {
		let show: Show = serde_json::from_value(serde_json::json!({
			"status": "running",
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
			serde_json::from_value(serde_json::json!({ "status": "not_installed" })).unwrap();
		assert!(show.resources.is_empty());
		assert_eq!(show.deployments().count(), 0);
	}

	#[test]
	fn app_list_parses_the_daemon_shape() {
		// Statuses as the daemon sends them: lower-cased and underscored, unlike
		// the capitalised lifecycle on a resource instance.
		let apps: Vec<App> = serde_json::from_value(serde_json::json!([
			{ "name": "postgres", "status": "installing", "has_stopped_resources": false,
			  "fault_count": 2, "description": "PostgreSQL database server" },
			{ "name": "tamanu-facility", "status": "not_installed",
			  "has_stopped_resources": true, "fault_count": 0, "description": "Tamanu facility" },
			{ "name": "tamanu-central", "status": "running", "fault_count": 0 },
		]))
		.unwrap();
		assert_eq!(apps.len(), 3);
		assert!(!apps[0].running());
		assert!(!apps[1].running());
		assert!(apps[2].running());
		assert_eq!(apps[0].fault_count, 2);
		assert!(apps[1].has_stopped_resources);
		assert_eq!(
			apps[0].description.as_deref(),
			Some("PostgreSQL database server")
		);
		assert!(
			!apps[2].has_stopped_resources,
			"an absent field defaults rather than failing the parse"
		);
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

	#[test]
	fn log_entry_names_the_thing_not_its_unit() {
		let entry = |unit: &str| LogEntry {
			timestamp: "t".into(),
			message: "m".into(),
			unit: unit.into(),
		};
		assert_eq!(
			entry("seedling-postgres-postgres-7c47d6e8.service").source(),
			"postgres-postgres-7c47d6e8"
		);
		assert_eq!(entry("seedling-caddy-blue.service").source(), "caddy-blue");
		assert_eq!(
			entry("postgres").source(),
			"postgres",
			"left alone when it carries neither"
		);
	}

	#[test]
	fn log_entry_parses_the_stream_shape() {
		let entry: LogEntry = serde_json::from_value(serde_json::json!({
			"timestamp": "2026-07-28T09:35:10.123456Z",
			"message": "database system is ready to accept connections",
			"unit": "postgres-postgres-7c47d6e8",
			"stream": "stdout",
			"app": "postgres",
			"resource_kind": "deployment",
		}))
		.unwrap();
		assert_eq!(entry.unit, "postgres-postgres-7c47d6e8");
		assert!(entry.message.contains("ready to accept"));
	}
}
