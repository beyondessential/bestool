//! How many mobile devices are syncing to a central.
//!
//! The count spans a month of sync sessions, far more rows than any check
//! reads, so it is taken a few times a day and the last answer is reported
//! on every sweep in between. Canopy keeps only each source's latest push, so
//! leaving the figure off a sweep would blank it.

use std::{
	sync::Mutex,
	time::{Duration, Instant},
};

use tracing::warn;

const WINDOW_DAYS: f64 = 30.0;

const REFRESH: Duration = Duration::from_secs(6 * 60 * 60);

// `completed_at` is indexed on every Tamanu version, so this reads the window
// and nothing older.
const SQL: &str = "SELECT count(DISTINCT parameters->>'deviceId') FROM sync_sessions \
	WHERE completed_at > now() - ($1::float8 * interval '1 day') \
	AND parameters->>'isMobile' = 'true'";

static LAST: Mutex<Option<(Instant, i64)>> = Mutex::new(None);

/// Distinct mobile devices with a sync session completed in the last 30 days.
pub async fn active_devices(client: &tokio_postgres::Client) -> Option<i64> {
	let cached = *LAST.lock().unwrap();
	if let Some((at, count)) = cached
		&& at.elapsed() < REFRESH
	{
		return Some(count);
	}

	match client.query_one(SQL, &[&WINDOW_DAYS]).await {
		Ok(row) => match row.try_get::<_, i64>(0) {
			Ok(count) => {
				*LAST.lock().unwrap() = Some((Instant::now(), count));
				Some(count)
			}
			Err(err) => {
				warn!("decoding mobile device count: {err}");
				cached.map(|(_, count)| count)
			}
		},
		Err(err) => {
			warn!("querying mobile device count: {err}");
			cached.map(|(_, count)| count)
		}
	}
}

#[cfg(test)]
mod tests {
	use std::time::Instant;

	use super::{LAST, active_devices};
	use crate::checks::test_support::central_ctx;

	#[tokio::test]
	async fn counts_against_central_then_serves_the_cached_count() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let client = ctx.db().await.expect("central connection");

		*LAST.lock().unwrap() = None;
		let counted = active_devices(&client).await.expect("count");
		assert!(counted >= 0);

		*LAST.lock().unwrap() = Some((Instant::now(), 41));
		assert_eq!(active_devices(&client).await, Some(41));
	}
}
