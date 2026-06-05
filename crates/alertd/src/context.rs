use std::sync::Arc;

use crate::canopy::CanopyClient;

/// Shared resources the daemon holds for the lifetime of the process and hands
/// to background tasks and HTTP endpoints.
#[derive(Debug, Clone)]
pub struct InternalContext {
	/// `None` on hosts with no Tamanu deployment (and therefore no database).
	pub pg_pool: Option<bestool_postgres::pool::PgPool>,
	pub http_client: reqwest::Client,
	pub canopy_client: Option<Arc<CanopyClient>>,
}
