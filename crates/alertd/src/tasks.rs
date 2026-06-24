use std::{collections::BTreeMap, sync::Arc, time::Duration};

use futures::{future::BoxFuture, stream::BoxStream};
use miette::Result;
use serde_json::Value;

use crate::{canopy::CanopyClient, context::InternalContext};

/// Shared resources passed to background tasks on every tick.
///
/// The `reqwest::Client` is shared with the daemon's other consumers so its
/// connection pool stays warm across tick intervals.
#[derive(Clone)]
pub struct TaskContext {
	/// `None` on hosts with no Tamanu deployment (and therefore no database).
	pub pg_pool: Option<bestool_postgres::pool::PgPool>,
	pub http_client: reqwest::Client,
	pub canopy_client: Option<Arc<CanopyClient>>,
	/// Bumped on each reload request (SIGHUP/SIGUSR1); a task can
	/// `reload.changed().await` to refresh its state without a restart.
	pub reload: tokio::sync::watch::Receiver<u64>,
	/// Query parameters of the request, for HTTP endpoint handlers. Empty on the
	/// periodic `run` tick.
	pub query: BTreeMap<String, String>,
}

impl TaskContext {
	pub(crate) fn from_internal(ctx: &InternalContext) -> Self {
		Self {
			pg_pool: ctx.pg_pool.clone(),
			http_client: ctx.http_client.clone(),
			canopy_client: ctx.canopy_client.clone(),
			reload: ctx.reload.clone(),
			query: BTreeMap::new(),
		}
	}
}

/// A periodic background task registered with the daemon.
///
/// The daemon spawns one tokio task per registered plugin at startup and ticks
/// it at `interval()`. Each tick counts as activity for the watchdog. Errors
/// returned from `run` are logged but don't kill the daemon.
///
/// Tasks can optionally expose HTTP endpoints (e.g. to surface their latest
/// computed state, or to trigger an on-demand re-run) via [`Self::http_endpoints`].
/// The daemon mounts each at `/tasks/{task-name}/{endpoint-name}` and routes
/// matching requests to the handler.
pub trait BackgroundTask: Send + Sync + 'static {
	fn name(&self) -> &'static str;
	fn interval(&self) -> Duration;
	fn run<'a>(&'a self, ctx: &'a TaskContext) -> BoxFuture<'a, Result<()>>;
	/// Endpoints this task wants the daemon to expose under
	/// `/tasks/{self.name()}/{endpoint.name}`.
	///
	/// Default is "no endpoints". Endpoint handlers are 'static closures, so
	/// they should capture an `Arc` of whatever shared state they need rather
	/// than borrowing from `self`.
	fn http_endpoints(&self) -> Vec<TaskEndpoint> {
		Vec::new()
	}
}

/// One HTTP endpoint a `BackgroundTask` exposes through the daemon.
pub struct TaskEndpoint {
	/// Name segment appended after `/tasks/{task}/` to form the URL path.
	pub name: &'static str,
	pub handler: TaskEndpointHandler,
}

/// Handler invoked when a request hits `/tasks/{task}/{endpoint}`.
///
/// The daemon hands the handler a fresh `TaskContext` (built from the
/// daemon's own resources) and awaits the future.
pub type TaskEndpointHandler =
	Arc<dyn Fn(TaskContext) -> BoxFuture<'static, TaskEndpointResponse> + Send + Sync + 'static>;

/// What an endpoint handler returns to the daemon for it to serialise.
///
/// `JsonLines` is for streaming: the daemon writes each yielded `Value` as a
/// JSON-encoded line followed by `\n`, with `Content-Type:
/// application/x-ndjson`. That's how the doctor's "recompute" endpoint
/// surfaces per-check progress to consumers like `bestool tamanu doctor`.
pub enum TaskEndpointResponse {
	Json(Value),
	JsonLines(BoxStream<'static, Value>),
	/// 4xx / 5xx response with a plain-text body.
	Error {
		status: u16,
		message: String,
	},
}
