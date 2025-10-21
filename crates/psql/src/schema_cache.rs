use miette::{IntoDiagnostic, Result};
use serde::Deserialize;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::Write;
use std::sync::{Arc, Mutex, RwLock};
use tempfile::NamedTempFile;
use tracing::debug;

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
		// Try unqualified name first
		self.columns
			.get(table)
			// Then try with public schema
			.or_else(|| self.columns.get(&format!("public.{}", table)))
			// Then try all schemas
			.or_else(|| {
				for schema in &self.schemas {
					if let Some(cols) = self.columns.get(&format!("{}.{}", schema, table)) {
						return Some(cols);
					}
				}
				None
			})
	}
}

/// Schema cache manager that queries through psql PTY
pub struct SchemaCacheManager {
	cache: Arc<RwLock<SchemaCache>>,
	pty_writer: Arc<Mutex<Box<dyn Write + Send>>>,
	print_enabled: Arc<Mutex<bool>>,
	write_mode: Arc<Mutex<bool>>,
	output_buffer: Arc<Mutex<VecDeque<u8>>>,
	boundary: String,
}

#[derive(Debug, Deserialize)]
struct TableRow {
	schemaname: String,
	tablename: String,
}

#[derive(Debug, Deserialize)]
struct ViewRow {
	schemaname: String,
	viewname: String,
}

#[derive(Debug, Deserialize)]
struct ColumnRow {
	table_schema: String,
	table_name: String,
	column_name: String,
}

#[derive(Debug, Deserialize)]
struct FunctionRow {
	proname: String,
}

#[derive(Debug, Deserialize)]
struct SchemaRow {
	schema_name: String,
}

impl SchemaCacheManager {
	/// Create a new cache manager with PTY writer access
	pub fn new(
		pty_writer: Arc<Mutex<Box<dyn Write + Send>>>,
		print_enabled: Arc<Mutex<bool>>,
		write_mode: Arc<Mutex<bool>>,
		output_buffer: Arc<Mutex<VecDeque<u8>>>,
		boundary: String,
	) -> Self {
		Self {
			cache: Arc::new(RwLock::new(SchemaCache::new())),
			pty_writer,
			print_enabled,
			write_mode,
			output_buffer,
			boundary,
		}
	}

	/// Get an Arc to the cache for sharing
	pub fn cache_arc(&self) -> Arc<RwLock<SchemaCache>> {
		self.cache.clone()
	}

	/// Refresh the schema cache by querying through psql
	pub fn refresh(&self) -> Result<()> {
		debug!("refreshing schema cache");

		// Disable output printing during schema refresh
		*self.print_enabled.lock().unwrap() = false;

		// Wait a moment for reader thread to see the flag
		std::thread::sleep(std::time::Duration::from_millis(50));

		// Clear the output buffer to discard any buffered content
		self.output_buffer.lock().unwrap().clear();

		// Guard to ensure printing is always re-enabled
		struct PrintGuard(Arc<Mutex<bool>>);
		impl Drop for PrintGuard {
			fn drop(&mut self) {
				*self.0.lock().unwrap() = true;
			}
		}
		let _guard = PrintGuard(self.print_enabled.clone());

		let mut new_cache = SchemaCache::new();

		if let Ok(schemas) = self.query_schemas() {
			new_cache.schemas = schemas;
			debug!(count = new_cache.schemas.len(), "loaded schemas");
		}

		if let Ok(tables) = self.query_tables() {
			new_cache.tables = tables;
			let total: usize = new_cache.tables.values().map(|v| v.len()).sum();
			debug!(count = total, "loaded tables");
		}

		if let Ok(views) = self.query_views() {
			new_cache.views = views;
			let total: usize = new_cache.views.values().map(|v| v.len()).sum();
			debug!(count = total, "loaded views");
		}

		if let Ok(columns) = self.query_columns() {
			new_cache.columns = columns;
			debug!(count = new_cache.columns.len(), "loaded column mappings");
		}

		if let Ok(functions) = self.query_functions() {
			new_cache.functions = functions;
			debug!(count = new_cache.functions.len(), "loaded functions");
		}

		*self.cache.write().unwrap() = new_cache;

		if *self.write_mode.lock().unwrap() {
			debug!("issuing ROLLBACK after schema refresh in write mode");
			let rollback_cmd = "ROLLBACK;\n";
			let mut writer = self.pty_writer.lock().unwrap();
			writer
				.write_all(rollback_cmd.as_bytes())
				.into_diagnostic()?;
			writer.flush().into_diagnostic()?;

			// Give psql time to process the rollback
			std::thread::sleep(std::time::Duration::from_millis(50));
		}

		debug!("schema cache refresh complete");
		Ok(())
	}

	/// Query all schema names
	fn query_schemas(&self) -> Result<Vec<String>> {
		let rows: Vec<SchemaRow> = self.query_json(
			"SELECT schema_name FROM information_schema.schemata \
             WHERE schema_name NOT IN ('pg_catalog', 'information_schema', 'pg_toast') \
             ORDER BY schema_name",
		)?;

		Ok(rows.into_iter().map(|r| r.schema_name).collect())
	}

