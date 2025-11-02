use std::io::Write;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use miette::{IntoDiagnostic, Result};
use tracing::{debug, info};

use super::{Audit, AuditEntry, AuditEntryWithTimestamp};

/// Parse a timestamp string (RFC3339, date, or datetime)
fn parse_timestamp(s: &str) -> Result<Timestamp> {
	// Try parsing as timestamp first
	if let Ok(ts) = s.parse::<Timestamp>() {
		return Ok(ts);
	}

	// Try parsing as datetime string
	jiff::civil::DateTime::strptime("%Y-%m-%d %H:%M:%S", s)
		.or_else(|_| jiff::civil::DateTime::strptime("%Y-%m-%d", s))
		.into_diagnostic()?
		.to_zoned(jiff::tz::TimeZone::system())
		.into_diagnostic()
		.map(|zdt| zdt.timestamp())
}

/// Options for querying audit entries
#[derive(Debug, Clone, Default)]
pub struct QueryOptions {
	/// Maximum number of entries to return
	pub limit: Option<usize>,
	/// Start from the oldest entries instead of newest
	pub from_oldest: bool,
	/// Filter entries after this date (parseable by jiff)
	pub since: Option<String>,
	/// Filter entries before this date (parseable by jiff)
	pub until: Option<String>,
}

/// Options for exporting audit entries
#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
	/// Path to audit database directory
	pub audit_path: Option<PathBuf>,
	/// Query options for filtering entries
	pub query_options: QueryOptions,
	/// Discover and read orphan databases instead of main database
	pub orphans: bool,
}

impl Audit {
	/// Query audit entries with filtering options
	///
	/// Returns entries as (timestamp, entry) tuples, always in chronological order (oldest first).
	/// The `from_oldest` option determines which end of the result set to take from.
	/// A limit of 0 means unlimited (return all entries).
	pub fn query(&self, options: &QueryOptions) -> Result<Vec<(u64, AuditEntry)>> {
		debug!(?options, "querying audit entries");

		// Get all entries
		let mut entries = self.list()?;

		// Apply timestamp filters
		if let Some(ref since_str) = options.since {
			let since = parse_timestamp(since_str)?;
			let since_micros = since.as_microsecond() as u64;
			entries.retain(|(ts, _)| *ts >= since_micros);
		}

		if let Some(ref until_str) = options.until {
			let until = parse_timestamp(until_str)?;
			let until_micros = until.as_microsecond() as u64;
			entries.retain(|(ts, _)| *ts <= until_micros);
		}

		// Apply limit (0 means unlimited)
		if let Some(limit) = options.limit
			&& limit > 0
		{
			if options.from_oldest {
				// Take first N entries (oldest)
				entries.truncate(limit);
			} else {
				// Take last N entries (newest), but result is still oldest-first
				let start = entries.len().saturating_sub(limit);
				entries.drain(..start);
			}
		}

		debug!(count = entries.len(), "returning audit entries");
		Ok(entries)
	}

	/// Find orphan audit databases in the given directory
	///
	/// Returns paths to orphaned databases that can be opened and queried.
	pub fn find_orphans(audit_dir: impl AsRef<Path>) -> Result<Vec<std::path::PathBuf>> {
		let audit_dir = audit_dir.as_ref();
		let main_path = Self::main_db_path(audit_dir);

		super::multi_process::WorkingDatabase::find_orphan_databases(&main_path)
	}

	/// Open an audit database from a specific file (useful for orphans)
	pub fn open_file(path: impl AsRef<Path>) -> Result<Self> {
		let path = path.as_ref();
		debug!(?path, "opening audit database file");

		let db = redb::Database::open(path).into_diagnostic()?;
		let db = std::sync::Arc::new(db);

		let repl_state = crate::repl::ReplState {
			output_file: None,
			sys_user: String::new(),
			db_user: String::new(),
			expanded_mode: false,
			write_mode: false,
			ots: None,
			use_colours: false,
			vars: std::collections::BTreeMap::new(),
			snippets: crate::snippets::Snippets::new(),
			transaction_state: crate::repl::TransactionState::None,
			result_store: crate::result_store::ResultStore::new(),
		};

		let audit = Self {
			db,
			repl_state: std::sync::Arc::new(std::sync::Mutex::new(repl_state)),
			working_info: None,
			sync_thread: None,
		};

		Ok(audit)
	}
}

