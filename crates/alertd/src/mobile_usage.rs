//! How many mobile devices are syncing to a central.
//!
//! The mobile flag is unindexed JSON, so counting a month of sessions in one
//! query reads every facility session of that month too: gigabytes on a large
//! central. Each sweep instead reads only the sessions completed since the last
//! one, through the `completed_at` index, and keeps when each device was last
//! seen. Only a central with no stored state reads the whole month.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::store::{CheckStore, CheckStoreJson, Lifetime};

const WINDOW_SECS: i64 = 30 * 24 * 60 * 60;

// A session completed just before a sweep can commit after that sweep read past
// it, so each read starts this far behind the last.
const OVERLAP_SECS: i64 = 5 * 60;

const KEY: &str = "mobile_usage";

const SQL: &str = "SELECT parameters->>'deviceId', \
	extract(epoch FROM max(completed_at))::bigint \
	FROM sync_sessions \
	WHERE completed_at > to_timestamp($1) \
	AND parameters->>'isMobile' = 'true' \
	AND parameters->>'deviceId' IS NOT NULL \
	GROUP BY 1";

/// Each device's last completed sync, and how far the sessions have been read,
/// in seconds since the epoch on the database's clock.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Seen {
	read_to: i64,
	devices: BTreeMap<String, i64>,
}

/// Distinct mobile devices with a sync session completed in the last 30 days.
pub async fn active_devices(
	client: &tokio_postgres::Client,
	store: &dyn CheckStore,
) -> Option<i64> {
	let stored: Option<Seen> = store.get_json(KEY).await;
	match read(client, stored.as_ref()).await {
		Ok(seen) => {
			store.put_json(KEY, &seen, Lifetime::Durable).await;
			Some(seen.devices.len() as i64)
		}
		Err(err) => {
			warn!("counting mobile devices: {err}");
			stored.map(|seen| seen.devices.len() as i64)
		}
	}
}

async fn read(
	client: &tokio_postgres::Client,
	stored: Option<&Seen>,
) -> Result<Seen, tokio_postgres::Error> {
	let now: i64 = client
		.query_one("SELECT extract(epoch FROM now())::bigint", &[])
		.await?
		.try_get(0)?;
	let since = match stored {
		Some(seen) => seen.read_to - OVERLAP_SECS,
		None => now - WINDOW_SECS,
	};

	let mut devices = stored.map(|seen| seen.devices.clone()).unwrap_or_default();
	for row in client.query(SQL, &[&(since as f64)]).await? {
		let last: i64 = row.try_get(1)?;
		let seen = devices.entry(row.try_get(0)?).or_insert(last);
		*seen = (*seen).max(last);
	}
	devices.retain(|_, last| *last > now - WINDOW_SECS);

	Ok(Seen {
		read_to: now,
		devices,
	})
}

#[cfg(test)]
mod tests {
	use std::collections::BTreeMap;

	use super::{KEY, Seen, active_devices};
	use crate::checks::test_support::central_ctx;
	use crate::store::{CheckStoreJson, Lifetime, MemoryStore};

	// A temp table shadows the real one for this connection only, so the query
	// runs over seeded sessions without touching the database's own.
	const SEED: &str = "CREATE TEMP TABLE sync_sessions (parameters jsonb, completed_at timestamptz); \
		INSERT INTO sync_sessions VALUES \
		('{\"deviceId\": \"tab-a\", \"isMobile\": true}', now() - '10 minutes'::interval), \
		('{\"deviceId\": \"tab-a\", \"isMobile\": true}', now() - '2 days'::interval), \
		('{\"deviceId\": \"tab-b\", \"isMobile\": true}', now() - '29 days'::interval), \
		('{\"deviceId\": \"tab-old\", \"isMobile\": true}', now() - '31 days'::interval), \
		('{\"deviceId\": \"tab-open\", \"isMobile\": true}', NULL), \
		('{\"deviceId\": \"facility-1\", \"isMobile\": false}', now() - '1 day'::interval), \
		('{\"deviceId\": \"facility-2\"}', now() - '1 day'::interval)";

	#[tokio::test]
	async fn with_nothing_stored_it_reads_the_whole_month() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let client = ctx.db().await.expect("central connection");
		client.batch_execute(SEED).await.expect("seed");

		let store = MemoryStore::new();
		assert_eq!(active_devices(&client, &store).await, Some(2));
		let seen: Seen = store.get_json(KEY).await.expect("state stored");
		assert_eq!(
			seen.devices.keys().collect::<Vec<_>>(),
			vec!["tab-a", "tab-b"]
		);
	}

	#[tokio::test]
	async fn with_state_stored_it_reads_only_since_and_forgets_after_a_month() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let client = ctx.db().await.expect("central connection");
		client.batch_execute(SEED).await.expect("seed");
		let now: i64 = client
			.query_one("SELECT extract(epoch FROM now())::bigint", &[])
			.await
			.unwrap()
			.get(0);
		let day = 24 * 60 * 60;

		let store = MemoryStore::new();
		store
			.put_json(
				KEY,
				&Seen {
					read_to: now - 60 * 60,
					devices: BTreeMap::from([
						("tab-kept".to_string(), now - 10 * day),
						("tab-gone".to_string(), now - 31 * day),
					]),
				},
				Lifetime::Durable,
			)
			.await;

		// tab-b synced 29 days ago, before the stored read, so it is not read
		// again; tab-a synced since.
		assert_eq!(active_devices(&client, &store).await, Some(2));
		let seen: Seen = store.get_json(KEY).await.unwrap();
		assert_eq!(
			seen.devices.keys().collect::<Vec<_>>(),
			vec!["tab-a", "tab-kept"]
		);
		assert!(seen.read_to >= now);
	}
}
