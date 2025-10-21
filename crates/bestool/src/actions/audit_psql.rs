//! Audit psql history command.

use clap::Parser;
use miette::Result;

use crate::actions::Context;

/// Audit and inspect bestool tamanu psql command history.
#[derive(Debug, Clone, Parser)]
pub struct AuditPsqlArgs {
	/// Path to history database (default: ~/.local/state/bestool-psql/history.redb)
	#[arg(long, global = true)]
	history_path: Option<std::path::PathBuf>,

	#[command(subcommand)]
	command: Command,
}

#[derive(Debug, Clone, clap::Subcommand)]
enum Command {
	/// List recent history entries
	List {
		/// Number of entries to show
		#[arg(short = 'n', long, default_value = "20")]
		limit: usize,

		/// Filter by database user
		#[arg(long)]
		db_user: Option<String>,

		/// Filter by system user
		#[arg(long)]
		sys_user: Option<String>,

		/// Show only write-mode queries
		#[arg(long)]
		write_only: bool,

		/// Output as JSON (one entry per line)
		#[arg(long)]
		json: bool,

		/// Filter queries by regex pattern
		#[arg(long)]
		filter: Option<String>,
	},

	/// Search history entries
	Search {
		/// Search pattern (case-insensitive substring match)
		pattern: String,

		/// Maximum number of results
		#[arg(short = 'n', long, default_value = "50")]
		limit: usize,
	},

	/// Show statistics about the history database
	Stats,

	/// Clear all history entries (requires confirmation)
	Clear {
		/// Skip confirmation prompt
		#[arg(long)]
		yes: bool,
	},

	/// Compact the history database to reclaim space
	Compact,
}