	/// Query all tables by schema
	fn query_tables(&self) -> Result<HashMap<String, Vec<String>>> {
		let rows: Vec<TableRow> = self.query_json(
			"SELECT schemaname, tablename FROM pg_tables \
             WHERE schemaname NOT IN ('pg_catalog', 'information_schema') \
             ORDER BY schemaname, tablename",
		)?;

		let mut tables: HashMap<String, Vec<String>> = HashMap::new();
		for row in rows {
			tables
				.entry(row.schemaname)
				.or_default()
				.push(row.tablename);
		}

		Ok(tables)
	}

	/// Query all views by schema
	fn query_views(&self) -> Result<HashMap<String, Vec<String>>> {
		let rows: Vec<ViewRow> = self.query_json(
			"SELECT schemaname, viewname FROM pg_views \
             WHERE schemaname NOT IN ('pg_catalog', 'information_schema') \
             ORDER BY schemaname, viewname",
		)?;

		let mut views: HashMap<String, Vec<String>> = HashMap::new();
		for row in rows {
			views.entry(row.schemaname).or_default().push(row.viewname);
		}

		Ok(views)
	}

	/// Query all columns for all tables
	fn query_columns(&self) -> Result<HashMap<String, Vec<String>>> {
		let rows: Vec<ColumnRow> = self.query_json(
			"SELECT table_schema, table_name, column_name \
             FROM information_schema.columns \
             WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
             ORDER BY table_schema, table_name, ordinal_position",
		)?;

		let mut columns: HashMap<String, Vec<String>> = HashMap::new();
		for row in rows {
			// Store both qualified and unqualified names
			let qualified = format!("{}.{}", row.table_schema, row.table_name);
			columns
				.entry(qualified)
				.or_default()
				.push(row.column_name.clone());

			// Also store unqualified for easier lookup (public schema priority)
			if row.table_schema == "public" {
				columns
					.entry(row.table_name.clone())
					.or_default()
					.push(row.column_name);
			}
		}

		Ok(columns)
	}

	/// Query all function names
	fn query_functions(&self) -> Result<Vec<String>> {
		let rows: Vec<FunctionRow> = self.query_json(
			"SELECT DISTINCT proname FROM pg_proc \
             JOIN pg_namespace ON pg_proc.pronamespace = pg_namespace.oid \
             WHERE pg_namespace.nspname NOT IN ('pg_catalog', 'information_schema') \
             ORDER BY proname",
		)?;

		Ok(rows.into_iter().map(|r| r.proname).collect())
	}

	/// Execute a query and parse JSON results from a temp file
	fn query_json<T: for<'de> Deserialize<'de>>(&self, query: &str) -> Result<Vec<T>> {
		let temp_file = NamedTempFile::new().into_diagnostic()?;
		let temp_path = temp_file.path().to_path_buf();

		debug!(query = %query, "executing schema query");

		let commands = format!(
			"\\t\n\\a\n\\o {}\nSELECT json_agg(t) FROM ({}) t;\n\\o\n\\t\n\\a\n",
			temp_path.display(),
			query
		);

		{
			let mut writer = self.pty_writer.lock().unwrap();
			writer.write_all(commands.as_bytes()).into_diagnostic()?;
			writer.flush().into_diagnostic()?;
		}

		// Wait for psql to return to prompt (check for boundary marker)
		let boundary_marker = format!("<<<{}|||", self.boundary);
		let timeout = std::time::Duration::from_secs(10);
		let start = std::time::Instant::now();

		loop {
			if start.elapsed() > timeout {
				return Err(miette::miette!(
					"timeout waiting for schema query to complete"
				));
			}

			let buffer = self.output_buffer.lock().unwrap();
			let buffer_vec: Vec<u8> = buffer.iter().copied().collect();
			let buffer_str = String::from_utf8_lossy(&buffer_vec);

			if buffer_str.contains(&boundary_marker) {
				drop(buffer);
				// Small delay to ensure psql has flushed the file
				std::thread::sleep(std::time::Duration::from_millis(100));
				break;
			}
			drop(buffer);

			std::thread::sleep(std::time::Duration::from_millis(50));
		}

		// Read and parse the JSON file
		let content = fs::read_to_string(&temp_path).into_diagnostic()?;
		let trimmed = content.trim();

		// Handle empty results (psql outputs "null" for empty json_agg)
		if trimmed.is_empty() || trimmed == "null" {
			return Ok(Vec::new());
		}

		// Parse JSON array
		let results: Vec<T> = serde_json::from_str(trimmed).into_diagnostic()?;

		Ok(results)
	}
}

impl Clone for SchemaCacheManager {
	fn clone(&self) -> Self {
		Self {
			cache: self.cache.clone(),
			pty_writer: self.pty_writer.clone(),
			print_enabled: self.print_enabled.clone(),
			write_mode: self.write_mode.clone(),
			output_buffer: self.output_buffer.clone(),
			boundary: self.boundary.clone(),
		}
	}
}
