use std::collections::VecDeque;
use sysinfo::System;
use tokio_postgres::Row;
use tracing::warn;

pub struct StoredResult {
	pub query: String,
	pub rows: Vec<Row>,
	pub estimated_size: usize,
	pub timestamp: jiff::Timestamp,
	pub duration: std::time::Duration,
}

impl std::fmt::Debug for StoredResult {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("StoredResult")
			.field("query", &self.query)
			.field("row_count", &self.rows.len())
			.field("estimated_size", &self.estimated_size)
			.field("timestamp", &self.timestamp)
			.field("duration", &self.duration)
			.finish()
	}
}

impl Clone for StoredResult {
	fn clone(&self) -> Self {
		Self {
			query: self.query.clone(),
			rows: self.rows.clone(),
			estimated_size: self.estimated_size,
			timestamp: self.timestamp,
			duration: self.duration,
		}
	}
}

impl StoredResult {
	pub fn from_query_result(query: String, rows: Vec<Row>, duration: std::time::Duration) -> Self {
		let estimated_size = estimate_result_size(&rows);

		Self {
			query,
			rows,
			estimated_size,
			timestamp: jiff::Timestamp::now(),
			duration,
		}
	}
}

#[derive(Debug)]
pub struct ResultStore {
	results: VecDeque<StoredResult>,
	total_size: usize,
	max_size: usize,
}

impl Clone for ResultStore {
	fn clone(&self) -> Self {
		Self::new()
	}
}

impl Default for ResultStore {
	fn default() -> Self {
		Self::new()
	}
}

impl ResultStore {
	pub fn new() -> Self {
		let max_size = calculate_max_size();
		if max_size < 50 * 1024 * 1024 {
			warn!("Less than 50MB available for result storage (5% of available system memory)");
			warn!("You might only be able to store a few results and may run out of memory");
		}

		Self {
			results: VecDeque::new(),
			total_size: 0,
			max_size,
		}
	}

	pub fn push(&mut self, query: String, rows: Vec<Row>, duration: std::time::Duration) {
		let result = StoredResult::from_query_result(query, rows, duration);
		let result_size = result.estimated_size;

		while self.total_size + result_size > self.max_size && !self.results.is_empty() {
			if let Some(removed) = self.results.pop_front() {
				self.total_size = self.total_size.saturating_sub(removed.estimated_size);
			}
		}

		self.total_size += result_size;
		self.results.push_back(result);
	}

	pub fn get(&self, index: usize) -> Option<&StoredResult> {
		self.results.get(index)
	}

	pub fn get_last(&self) -> Option<&StoredResult> {
		self.results.back()
	}

	pub fn len(&self) -> usize {
		self.results.len()
	}

	pub fn is_empty(&self) -> bool {
		self.results.is_empty()
	}

	pub fn iter(&self) -> impl Iterator<Item = &StoredResult> {
		self.results.iter()
	}

	pub fn total_size(&self) -> usize {
		self.total_size
	}

	pub fn max_size(&self) -> usize {
		self.max_size
	}

	#[cfg(test)]
	pub fn clear(&mut self) {
		self.results.clear();
		self.total_size = 0;
	}

	#[cfg(test)]
	pub fn set_max_size(&mut self, size: usize) {
		self.max_size = size;
	}
}

fn calculate_max_size() -> usize {
	const ONE_GB: usize = 1024 * 1024 * 1024;
	const FIVE_PERCENT: f64 = 0.05;

	let mut sys = System::new_all();
	sys.refresh_memory();

	let available_memory = sys.available_memory() as usize;
	let five_percent = (available_memory as f64 * FIVE_PERCENT) as usize;

	std::cmp::min(five_percent, ONE_GB)
}

fn estimate_result_size(rows: &[Row]) -> usize {
	let mut size = 0;

	for row in rows {
		size += std::mem::size_of::<Row>();

		for i in 0..row.len() {
			if let Ok(v) = row.try_get::<_, String>(i) {
				size += v.len();
			} else if let Ok(v) = row.try_get::<_, Vec<u8>>(i) {
				size += v.len();
			} else if let Ok(v) = row.try_get::<_, Vec<String>>(i) {
				size += v.iter().map(|s| s.len()).sum::<usize>();
			} else if let Ok(v) = row.try_get::<_, serde_json::Value>(i) {
				size += serde_json::to_string(&v).map(|s| s.len()).unwrap_or(64);
			} else if row.try_get::<_, i16>(i).is_ok()
				|| row.try_get::<_, i32>(i).is_ok()
				|| row.try_get::<_, i64>(i).is_ok()
				|| row.try_get::<_, f32>(i).is_ok()
				|| row.try_get::<_, f64>(i).is_ok()
				|| row.try_get::<_, jiff::Timestamp>(i).is_ok()
				|| row.try_get::<_, jiff::civil::DateTime>(i).is_ok()
				|| row.try_get::<_, jiff::civil::Time>(i).is_ok()
			{
				size += 8;
			} else if row.try_get::<_, bool>(i).is_ok() {
				size += 1;
			} else if row.try_get::<_, jiff::civil::Date>(i).is_ok() {
				size += 4;
			} else {
				size += 64;
			}
		}
	}

	size
}