pub async fn run(ctx: Context<AuditPsqlArgs>) -> Result<()> {
	let history_path = if let Some(ref path) = ctx.args_top.history_path {
		path.clone()
	} else {
		bestool_psql::history::History::default_path()?
	};

	match &ctx.args_top.command {
		Command::List {
			limit,
			db_user,
			sys_user,
			write_only,
			json,
			filter,
		} => {
			let history = bestool_psql::history::History::open(&history_path)?;
			let entries = history.list()?;

			let filter_regex = if let Some(pattern) = filter {
				Some(
					regex::Regex::new(pattern)
						.map_err(|e| miette::miette!("Invalid regex: {}", e))?,
				)
			} else {
				Some(
					regex::Regex::new(r"^\\q\s*$")
						.map_err(|e| miette::miette!("Invalid regex: {}", e))?,
				)
			};

			let filtered: Vec<_> = entries
				.into_iter()
				.rev() // Reverse to show most recent first
				.filter(|(_, entry)| {
					if let Some(ref re) = filter_regex {
						if filter.is_some() {
							if !re.is_match(&entry.query) {
								return false;
							}
						} else {
							if re.is_match(&entry.query) {
								return false;
							}
						}
					}
					if let Some(user) = db_user
						&& &entry.db_user != user
					{
						return false;
					}
					if let Some(user) = sys_user
						&& &entry.sys_user != user
					{
						return false;
					}
					if *write_only && !entry.writemode {
						return false;
					}
					true
				})
				.take(*limit)
				.collect();

			if filtered.is_empty() {
				if !json {
					println!("No matching history entries found");
				}
				return Ok(());
			}

			if *json {
				for (timestamp, entry) in filtered {
					let export_entry = ExportEntry {
						ts: timestamp_to_rfc3339(timestamp),
						query: entry.query,
						db_user: entry.db_user,
						sys_user: entry.sys_user,
						writemode: entry.writemode,
						tailscale: entry.tailscale,
						ots: entry.ots,
					};
					let json_str = serde_json::to_string(&export_entry)
						.map_err(|e| miette::miette!("Failed to serialize entry: {}", e))?;
					println!("{}", json_str);
				}
			} else {
				let count = filtered.len();
				for (timestamp, entry) in filtered {
					let ts_str = if timestamp > 0 {
						chrono::DateTime::from_timestamp_micros(timestamp as i64)
							.map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
							.unwrap_or_else(|| format!("timestamp:{}", timestamp))
					} else {
						"imported".to_string()
					};

					let mode = if entry.writemode { "W" } else { "R" };
					let users = if entry.db_user.is_empty() && entry.sys_user.is_empty() {
						String::new()
					} else {
						format!(" [{}@{}]", entry.sys_user, entry.db_user)
					};

					println!(
						"{} [{}]{}: {}",
						ts_str,
						mode,
						users,
						entry.query.lines().next().unwrap_or("")
					);
				}

				println!("\nShowing {} entries", count);
			}
		}

		Command::Search { pattern, limit } => {
			let history = bestool_psql::history::History::open(&history_path)?;
			let entries = history.list()?;

			let pattern_lower = pattern.to_lowercase();
			let matches: Vec<_> = entries
				.into_iter()
				.rev() // Search most recent first
				.filter(|(_, entry)| entry.query.to_lowercase().contains(&pattern_lower))
				.take(*limit)
				.collect();

			if matches.is_empty() {
				println!("No matches found for pattern: {}", pattern);
				return Ok(());
			}

			let count = matches.len();
			for (timestamp, entry) in matches {
				let ts_str = if timestamp > 0 {
					chrono::DateTime::from_timestamp_micros(timestamp as i64)
						.map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
						.unwrap_or_else(|| format!("timestamp:{}", timestamp))
				} else {
					"imported".to_string()
				};

				println!("\n{} [{}@{}]:", ts_str, entry.sys_user, entry.db_user);
				println!("{}", entry.query);
			}

			println!("\n{} matches found", count);
		}

		Command::Stats => {
			let history = bestool_psql::history::History::open(&history_path)?;
			let entries = history.list()?;

			let total = entries.len();
			let write_count = entries.iter().filter(|(_, e)| e.writemode).count();
			let read_count = total - write_count;
			let imported = entries.iter().filter(|(ts, _)| *ts == 0).count();

			let users: std::collections::HashSet<_> =
				entries.iter().map(|(_, e)| &e.db_user).collect();
			let sys_users: std::collections::HashSet<_> =
				entries.iter().map(|(_, e)| &e.sys_user).collect();

			println!("History Database Statistics");
			println!("===========================");
			println!("Path: {}", history_path.display());
			if let Ok(metadata) = std::fs::metadata(&history_path) {
				let size_mb = metadata.len() as f64 / (1024.0 * 1024.0);
				println!("Size: {:.2} MB", size_mb);
			}
			println!("\nTotal entries: {}", total);
			println!("  Read-only:   {}", read_count);
			println!("  Write mode:  {}", write_count);
			println!("  Imported:    {}", imported);
			println!("\nUnique database users: {}", users.len());
			println!("Unique system users:   {}", sys_users.len());
		}

		Command::Clear { yes } => {
			if !yes {
				println!("This will permanently delete all history entries.");
				print!("Are you sure? (y/N): ");
				use std::io::Write;
				std::io::stdout().flush().unwrap();

				let mut input = String::new();
				std::io::stdin().read_line(&mut input).unwrap();

				if !input.trim().eq_ignore_ascii_case("y") {
					println!("Cancelled");
					return Ok(());
				}
			}

			let mut history = bestool_psql::history::History::open(&history_path)?;
			history.clear_all()?;
			println!("All history entries cleared");
		}

		Command::Compact => {
			let mut history = bestool_psql::history::History::open(&history_path)?;

			let size_before = std::fs::metadata(&history_path)
				.map(|m| m.len())
				.unwrap_or(0);

			history.compact()?;

			let size_after = std::fs::metadata(&history_path)
				.map(|m| m.len())
				.unwrap_or(0);

			let saved = size_before.saturating_sub(size_after);
			let saved_mb = saved as f64 / (1024.0 * 1024.0);

			println!("Database compacted");
			println!("Space reclaimed: {:.2} MB", saved_mb);
		}
	}

	Ok(())
}

/// Export entry format with RFC3339 timestamp
#[derive(Debug, serde::Serialize)]
struct ExportEntry {
	ts: String,
	query: String,
	db_user: String,
	sys_user: String,
	writemode: bool,
	#[serde(skip_serializing_if = "Vec::is_empty")]
	tailscale: Vec<bestool_psql::history::TailscalePeer>,
	#[serde(skip_serializing_if = "Option::is_none")]
	ots: Option<String>,
}

fn timestamp_to_rfc3339(micros: u64) -> String {
	use jiff::Timestamp;

	let secs = (micros / 1_000_000) as i64;
	let nanos = ((micros % 1_000_000) * 1_000) as i32;

	Timestamp::new(secs, nanos)
		.map(|ts| ts.to_string())
		.unwrap_or_else(|_| format!("invalid-timestamp-{}", micros))
}
