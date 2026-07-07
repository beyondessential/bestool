use std::{collections::HashMap, sync::Arc, time::Duration};

use bestool_canopy::schema::CheckSeverity;
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use jiff::Timestamp;
use miette::{Result, miette};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tracing::warn;

use crate::doctor::{self, check::Check, progress::DoctorEvent};
use crate::tasks::TaskEndpointHandler;
use crate::{BackgroundTask, TaskContext, TaskEndpoint, TaskEndpointResponse};

const DOCTOR_INTERVAL: Duration = Duration::from_secs(60);

/// Invoked with the `backup_now` list from canopy's status response.
///
/// alertd has no backup logic of its own; the bestool binary supplies this to
/// run the in-process backup driver. Fire-and-forget: the callback spawns its
/// own work and guards against overlapping runs.
pub type BackupDispatch = Arc<dyn Fn(Vec<String>) + Send + Sync>;

/// Apply the effective-severity ceiling to a single streamed check, if a
/// mapping is available (a no-op otherwise). Mirrors
/// [`doctor::SweepResult::apply_severities`] for the one-check streaming case.
fn cap_check(check: Check, severities: Option<&HashMap<String, CheckSeverity>>) -> Check {
	match severities {
		Some(map) => {
			let ceiling = doctor::sweep::severity_ceiling(map, check.name);
			Check {
				status: check.status.cap_to(ceiling),
				..check
			}
		}
		None => check,
	}
}

/// Periodic doctor sweep, plus on-demand `latest` / `recompute` HTTP endpoints.
///
/// The outer struct just holds an `Arc<Inner>` so we can hand inner clones to
/// the `'static` HTTP endpoint handlers without forcing the trait method
/// `http_endpoints` to take `self: Arc<Self>`.
pub struct DoctorTask {
	inner: Arc<DoctorTaskInner>,
}

struct DoctorTaskInner {
	binary_version: String,
	/// `None` on hosts with no Tamanu deployment: sweeps still run (and post),
	/// with all Tamanu-dependent checks skipped.
	tamanu: Option<doctor::SweepTamanu>,
	/// `SELECT version()` result, populated on the first tick that succeeds in
	/// reaching the database. Stable for the lifetime of the PG instance, so we
	/// reuse it across ticks instead of re-querying every minute.
	pg_version_cache: Mutex<Option<String>>,
	/// Latest sweep, captured on every successful tick. Served by the `latest`
	/// HTTP endpoint so `bestool tamanu doctor` can read what the daemon
	/// already computed instead of re-running the checks itself.
	latest: Mutex<Option<LatestSweep>>,
	/// Effective-severity ceilings canopy last returned on a status push, keyed
	/// by check name. `None` until the first successful push. Applied to the
	/// sweeps this daemon serves locally (`latest` / `recompute`) so operators
	/// see the same severities the CLI and canopy show; the payload posted to
	/// canopy stays raw. See [`doctor::SweepResult::apply_severities`].
	check_severities: Mutex<Option<HashMap<String, CheckSeverity>>>,
	/// Runs the backup driver for the types canopy asks for via `backup_now`.
	/// `None` when backups aren't compiled in.
	backup_dispatch: Option<BackupDispatch>,
}

#[derive(Clone)]
struct LatestSweep {
	computed_at: Timestamp,
	/// The raw sweep result, kept typed so the `latest` endpoint can apply the
	/// current severity ceilings on read rather than baking them in at sweep time.
	sweep: doctor::SweepResult,
}

impl DoctorTask {
	pub fn new(binary_version: String, tamanu: Option<doctor::SweepTamanu>) -> Self {
		Self {
			inner: Arc::new(DoctorTaskInner {
				binary_version,
				tamanu,
				pg_version_cache: Mutex::new(None),
				latest: Mutex::new(None),
				check_severities: Mutex::new(None),
				backup_dispatch: None,
			}),
		}
	}