#[cfg(test)]
mod tests {
	use super::*;

	async fn create_test_client() -> tokio_postgres::Client {
		let (client, connection) = tokio_postgres::connect(
			&std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
			tokio_postgres::NoTls,
		)
		.await
		.expect("Failed to connect to database");

		tokio::spawn(async move {
			if let Err(e) = connection.await {
				eprintln!("connection error: {}", e);
			}
		});

		client
	}

	#[test]
	fn test_result_store_new() {
		let store = ResultStore::new();
		assert_eq!(store.len(), 0);
		assert!(store.is_empty());
		assert!(store.max_size() > 0);
	}

	#[tokio::test]
	async fn test_result_store_push_and_get() {
		let client = create_test_client().await;
		let rows = client.query("SELECT 1 as num", &[]).await.unwrap();

		let mut store = ResultStore::new();
		store.push(
			"SELECT 1 as num".to_string(),
			rows,
			std::time::Duration::from_millis(10),
		);

		assert_eq!(store.len(), 1);
		assert!(store.get(0).is_some());
		assert_eq!(store.get(0).unwrap().query, "SELECT 1 as num");
	}

	#[tokio::test]
	async fn test_result_store_get_last() {
		let client = create_test_client().await;
		let rows1 = client.query("SELECT 1", &[]).await.unwrap();
		let rows2 = client.query("SELECT 2", &[]).await.unwrap();

		let mut store = ResultStore::new();
		store.push(
			"SELECT 1".to_string(),
			rows1,
			std::time::Duration::from_millis(10),
		);
		store.push(
			"SELECT 2".to_string(),
			rows2,
			std::time::Duration::from_millis(10),
		);

		assert_eq!(store.get_last().unwrap().query, "SELECT 2");
	}

	#[tokio::test]
	async fn test_result_store_iter() {
		let client = create_test_client().await;
		let rows1 = client.query("SELECT 1", &[]).await.unwrap();
		let rows2 = client.query("SELECT 2", &[]).await.unwrap();

		let mut store = ResultStore::new();
		store.push(
			"SELECT 1".to_string(),
			rows1,
			std::time::Duration::from_millis(10),
		);
		store.push(
			"SELECT 2".to_string(),
			rows2,
			std::time::Duration::from_millis(10),
		);

		let queries: Vec<String> = store.iter().map(|r| r.query.clone()).collect();
		assert_eq!(queries, vec!["SELECT 1", "SELECT 2"]);
	}

	#[tokio::test]
	async fn test_estimate_result_size() {
		let client = create_test_client().await;
		let rows = client.query("SELECT 'hello' as text", &[]).await.unwrap();

		let stored = StoredResult::from_query_result(
			"SELECT 'hello' as text".to_string(),
			rows,
			std::time::Duration::from_millis(10),
		);
		assert!(stored.estimated_size > 0);
		assert!(stored.estimated_size < 1000);
	}

	#[test]
	fn test_calculate_max_size() {
		let max_size = calculate_max_size();
		const ONE_GB: usize = 1024 * 1024 * 1024;

		assert!(max_size > 0);
		assert!(max_size <= ONE_GB);
	}

	#[tokio::test]
	async fn test_result_store_eviction() {
		let client = create_test_client().await;

		let mut store = ResultStore::new();
		store.set_max_size(1000);

		let large_query = "SELECT string_agg(chr(65 + i % 26), '') FROM generate_series(1, 200) i";
		let rows1 = client.query(large_query, &[]).await.unwrap();
		let rows2 = client.query(large_query, &[]).await.unwrap();

		store.push(
			"query1".to_string(),
			rows1,
			std::time::Duration::from_millis(10),
		);
		store.push(
			"query2".to_string(),
			rows2,
			std::time::Duration::from_millis(10),
		);

		assert!(store.total_size() <= store.max_size());
	}

	#[tokio::test]
	async fn test_stored_result_cloneable() {
		let client = create_test_client().await;
		let rows = client
			.query("SELECT 42 as num, 'test' as text", &[])
			.await
			.unwrap();

		let stored = StoredResult::from_query_result(
			"SELECT 42 as num, 'test' as text".to_string(),
			rows,
			std::time::Duration::from_millis(10),
		);

		let cloned = stored.clone();
		assert_eq!(stored.query, cloned.query);
		assert_eq!(stored.rows.len(), cloned.rows.len());
	}

	#[tokio::test]
	async fn test_row_is_cloneable() {
		let client = create_test_client().await;
		let rows = client.query("SELECT 1 as num", &[]).await.unwrap();
		let row1 = &rows[0];
		let row2 = row1.clone();

		let val1: i32 = row1.get(0);
		let val2: i32 = row2.get(0);
		assert_eq!(val1, val2);
	}
}
