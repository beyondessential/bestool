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

use bestool_tamanu::seedling::{App, Ctl, Resource, Show, target};

/// The volume postgres exports so other apps can reach it over a unix socket.
const SOCKET_VOLUME: &str = "socket";

/// Resolve which app the commands act on.
pub async fn app(ctl: &Ctl, named: Option<&str>) -> Result<App> {
	let apps = ctl.apps().await?;
	Ok(target(&apps, named)?.clone())
}

/// Filter resources by the NAMES arguments, matched as substrings the same way
/// the host path matches service names.
///
/// A name that matches nothing is an error naming what is available (typo
/// safety), unless `ignore_unmatched` warns and skips it the way the host
/// path's `--ignore-unmatched` does. A command must never act on a broader set
/// than the operator asked for, so an empty result after filtering is the
/// caller's signal to do nothing.
fn match_resources<'a>(
	resources: Vec<&'a Resource>,
	names: &[String],
	ignore_unmatched: bool,
) -> Result<Vec<&'a Resource>> {
	if names.is_empty() {
		return Ok(resources);
	}

	let unmatched: Vec<&str> = names
		.iter()
		.map(String::as_str)
		.filter(|name| !resources.iter().any(|r| r.name.contains(name)))
		.collect();
	if !unmatched.is_empty() {
		let available: Vec<&str> = resources.iter().map(|r| r.name.as_str()).collect();
		if ignore_unmatched {
			warn!(
				unmatched = unmatched.join(", "),
				"ignoring name(s) matching none of this app's resources"
			);
		} else {
			bail!(
				"no resource matches: {}; available names are: {}",
				unmatched.join(", "),
				available.join(", "),
			);
		}
	}

	Ok(resources
		.into_iter()
		.filter(|r| names.iter().any(|name| r.name.contains(name)))
		.collect())
}

/// Bring the app, or the named resources, out of the stopped state.
///
/// spec: SHC#lifecycle
pub async fn start(app_info: &App, ctl: &Ctl, names: &[String], ignore_unmatched: bool) -> Result<()> {
	let app = &app_info.name;
	// Starting can't help an app that was never installed: installing it is
	// the recovery, and that takes parameters this command doesn't hold.
	if app_info.status.eq_ignore_ascii_case("not_installed") {
		bail!("the {app} app is not installed, so there is nothing to start; it is installed with `seedling-ctl apps install`");
	}

	if names.is_empty() {
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
	} else {
		let show = ctl.show(app).await?;
		let matched = match_resources(show.stoppable().collect(), names, ignore_unmatched)?;
		for resource in matched {
			info!(app, kind = %resource.kind, name = %resource.name, "starting");
			ctl.unstop_resource(app, &resource.kind, &resource.name)
				.await?;
		}
	}
	report(ctl, app).await;
	Ok(())
}

/// Return the app, or the named resources, to the stopped state.
///
/// spec: SHC#lifecycle
pub async fn stop(app_info: &App, ctl: &Ctl, names: &[String]) -> Result<()> {
	let app = &app_info.name;
	let show = ctl.show(app).await?;
	let mut stoppable = match_resources(show.stoppable().collect(), names, false)?;

	if stoppable.is_empty() {
		info!(app, "nothing to stop: the app has no stoppable resources");
		return Ok(());
	}

	// Ingresses stop first so traffic stops arriving before the deployments
	// serving it go down, then jobs last.
	let order = |r: &&Resource| match r.kind.as_str() {
		"ingress" => 0,
		"deployment" => 1,
		_ => 2,
	};
	stoppable.sort_by_key(order);

	for resource in stoppable {
		info!(app, kind = %resource.kind, name = %resource.name, "stopping");
		ctl.stop_resource(app, &resource.kind, &resource.name)
			.await?;
	}
	report(ctl, app).await;
	Ok(())
}

/// Roll each of the app's deployments, following the update strategy each one
/// declares.
///
/// spec: SHC#lifecycle
pub async fn restart(
	app_info: &App,
	ctl: &Ctl,
	names: &[String],
	ignore_unmatched: bool,
) -> Result<()> {
	let app = &app_info.name;
	let show = ctl.show(app).await?;
	let deployments: Vec<String> =
		match_resources(show.deployments().collect(), names, ignore_unmatched)?
			.into_iter()
			.map(|r| r.name.clone())
			.collect();

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
/// The operator may name a different role; local socket connections are
/// trusted, so any role works without a password.
///
/// spec: SHC#interactive-database-access
pub async fn database_url(app_info: &App, ctl: &Ctl, username: Option<&str>) -> Result<String> {
	let show = ctl.show(&app_info.name).await?;
	let database = show.param("db-name").ok_or_else(|| {
		miette!(
			"the {} app declares no db-name parameter, so its database can't be identified",
			app_info.name
		)
	})?;
	let user = match username {
		Some(user) => user,
		None => show.param("db-user").ok_or_else(|| {
			miette!(
				"the {} app declares no db-user parameter, so its role can't be identified",
				app_info.name
			)
		})?,
	};

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
pub async fn status(app_info: &App, ctl: &Ctl, names: &[String]) -> Result<()> {
	let app = &app_info.name;
	let show = ctl.show(app).await?;
	println!("{app}: {}", show.status);
	if !show.faults.is_empty() {
		println!("  {} app-level fault(s)", show.faults.len());
	}

	for resource in match_resources(show.resources.iter().collect(), names, false)? {
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