	/// Attach the backup dispatcher invoked when canopy requests a backup.
	///
	/// Call right after [`DoctorTask::new`] (before the task is shared).
	pub fn with_backup_dispatch(self, dispatch: BackupDispatch) -> Self {
		let mut inner =
			Arc::try_unwrap(self.inner).unwrap_or_else(|_| panic!("DoctorTask already shared"));
		inner.backup_dispatch = Some(dispatch);
		Self {
			inner: Arc::new(inner),
		}
	}
}

impl DoctorTaskInner {
	async fn run_sweep(
		self: &Arc<Self>,
		ctx: &TaskContext,
		progress: Option<doctor::progress::ProgressSender>,
	) -> Result<doctor::SweepResult> {
		let cached = self.pg_version_cache.lock().await.clone();
		let sweep = doctor::perform_sweep(
			&self.binary_version,
			self.tamanu.clone(),
			ctx.http_client.clone(),
			&[],
			&[],
			cached,
			progress,
		)
		.await?;

		if let Some(ref version) = sweep.pg_version {
			let mut guard = self.pg_version_cache.lock().await;
			if guard.is_none() {
				*guard = Some(version.clone());
			}
		}

		let latest = LatestSweep {
			computed_at: Timestamp::now(),
			sweep: sweep.clone(),
		};
		*self.latest.lock().await = Some(latest);

		Ok(sweep)
	}

	/// Snapshot the severity ceilings canopy last returned, if any.
	async fn severities_snapshot(&self) -> Option<HashMap<String, CheckSeverity>> {
		self.check_severities.lock().await.clone()
	}

	/// Apply the current severity ceilings to a sweep, if we have any. A no-op
	/// (leaving the raw sweep) until canopy has returned a mapping.
	async fn capped(&self, mut sweep: doctor::SweepResult) -> doctor::SweepResult {
		if let Some(severities) = self.severities_snapshot().await {
			sweep.apply_severities(&severities);
		}
		sweep
	}

	async fn tick(self: &Arc<Self>, ctx: &TaskContext) -> Result<()> {
		let sweep = self.run_sweep(ctx, None).await?;

		let Some(server_id) = sweep.server_id else {
			warn!("no metaServerId available; skipping canopy status push");
			return Ok(());
		};

		let Some(canopy) = ctx.canopy_client.as_ref() else {
			warn!("no canopy client available; skipping canopy status push");
			return Ok(());
		};

		let response = canopy
			.status(&server_id, &sweep.payload)
			.await
			.map_err(|err| miette!("posting doctor status to canopy: {err}"))?;

		// Cache the effective-severity ceilings for the sweeps we serve locally.
		// The payload we just posted stays raw: canopy is the source of truth and
		// maps severities itself.
		*self.check_severities.lock().await = Some(response.check_severities);

		let backup_now = response.backup_now;

		if !backup_now.is_empty() {
			match &self.backup_dispatch {
				Some(dispatch) => dispatch(backup_now),
				None => warn!(
					?backup_now,
					"canopy requested a backup but no backup dispatcher is configured"
				),
			}
		}

		Ok(())
	}

	/// `GET /tasks/doctor/latest` — return the last sweep this daemon
	/// computed, or 404 if it hasn't ticked yet.
	async fn endpoint_latest(self: Arc<Self>) -> TaskEndpointResponse {
		let snapshot = self.latest.lock().await.clone();
		match snapshot {
			Some(s) => {
				let sweep = self.capped(s.sweep).await;
				TaskEndpointResponse::Json(json!({
					"computedAt": s.computed_at.to_string(),
					"serverId": sweep.server_id,
					"payload": sweep.payload,
				}))
			}
			None => TaskEndpointResponse::Error {
				status: 503,
				message: "no doctor sweep cached yet (daemon may have just started)".into(),
			},
		}
	}