/// Export audit entries as JSON to stdout
///
/// This is the main library function that implements the audit export functionality.
/// Both the standalone binary and the bestool subcommand use this function.
pub fn export_audit_entries(options: ExportOptions) -> Result<()> {
	let audit_path = if let Some(path) = options.audit_path {
		path
	} else {
		Audit::default_path()?
	};

	debug!(?audit_path, "using audit path");

	let mut stdout = std::io::stdout().lock();

	if options.orphans {
		// Find and read orphan databases
		let orphans = Audit::find_orphans(&audit_path)?;

		if orphans.is_empty() {
			info!("no orphan databases found");
			return Ok(());
		}

		for orphan_path in orphans {
			info!(
				"reading orphan: {}",
				orphan_path.file_name().unwrap().to_string_lossy()
			);

			let audit = Audit::open_file(&orphan_path)?;
			let entries = audit.query(&options.query_options)?;

			for (timestamp, entry) in entries {
				let entry_with_ts =
					AuditEntryWithTimestamp::from_entry_and_timestamp(entry, timestamp);
				let json = serde_json::to_string(&entry_with_ts).into_diagnostic()?;
				writeln!(stdout, "{}", json).into_diagnostic()?;
			}
		}
	} else {
		// Read main database
		let audit = Audit::open_file(Audit::main_db_path(&audit_path))?;
		let entries = audit.query(&options.query_options)?;

		for (timestamp, entry) in entries {
			let entry_with_ts = AuditEntryWithTimestamp::from_entry_and_timestamp(entry, timestamp);
			let json = serde_json::to_string(&entry_with_ts).into_diagnostic()?;
			writeln!(stdout, "{}", json).into_diagnostic()?;
		}
	}

	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn test_query_limit_from_oldest() {
		let temp_dir = tempfile::tempdir().unwrap();
		let db_path = temp_dir.path().join("test.redb");

		let mut audit = Audit::open_empty(db_path).unwrap();

		// Add 10 entries
		for i in 0..10 {
			audit.add_entry(format!("SELECT {};", i)).unwrap();
			// Sleep a bit to ensure different timestamps
			std::thread::sleep(std::time::Duration::from_micros(10));
		}

		// Query first 3 entries
		let opts = QueryOptions {
			limit: Some(3),
			from_oldest: true,
			..Default::default()
		};
		let entries = audit.query(&opts).unwrap();
		assert_eq!(entries.len(), 3);
		assert_eq!(entries[0].1.query, "SELECT 0;");
		assert_eq!(entries[1].1.query, "SELECT 1;");
		assert_eq!(entries[2].1.query, "SELECT 2;");
	}

	#[test]
	fn test_query_limit_from_newest() {
		let temp_dir = tempfile::tempdir().unwrap();
		let db_path = temp_dir.path().join("test.redb");

		let mut audit = Audit::open_empty(db_path).unwrap();

		// Add 10 entries
		for i in 0..10 {
			audit.add_entry(format!("SELECT {};", i)).unwrap();
			std::thread::sleep(std::time::Duration::from_micros(10));
		}

		// Query last 3 entries (but still returned oldest-first)
		let opts = QueryOptions {
			limit: Some(3),
			from_oldest: false,
			..Default::default()
		};
		let entries = audit.query(&opts).unwrap();
		assert_eq!(entries.len(), 3);
		assert_eq!(entries[0].1.query, "SELECT 7;");
		assert_eq!(entries[1].1.query, "SELECT 8;");
		assert_eq!(entries[2].1.query, "SELECT 9;");
	}
}
