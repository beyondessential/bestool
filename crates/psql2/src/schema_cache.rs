use miette::{IntoDiagnostic, Result};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, warn};

use crate::pool::PgPool;

/// Cached database schema information
#[derive(Debug, Clone, Default)]
pub struct SchemaCache {
	/// Table names by schema (schema_name -> table_names)
	pub tables: HashMap<String, Vec<String>>,
	/// View names by schema
	pub views: HashMap<String, Vec<String>>,
	/// Column names by table (qualified_table_name -> column_names)
	pub columns: HashMap<String, Vec<String>>,
	/// Function names
	pub functions: Vec<String>,
	/// Schema names
	pub schemas: Vec<String>,
}

impl SchemaCache {
	/// Create a new empty cache
	pub fn new() -> Self {
		Self::default()
	}

	/// Get all table names (across all schemas)
	pub fn all_tables(&self) -> Vec<String> {
		self.tables.values().flatten().cloned().collect()
	}

	/// Get all view names (across all schemas)
	pub fn all_views(&self) -> Vec<String> {
		self.views.values().flatten().cloned().collect()
	}

	/// Get all column names for a given table
	#[allow(dead_code)]
	pub fn columns_for_table(&self, table: &str) -> Option<&Vec<String>> {
		self.columns
			.get(table)
			.or_else(|| self.columns.get(&format!("public.{table}")))
			.or_else(|| {
				for schema in &self.schemas {
					if let Some(cols) = self.columns.get(&format!("{schema}.{table}")) {
						return Some(cols);
					}
				}
				None
			})
	}
}

/// Schema cache manager that runs queries on a dedicated background connection
pub struct SchemaCacheManager {
	cache: Arc<RwLock<SchemaCache>>,
	pool: PgPool,
}

impl SchemaCacheManager {
	/// Create a new cache manager
	pub fn new(pool: PgPool) -> Self {
		Self {
			cache: Arc::new(RwLock::new(SchemaCache::new())),
			pool,
		}
	}

	/// Get an Arc to the cache for sharing
	pub fn cache_arc(&self) -> Arc<RwLock<SchemaCache>> {
		self.cache.clone()
	}

	/// Start background refresh task
	pub fn start_background_refresh(self) -> tokio::task::JoinHandle<()> {
		tokio::spawn(async move {
			if let Err(e) = self.refresh_loop().await {
				warn!("schema cache refresh task failed: {e}");
			}
		})
	}

	/// Continuously refresh the schema cache
	async fn refresh_loop(&self) -> Result<()> {
		loop {
			if let Err(e) = self.refresh().await {
				warn!("failed to refresh schema cache: {e}");
			}

			tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
		}
	}

	/// Refresh the schema cache by querying the database
	pub async fn refresh(&self) -> Result<()> {
		debug!("refreshing schema cache");

		let client = self.pool.get().await.into_diagnostic()?;

		let mut new_cache = SchemaCache::new();

		if let Ok(schemas) = self.query_schemas(&client).await {
			new_cache.schemas = schemas;
			debug!(count = new_cache.schemas.len(), "loaded schemas");
		}

		if let Ok(tables) = self.query_tables(&client).await {
			new_cache.tables = tables;
			let total: usize = new_cache.tables.values().map(|v| v.len()).sum();
			debug!(count = total, "loaded tables");
		}

		if let Ok(views) = self.query_views(&client).await {
			new_cache.views = views;
			let total: usize = new_cache.views.values().map(|v| v.len()).sum();
			debug!(count = total, "loaded views");
		}

		if let Ok(columns) = self.query_columns(&client).await {
			new_cache.columns = columns;
			debug!(count = new_cache.columns.len(), "loaded column mappings");
		}

		if let Ok(functions) = self.query_functions(&client).await {
			new_cache.functions = functions;
			debug!(count = new_cache.functions.len(), "loaded functions");
		}

		*self.cache.write().unwrap() = new_cache;

		debug!("schema cache refresh complete");
		Ok(())
	}

	/// Query all schema names
	async fn query_schemas(&self, client: &tokio_postgres::Client) -> Result<Vec<String>> {
		let rows = client
			.query(
				"SELECT schema_name FROM information_schema.schemata \
                 WHERE schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast') \
                 ORDER BY schema_name",
				&[],
			)
			.await
			.into_diagnostic()?;

		Ok(rows.into_iter().map(|r| r.get(0)).collect())
	}

	/// Query all tables by schema
	async fn query_tables(
		&self,
		client: &tokio_postgres::Client,
	) -> Result<HashMap<String, Vec<String>>> {
		let rows = client
			.query(
				"SELECT schemaname, tablename FROM pg_tables \
                 WHERE schemaname NOT IN ('pg_catalog', 'information_schema') \
                 ORDER BY schemaname, tablename",
				&[],
			)
			.await
			.into_diagnostic()?;

		let mut tables: HashMap<String, Vec<String>> = HashMap::new();
		for row in rows {
			let schemaname: String = row.get(0);
			let tablename: String = row.get(1);
			tables.entry(schemaname).or_default().push(tablename);
		}

		Ok(tables)
	}

	/// Query all views by schema
	async fn query_views(
		&self,
		client: &tokio_postgres::Client,
	) -> Result<HashMap<String, Vec<String>>> {
		let rows = client
			.query(
				"SELECT schemaname, viewname FROM pg_views \
                 WHERE schemaname NOT IN ('pg_catalog', 'information_schema') \
                 ORDER BY schemaname, viewname",
				&[],
			)
			.await
			.into_diagnostic()?;

		let mut views: HashMap<String, Vec<String>> = HashMap::new();
		for row in rows {
			let schemaname: String = row.get(0);
			let viewname: String = row.get(1);
			views.entry(schemaname).or_default().push(viewname);
		}

		Ok(views)
	}

	/// Query all columns for all tables
	async fn query_columns(
		&self,
		client: &tokio_postgres::Client,
	) -> Result<HashMap<String, Vec<String>>> {
		let rows = client
			.query(
				"SELECT table_schema, table_name, column_name \
                 FROM information_schema.columns \
                 WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
                 ORDER BY table_schema, table_name, ordinal_position",
				&[],
			)
			.await
			.into_diagnostic()?;

		let mut columns: HashMap<String, Vec<String>> = HashMap::new();
		for row in rows {
			let table_schema: String = row.get(0);
			let table_name: String = row.get(1);
			let column_name: String = row.get(2);

			let qualified = format!("{table_schema}.{table_name}");
			columns
				.entry(qualified)
				.or_default()
				.push(column_name.clone());

			if table_schema == "public" {
				columns
					.entry(table_name.clone())
					.or_default()
					.push(column_name);
			}
		}

		Ok(columns)
	}

	/// Query all function names
	async fn query_functions(&self, client: &tokio_postgres::Client) -> Result<Vec<String>> {
		let rows = client
			.query(
				"SELECT DISTINCT proname FROM pg_proc \
                 JOIN pg_namespace ON pg_proc.pronamespace = pg_namespace.oid \
                 WHERE pg_namespace.nspname NOT IN ('pg_catalog', 'information_schema') \
                 ORDER BY proname",
				&[],
			)
			.await
			.into_diagnostic()?;

		Ok(rows.into_iter().map(|r| r.get(0)).collect())
	}
}

impl Clone for SchemaCacheManager {
	fn clone(&self) -> Self {
		Self {
			cache: self.cache.clone(),
			pool: self.pool.clone(),
		}
	}
}