	/// `GET /tasks/doctor/recompute` — drive a fresh sweep and stream each
	/// progress event back as NDJSON. Final line is the full sweep result.
	async fn endpoint_recompute(self: Arc<Self>, ctx: TaskContext) -> TaskEndpointResponse {
		let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<DoctorEvent>();
		let (out_tx, out_rx) = mpsc::unbounded_channel::<Value>();

		// Snapshot the ceilings once so the streamed per-check events and the
		// final payload are capped consistently, matching what `latest` serves.
		let severities = self.severities_snapshot().await;

		let task_self = self.clone();
		tokio::spawn(async move {
			let progress_forward_tx = out_tx.clone();
			let stream_severities = severities.clone();
			let forwarder = tokio::spawn(async move {
				while let Some(event) = progress_rx.recv().await {
					let DoctorEvent::Completed(check) = event;
					let check = cap_check(check, stream_severities.as_ref());
					let _ = progress_forward_tx.send(json!({
						"event": "check",
						"check": check.to_streaming_json(),
					}));
				}
			});

			match task_self.run_sweep(&ctx, Some(progress_tx)).await {
				Ok(mut sweep) => {
					if let Some(severities) = &severities {
						sweep.apply_severities(severities);
					}
					// Make sure all `Completed` events arrived before we emit
					// `done` — perform_sweep drops the sender on return, which
					// closes the forwarder loop above.
					let _ = forwarder.await;
					let _ = out_tx.send(json!({
						"event": "done",
						"computedAt": Timestamp::now().to_string(),
						"serverId": sweep.server_id,
						"payload": sweep.payload,
					}));
				}
				Err(err) => {
					let _ = forwarder.await;
					let _ = out_tx.send(json!({
						"event": "error",
						"message": format!("{err:?}"),
					}));
				}
			}
		});

		let stream: BoxStream<'static, Value> =
			Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(out_rx).map(|v| v));
		TaskEndpointResponse::JsonLines(stream)
	}
}

impl BackgroundTask for DoctorTask {
	fn name(&self) -> &'static str {
		"doctor"
	}

	fn interval(&self) -> Duration {
		DOCTOR_INTERVAL
	}

	fn run<'a>(&'a self, ctx: &'a TaskContext) -> BoxFuture<'a, Result<()>> {
		let inner = self.inner.clone();
		Box::pin(async move { inner.tick(ctx).await })
	}

	fn http_endpoints(&self) -> Vec<TaskEndpoint> {
		let latest_handler: TaskEndpointHandler = {
			let inner = self.inner.clone();
			Arc::new(move |_ctx| {
				let inner = inner.clone();
				Box::pin(async move { inner.endpoint_latest().await })
			})
		};

		let recompute_handler: TaskEndpointHandler = {
			let inner = self.inner.clone();
			Arc::new(move |ctx| {
				let inner = inner.clone();
				Box::pin(async move { inner.endpoint_recompute(ctx).await })
			})
		};

		vec![
			TaskEndpoint {
				name: "latest",
				handler: latest_handler,
			},
			TaskEndpoint {
				name: "recompute",
				handler: recompute_handler,
			},
		]
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::check::CheckStatus;

	#[test]
	fn cap_check_applies_ceiling_when_present() {
		let mut severities = HashMap::new();
		severities.insert("disk_free".to_string(), CheckSeverity::Warn);
		let check = Check::fail("disk_free", "1% free", "out of space");
		let capped = cap_check(check, Some(&severities));
		match capped.status {
			CheckStatus::Warning(r) => assert_eq!(r, "out of space"),
			other => panic!("expected Warning, got {other:?}"),
		}
	}

	#[test]
	fn cap_check_absent_check_defaults_to_warn() {
		// No entry for this check: canopy's default ceiling is warn, so a fail
		// streams as a warning.
		let check = Check::fail("brand_new", "bad", "reason");
		let capped = cap_check(check, Some(&HashMap::new()));
		assert!(matches!(capped.status, CheckStatus::Warning(_)));
	}

	#[test]
	fn cap_check_no_mapping_is_a_noop() {
		let check = Check::fail("disk_free", "1% free", "out of space");
		let capped = cap_check(check, None);
		assert!(matches!(capped.status, CheckStatus::Fail(_)));
	}
}
