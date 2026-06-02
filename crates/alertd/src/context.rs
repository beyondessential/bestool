use std::sync::Arc;

use crate::canopy::CanopyClient;

/// Shared resources the daemon holds for the lifetime of the process and hands
/// to background tasks and HTTP endpoints.
#[derive(Debug, Clone)]
pub struct InternalContext {
	pub pg_pool: bestool_postgres::pool::PgPool,
	pub http_client: reqwest::Client,
	pub canopy_client: Option<Arc<CanopyClient>>,
}
