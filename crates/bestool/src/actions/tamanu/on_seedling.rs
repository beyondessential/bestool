//! The Tamanu lifecycle commands as they act on a Seedling host.
//!
//! Each command resolves the host before doing any install discovery or
//! privilege elevation, then comes here. The host-supervisor path's model of
//! expected services and their instances has no analogue here: the daemon owns
//! the app and reports its own resources, so these act on what it reports
//! rather than on a set of units computed from config.
//!
//! spec: SHC

use miette::Result;
use tracing::{info, warn};

use bestool_tamanu::seedling::{Ctl, Show, target};

/// Resolve which app the commands act on.
pub async fn app(ctl: &Ctl, named: Option<&str>) -> Result<String> {
	let apps = ctl.apps().await?;
	Ok(target(&apps, named)?.name.clone())
}

/// Bring the app out of the stopped state.
///
/// spec: SHC#lifecycle
pub async fn start(ctl: &Ctl, app: &str) -> Result<()> {
	info!(app, "starting through the Seedling daemon");
	ctl.unstop(app).await?;
	report(ctl, app).await;
	Ok(())
}

/// Return the app to the stopped state.
///
/// spec: SHC#lifecycle
pub async fn stop(ctl: &Ctl, app: &str) -> Result<()> {
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
pub async fn restart(ctl: &Ctl, app: &str) -> Result<()> {
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

/// Report the app state the daemon holds, resource by resource.
///
/// spec: SHC#status
pub async fn status(ctl: &Ctl, app: &str) -> Result<()> {
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
