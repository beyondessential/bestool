use std::{path::PathBuf, sync::Arc, time::Duration};

use bestool_alertd::{
	BackgroundTask, TaskContext, TaskEndpoint, TaskEndpointResponse,
	canopy::DEFAULT_CANOPY_URL,
	tasks::TaskEndpointHandler,
};
use bestool_tamanu::{config::TamanuConfig, doctor::progress::DoctorEvent};
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use jiff::Timestamp;
use miette::{Result, miette};
use node_semver::Version;
use reqwest::Url;
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tracing::warn;

use crate::actions::tamanu::doctor;

const DOCTOR_INTERVAL: Duration = Duration::from_secs(60);

/// Periodic doctor sweep, plus on-demand `latest` / `recompute` HTTP endpoints.
///
/// The outer struct just holds an `Arc<Inner>` so we can hand inner clones to
/// the `'static` HTTP endpoint handlers without forcing the trait method
/// `http_endpoints` to take `self: Arc<Self>`.
pub struct DoctorTask {
	inner: Arc<DoctorTaskInner>,
}

struct DoctorTaskInner {
	tamanu_version: Version,
	tamanu_root: PathBuf,
	config: Arc<TamanuConfig>,
	database_url: String,
	canopy_base_url: Url,
	/// `SELECT version()` result, populated on the first tick that succeeds in
	/// reaching the database. Stable for the lifetime of the PG instance, so we
	/// reuse it across ticks instead of re-querying every minute.
	pg_version_cache: Mutex<Option<String>>,
	/// Latest sweep, captured on every successful tick. Served by the `latest`
	/// HTTP endpoint so `bestool tamanu doctor` can read what the daemon
	/// already computed instead of re-running the checks itself.
	latest: Mutex<Option<LatestSweep>>,
}

#[derive(Clone)]
struct LatestSweep {
	computed_at: Timestamp,
	payload: Value,
	server_id: Option<String>,
}

impl DoctorTask {
	pub fn new(
		tamanu_version: Version,
		tamanu_root: PathBuf,
		config: Arc<TamanuConfig>,
		database_url: String,
	) -> Self {
		Self {
			inner: Arc::new(DoctorTaskInner {
				tamanu_version,
				tamanu_root,
				config,
				database_url,
				canopy_base_url: DEFAULT_CANOPY_URL
					.parse()
					.expect("default canopy URL is valid"),
				pg_version_cache: Mutex::new(None),
				latest: Mutex::new(None),
			}),
		}
	}
}

impl DoctorTaskInner {
	async fn run_sweep(
		self: &Arc<Self>,
		ctx: &TaskContext,
		progress: Option<bestool_tamanu::doctor::progress::ProgressSender>,
	) -> Result<doctor::SweepResult> {
		let cached = self.pg_version_cache.lock().await.clone();
		let sweep = doctor::perform_sweep(
			&self.tamanu_version,
			&self.tamanu_root,
			self.config.clone(),
			&self.database_url,
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
			payload: sweep.payload.clone(),
			server_id: sweep.server_id.clone(),
		};
		*self.latest.lock().await = Some(latest);

		Ok(sweep)
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

		canopy
			.post_status(&self.canopy_base_url, &server_id, &sweep.payload)
			.await
			.map_err(|err| miette!("posting doctor status to canopy: {err}"))
	}

	/// `GET /tasks/doctor/latest` — return the last sweep this daemon
	/// computed, or 404 if it hasn't ticked yet.
	async fn endpoint_latest(self: Arc<Self>) -> TaskEndpointResponse {
		let snapshot = self.latest.lock().await.clone();
		match snapshot {
			Some(s) => TaskEndpointResponse::Json(json!({
				"computedAt": s.computed_at.to_string(),
				"serverId": s.server_id,
				"payload": s.payload,
			})),
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

		let task_self = self.clone();
		tokio::spawn(async move {
			let progress_forward_tx = out_tx.clone();
			let forwarder = tokio::spawn(async move {
				while let Some(event) = progress_rx.recv().await {
					let DoctorEvent::Completed(check) = event;
					let _ = progress_forward_tx.send(json!({
						"event": "check",
						"check": check.to_streaming_json(),
					}));
				}
			});

			match task_self.run_sweep(&ctx, Some(progress_tx)).await {
				Ok(sweep) => {
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

		let stream: BoxStream<'static, Value> = Box::pin(
			tokio_stream::wrappers::UnboundedReceiverStream::new(out_rx).map(|v| v),
		);
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
