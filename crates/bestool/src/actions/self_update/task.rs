//! Windows-only background task that keeps bestool up to date.
//!
//! The daemon checks daily — staggered across the fleet — for a newer published
//! release; when one is available it downloads and verifies it, replaces the
//! binary, and restarts itself so the new binary takes effect. On Linux the
//! package manager owns updates, so this task is Windows-only.
//!
//! It also exposes `/tasks/self-update/update`, which `bestool self-update`
//! calls to hand an on-demand update to the running daemon instead of swapping
//! the binary from a separate process.

use std::{
	collections::hash_map::DefaultHasher,
	hash::{Hash as _, Hasher as _},
	sync::{Arc, Mutex},
	time::Duration,
};

use futures::future::BoxFuture;
use miette::Result;
use serde_json::json;
use tracing::{debug, info, warn};

use crate::alertd::{BackgroundTask, TaskContext, TaskEndpoint, TaskEndpointResponse};

use super::{UpdateOutcome, perform_update, perform_update_from_file};
use crate::download::{fetch_latest_version, remote_is_newer};

/// How often to check for a new release once the initial stagger has elapsed.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Window over which the first check is spread across hosts, so a fleet doesn't
/// fetch and restart in lockstep. The offset within it is derived from the
/// hostname, so a given host's schedule is stable across restarts.
const STAGGER_WINDOW: Duration = Duration::from_secs(60 * 60);

/// The most recent version that failed to install, and why.
///
/// spec: UPD#update-failures
#[derive(Debug, Clone)]
struct UpdateFailure {
	version: String,
	reason: String,
}

impl UpdateFailure {
	fn new(version: String, err: &miette::Report) -> Self {
		Self {
			version,
			reason: describe_error(err),
		}
	}
}

/// Flatten an error and its causes into one line, so the reason survives being
/// shown in a single status line or CLI error.
fn describe_error(err: &miette::Report) -> String {
	err.chain()
		.map(ToString::to_string)
		.collect::<Vec<_>>()
		.join(": ")
}

type SharedFailure = Arc<Mutex<Option<UpdateFailure>>>;

pub(crate) struct SelfUpdateTask {
	/// A version that failed to install, so the same failure isn't reattempted
	/// on every check. A newer version compares unequal and is still tried.
	/// Shared with the on-demand endpoint handler.
	failed: SharedFailure,
}

impl SelfUpdateTask {
	pub(crate) fn new() -> Self {
		Self {
			failed: Arc::new(Mutex::new(None)),
		}
	}

	fn failed_version_is(&self, version: &str) -> bool {
		self.failed
			.lock()
			.unwrap()
			.as_ref()
			.is_some_and(|failure| failure.version == version)
	}

	async fn check_and_update(&self, ctx: &TaskContext) {
		let current = env!("CARGO_PKG_VERSION");
		let latest = match fetch_latest_version().await {
			Ok(latest) => latest,
			Err(err) => {
				warn!("self-update: could not check latest version: {err}");
				return;
			}
		};

		if !remote_is_newer(current, &latest) {
			debug!(current, %latest, "self-update: already current");
			return;
		}

		if self.failed_version_is(&latest) {
			warn!(%latest, "self-update: skipping version that previously failed to install");
			return;
		}

		info!(current, %latest, "self-update: newer version available, installing");
		match perform_update(&latest, None, None, false).await {
			Ok(UpdateOutcome::Updated { from, to }) => {
				info!(%from, %to, "self-update installed; requesting daemon restart");
				request_restart(ctx).await;
			}
			Ok(UpdateOutcome::AlreadyCurrent { .. }) => {}
			Err(err) => {
				warn!(%latest, "self-update failed; not retrying this version: {err}");
				*self.failed.lock().unwrap() = Some(UpdateFailure::new(latest, &err));
			}
		}
	}
}

/// Initial delay before the first check, derived from the hostname so the
/// fleet's checks spread across [`STAGGER_WINDOW`] rather than aligning.
fn stagger_offset() -> Duration {
	let host = std::env::var("COMPUTERNAME").unwrap_or_default();
	let mut hasher = DefaultHasher::new();
	host.hash(&mut hasher);
	Duration::from_secs(hasher.finish() % STAGGER_WINDOW.as_secs())
}

async fn request_restart(ctx: &TaskContext) {
	if let Some(restart) = &ctx.restart {
		restart.request_restart().await;
	} else {
		warn!("self-update: no restart handle; the new binary takes effect on next restart");
	}
}

