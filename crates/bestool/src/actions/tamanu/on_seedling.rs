//! The Tamanu lifecycle commands as they act on a Seedling host.
//!
//! Each command resolves the host before doing any install discovery or
//! privilege elevation, then comes here. The host-supervisor path's model of
//! expected services and their instances has no analogue here: the daemon owns
//! the app and reports its own resources, so these act on what it reports
//! rather than on a set of units computed from config.
//!
//! spec: SHC

use miette::{Result, bail, miette};
use tracing::{debug, info, warn};

use bestool_tamanu::seedling::{App, Ctl, Show, target};

/// The volume postgres exports so other apps can reach it over a unix socket.
const SOCKET_VOLUME: &str = "socket";

/// Resolve which app the commands act on.
pub async fn app(ctl: &Ctl, named: Option<&str>) -> Result<App> {
	let apps = ctl.apps().await?;
	Ok(target(&apps, named)?.clone())
}

/// Bring the app out of the stopped state.
///
/// spec: SHC#lifecycle
pub async fn start(app_info: &App, ctl: &Ctl) -> Result<()> {
	let app = &app_info.name;
	if !app_info.has_stopped_resources {
		info!(
			app,
			status = %app_info.status,
			"nothing to start: no resources are stopped"
		);
		return Ok(());
	}

	info!(app, "starting through the Seedling daemon");
	ctl.unstop(app).await?;
	report(ctl, app).await;
	Ok(())
}

/// Return the app to the stopped state.
///
/// spec: SHC#lifecycle
pub async fn stop(app_info: &App, ctl: &Ctl) -> Result<()> {
	let app = &app_info.name;
	let show = ctl.show(app).await?;
	let stoppable: Vec<(String, String)> = show
		.stoppable()
		.map(|r| (r.kind.clone(), r.name.clone()))
		.collect();

	if stoppable.is_empty() {
		info!(app, "nothing to stop: the app has no stoppable resources");
		return Ok(());
	}

	for (kind, name) in &stoppable {
		info!(app, kind, name, "stopping");
		ctl.stop_resource(app, kind, name).await?;
	}
	report(ctl, app).await;
	Ok(())
}

/// Roll each of the app's deployments, following the update strategy each one
/// declares.
///
/// spec: SHC#lifecycle
pub async fn restart(app_info: &App, ctl: &Ctl) -> Result<()> {
	let app = &app_info.name;
	let show = ctl.show(app).await?;
	let deployments: Vec<String> = show.deployments().map(|r| r.name.clone()).collect();

	if deployments.is_empty() {
		info!(app, "nothing to restart: the app has no deployments");
		return Ok(());
	}

	// Rolled one at a time so a deployment that can keep an instance serving
	// through its own update strategy actually does, instead of every
	// deployment going down together.
	for deployment in &deployments {
		info!(app, deployment, "rolling");
		ctl.restart(app, deployment).await?;
	}
	report(ctl, app).await;
	Ok(())
}

/// The libpq URL for reaching Tamanu's database on a Seedling host.
///
/// Postgres runs as its own app and exports the directory holding its unix
/// socket, so a co-located tool connects over that socket rather than over the
/// network. Local socket connections are trusted, which is what lets this work
/// without handling the superuser password, and the database never has to be
/// reachable beyond the host.
///
/// Everything here comes from the daemon: the socket's location from the
/// exported-volume listing, the database and role from the Tamanu app's own
/// parameters.
///
/// spec: SHC#interactive-database-access
pub async fn database_url(app_info: &App, ctl: &Ctl) -> Result<String> {
	let show = ctl.show(&app_info.name).await?;
	let database = show.param("db-name").ok_or_else(|| {
		miette!(
			"the {} app declares no db-name parameter, so its database can't be identified",
			app_info.name
		)
	})?;
	let user = show.param("db-user").ok_or_else(|| {
		miette!(
			"the {} app declares no db-user parameter, so its role can't be identified",
			app_info.name
		)
	})?;

	let exported = ctl.exported_volumes().await?;
	let socket = exported
		.iter()
		.find(|v| v.volume_name == SOCKET_VOLUME && v.host_path.is_dir())
		.ok_or_else(|| {
			miette!(
				"no app exports a `{SOCKET_VOLUME}` volume, so there is no local socket to reach a database through; the postgres app provides one when it is installed"
			)
		})?;

	// The socket directory goes in the host parameter rather than the authority,
	// which is how libpq is told to use a socket instead of a hostname. It is
	// carried literally, so a path holding a character that would end the
	// parameter is refused rather than silently truncating the path.
	let host = socket.host_path.to_string_lossy();
	if let Some(bad) = host.chars().find(|c| "?#&= ".contains(*c) || c.is_whitespace()) {
		bail!(
			"the exported socket directory {} contains {bad:?}, which can't be carried in a connection URL",
			socket.host_path.display()
		);
	}
	debug!(%database, %user, path = %socket.host_path.display(), "reaching the database over Seedling's exported socket");
	Ok(format!("postgresql://{user}@/{database}?host={host}"))
}

/// Report the app state the daemon holds, resource by resource.
///
/// spec: SHC#status
pub async fn status(app_info: &App, ctl: &Ctl) -> Result<()> {
	let app = &app_info.name;
	let show = ctl.show(app).await?;
	println!("{app}: {}", show.status);
	if !show.faults.is_empty() {
		println!("  {} app-level fault(s)", show.faults.len());
	}

	for resource in &show.resources {
		let faults = if resource.faults.is_empty() {
			String::new()
		} else {
			format!(", {} fault(s)", resource.faults.len())
		};

		if resource.instances.is_empty() {
			println!("  {} ({}){faults}", resource.name, resource.kind);
			continue;
		}

		let instances = resource
			.instances
			.iter()
			.map(|i| format!("{}={}", i.display_name, i.lifecycle))
			.collect::<Vec<_>>()
			.join(" ");
		println!(
			"  {} ({}){faults}: {instances}",
			resource.name, resource.kind
		);
	}
	Ok(())
}

/// Log where the app ended up, so an operator sees the outcome without running
/// status separately. A failure to read it back doesn't undo the action that
/// just succeeded, so it warns rather than propagating.
async fn report(ctl: &Ctl, app: &str) {
	match ctl.show(app).await {
		Ok(Show { status, .. }) => info!(app, %status, "app state"),
		Err(err) => warn!(app, %err, "could not read the app state back"),
	}
}
