use std::{sync::Arc, time::Duration};

use futures::future::BoxFuture;
use miette::Result;

use crate::{alert::InternalContext, canopy::CanopyClient};

/// Shared resources passed to background tasks on every tick.
///
/// The `reqwest::Client` is shared with the daemon's other consumers so its
/// connection pool stays warm across tick intervals.
#[derive(Clone)]
pub struct TaskContext {
	pub pg_pool: bestool_postgres::pool::PgPool,
	pub http_client: reqwest::Client,
	pub canopy_client: Option<Arc<CanopyClient>>,
}

impl TaskContext {
	pub(crate) fn from_internal(ctx: &InternalContext) -> Self {
		Self {
			pg_pool: ctx.pg_pool.clone(),
			http_client: ctx.http_client.clone(),
			canopy_client: ctx.canopy_client.clone(),
		}
	}
}

/// A periodic background task registered with the daemon.
///
/// The daemon spawns one tokio task per registered plugin at startup and ticks
/// it at `interval()`. Each tick counts as activity for the watchdog. Errors
/// returned from `run` are logged but don't kill the daemon.
pub trait BackgroundTask: Send + Sync + 'static {
	fn name(&self) -> &'static str;
	fn interval(&self) -> Duration;
	fn run<'a>(&'a self, ctx: &'a TaskContext) -> BoxFuture<'a, Result<()>>;
}