impl BackgroundTask for SelfUpdateTask {
	fn name(&self) -> &'static str {
		"self-update"
	}

	fn interval(&self) -> Duration {
		// run() is resident (it loops internally), so this only gates the
		// daemon's single first tick.
		CHECK_INTERVAL
	}

	fn run<'a>(&'a self, ctx: &'a TaskContext) -> BoxFuture<'a, Result<()>> {
		Box::pin(async move {
			let offset = stagger_offset();
			info!(?offset, "self-update: first check staggered");
			tokio::time::sleep(offset).await;

			let mut tick = tokio::time::interval(CHECK_INTERVAL);
			loop {
				tick.tick().await;
				self.check_and_update(ctx).await;
			}
		})
	}

	fn http_endpoints(&self) -> Vec<TaskEndpoint> {
		let failed_for_update = self.failed.clone();
		let failed_for_status = self.failed.clone();
		vec![
			TaskEndpoint::open(
				"update",
				Arc::new(move |ctx: TaskContext| {
					Box::pin(on_demand_update(ctx, failed_for_update.clone()))
				}),
			),
			// Lets `bestool self-update` observe a delegated update: it polls this
			// to distinguish a failed install (this process stays up and reports
			// the version it couldn't install) from a still-in-progress one.
			TaskEndpoint::open(
				"status",
				Arc::new(move |_ctx: TaskContext| {
					let failed = failed_for_status.clone();
					Box::pin(async move {
						let failed = failed.lock().unwrap().clone();
						TaskEndpointResponse::Json(status_body(failed.as_ref()))
					})
				}),
			),
		]
	}
}

/// The `/tasks/self-update/status` body: the running version, and the version
/// that last failed to install with the reason it failed.
///
/// spec: UPD#update-failures
fn status_body(failed: Option<&UpdateFailure>) -> serde_json::Value {
	json!({
		"current": env!("CARGO_PKG_VERSION"),
		"failed_version": failed.map(|f| &f.version),
		"failed_reason": failed.map(|f| &f.reason),
	})
}

/// Handle `/tasks/self-update/update`: decide whether an update is warranted and
/// respond immediately, kicking off the download, install, and restart in the
/// background so the response reaches the caller before the daemon exits.
async fn on_demand_update(
	ctx: TaskContext,
	failed: SharedFailure,
) -> TaskEndpointResponse {
	let current = env!("CARGO_PKG_VERSION");

	// An operator-supplied local file takes precedence over any download inputs
	// and skips version resolution entirely. Signature verification is bypassed
	// deliberately: the file is an explicit local binary handed over by the
	// operator (analogous to --force), and this endpoint is loopback-only.
	if let Some(path) = ctx.query.get("from_file") {
		let path = std::path::PathBuf::from(path);
		let to = format!("file:{}", path.display());
		let restart = ctx.restart.clone();
		tokio::spawn(async move {
			match perform_update_from_file(&path).await {
				Ok(_) => {
					info!(from = %path.display(), "on-demand self-update from file installed; restarting");
					// Brief grace so the HTTP response flushes before the daemon exits.
					tokio::time::sleep(Duration::from_secs(1)).await;
					if let Some(restart) = restart {
						restart.request_restart().await;
					}
				}
				Err(err) => {
					warn!(from = %path.display(), "on-demand self-update from file failed: {err}");
					*failed.lock().unwrap() =
						Some(UpdateFailure::new(format!("file:{}", path.display()), &err));
				}
			}
		});

		return TaskEndpointResponse::Json(json!({
			"updating": true,
			"from": current,
			"to": to,
		}));
	}

	let requested = ctx
		.query
		.get("version")
		.map(String::as_str)
		.unwrap_or("latest");
	let force = ctx.query.get("force").map(|v| v == "true").unwrap_or(false);

	let resolved = if requested == "latest" {
		match fetch_latest_version().await {
			Ok(latest) => {
				if !force && !remote_is_newer(current, &latest) {
					return TaskEndpointResponse::Json(json!({
						"updating": false,
						"current": current,
						"latest": latest,
					}));
				}
				latest
			}
			Err(err) => {
				return TaskEndpointResponse::Error {
					status: 502,
					message: format!("could not check latest version: {err}"),
				};
			}
		}
	} else {
		requested.to_string()
	};

	let restart = ctx.restart.clone();
	let to = resolved.clone();
	tokio::spawn(async move {
		match perform_update(&resolved, None, None, true).await {
			Ok(_) => {
				info!(version = %resolved, "on-demand self-update installed; restarting");
				// Brief grace so the HTTP response flushes before the daemon exits.
				tokio::time::sleep(Duration::from_secs(1)).await;
				if let Some(restart) = restart {
					restart.request_restart().await;
				}
			}
			Err(err) => {
				warn!(version = %resolved, "on-demand self-update failed: {err}");
				*failed.lock().unwrap() = Some(UpdateFailure::new(resolved, &err));
			}
		}
	});

	TaskEndpointResponse::Json(json!({
		"updating": true,
		"from": current,
		"to": to,
	}))
}

#[cfg(test)]
mod tests {
	use miette::miette;

	use super::*;

	#[test]
	fn status_body_reports_version_and_reason() {
		let err = miette!("signature did not verify").wrap_err("installing 2.2.5");
		let failure = UpdateFailure::new("2.2.5".into(), &err);
		let body = status_body(Some(&failure));
		assert_eq!(body["failed_version"], "2.2.5");
		assert_eq!(
			body["failed_reason"],
			"installing 2.2.5: signature did not verify"
		);
	}

	#[test]
	fn status_body_without_failure_reports_nulls() {
		let body = status_body(None);
		assert!(body["failed_version"].is_null());
		assert!(body["failed_reason"].is_null());
	}
}
