use bestool_psql::history::History;
use clap::{Parser, Subcommand};
use miette::{IntoDiagnostic, Result};
use std::path::PathBuf;

/// Manage bestool-psql query history
#[derive(Debug, Parser)]
#[command(name = "psql-history")]
#[command(about = "Manage bestool-psql query history")]
struct Args {
	/// Path to history database (default: ~/.cache/bestool-psql/history.redb)
	#[arg(long, global = true)]
	history_path: Option<PathBuf>,

	#[command(subcommand)]
	command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
	/// List all history entries
	List {
		/// Number of entries to show (default: all)
		#[arg(short, long)]
		limit: Option<usize>,

		/// Show in reverse order (newest first)
		#[arg(short, long)]
		reverse: bool,
	},

	/// Show recent history entries
	Recent {
		/// Number of entries to show
		#[arg(short, long, default_value = "10")]
		limit: usize,
	},

	/// Clear all history
	Clear {
		/// Skip confirmation prompt
		#[arg(short = 'y', long)]
		yes: bool,
	},

	/// Show history statistics
	Stats,

	/// Export history as JSON
	Export {
		/// Output file (default: stdout)
		#[arg(short, long)]
		output: Option<PathBuf>,
	},
}

fn main() -> Result<()> {
	let args = Args::parse();

	// Determine history path
	let history_path = if let Some(path) = args.history_path {
		path
	} else {
		History::default_path()?
	};

	if !history_path.exists() {
		eprintln!("History database not found at: {}", history_path.display());
		eprintln!("No history has been recorded yet.");
		return Ok(());
	}

	let history = History::open(history_path)?;

	match args.command {
		Commands::List { limit, reverse } => {
			let mut entries = history.list()?;

			if reverse {
				entries.reverse();
			}

			if let Some(lim) = limit {
				entries.truncate(lim);
			}

			if entries.is_empty() {
				println!("No history entries found.");
				return Ok(());
			}

			for (timestamp, entry) in entries {
				let datetime = timestamp_to_datetime(timestamp);
				let mode = if entry.writemode { "WRITE" } else { "READ" };
				println!(
					"[{}] {} - db={} sys={}",
					datetime, mode, entry.db_user, entry.sys_user
				);
				if let Some(ref tailscale) = entry.tailscale {
					print!("  tailscale=");
					for peer in tailscale {
						print!("{}:{} ", peer.user, peer.device);
					}
					println!();
				}
				println!("  {}", entry.query);
				println!();
			}
		}

		Commands::Recent { limit } => {
			let entries = history.recent(limit)?;

			if entries.is_empty() {
				println!("No history entries found.");
				return Ok(());
			}

			for (timestamp, entry) in entries {
				let datetime = timestamp_to_datetime(timestamp);
				let mode = if entry.writemode { "WRITE" } else { "READ" };
				println!(
					"[{}] {} - db:{} sys:{}",
					datetime, mode, entry.db_user, entry.sys_user
				);
				if let Some(ref tailscale) = entry.tailscale {
					print!("  tailscale:");
					for peer in tailscale {
						print!(" {}@{}", peer.user, peer.device);
					}
					println!();
				}
				println!("  {}", entry.query);
				println!();
			}
		}

		Commands::Clear { yes } => {
			if !yes {
				print!("Are you sure you want to clear all history? [y/N] ");
				use std::io::Write;
				std::io::stdout().flush().into_diagnostic()?;

				let mut input = String::new();
				std::io::stdin().read_line(&mut input).into_diagnostic()?;

				if !input.trim().eq_ignore_ascii_case("y") {
					println!("Cancelled.");
					return Ok(());
				}
			}

			history.clear()?;
			println!("History cleared.");
		}

		Commands::Stats => {
			let entries = history.list()?;

			if entries.is_empty() {
				println!("No history entries found.");
				return Ok(());
			}

			let total = entries.len();
			let write_count = entries.iter().filter(|(_, e)| e.writemode).count();
			let read_count = total - write_count;

			let mut db_users = std::collections::HashSet::new();
			let mut sys_users = std::collections::HashSet::new();
			let mut tailscale_users = std::collections::HashSet::new();
			for (_, entry) in &entries {
				db_users.insert(entry.db_user.clone());
				sys_users.insert(entry.sys_user.clone());
				if let Some(ref tailscale) = entry.tailscale {
					for peer in tailscale {
						tailscale_users.insert(format!("{}@{}", peer.user, peer.device));
					}
				}
			}

			let oldest = timestamp_to_datetime(entries.first().unwrap().0);
			let newest = timestamp_to_datetime(entries.last().unwrap().0);

			println!("History Statistics");
			println!("==================");
			println!("Total entries:    {}", total);
			println!("Read queries:     {}", read_count);
			println!("Write queries:    {}", write_count);
			println!("Unique DB users:  {}", db_users.len());
			println!(
				"DB users:         {}",
				db_users.iter().cloned().collect::<Vec<_>>().join(", ")
			);
			println!("Unique sys users: {}", sys_users.len());
			println!(
				"Sys users:        {}",
				sys_users.iter().cloned().collect::<Vec<_>>().join(", ")
			);
			if !tailscale_users.is_empty() {
				println!("Tailscale peers:  {}", tailscale_users.len());
				println!(
					"Peers:            {}",
					tailscale_users
						.iter()
						.cloned()
						.collect::<Vec<_>>()
						.join(", ")
				);
			}
			println!("Oldest entry:     {}", oldest);
			println!("Newest entry:     {}", newest);
		}

		Commands::Export { output } => {
			let entries = history.list()?;

			let json = serde_json::to_string_pretty(&entries).into_diagnostic()?;

			if let Some(path) = output {
				std::fs::write(path, json).into_diagnostic()?;
				println!("History exported.");
			} else {
				println!("{}", json);
			}
		}
	}

	Ok(())
}

fn timestamp_to_datetime(micros: u64) -> String {
	use std::time::{Duration, UNIX_EPOCH};

	let duration = Duration::from_micros(micros);
	let system_time = UNIX_EPOCH + duration;

	// Format as ISO 8601 datetime
	if let Ok(duration_since_epoch) = system_time.duration_since(UNIX_EPOCH) {
		let secs = duration_since_epoch.as_secs();
		let nanos = duration_since_epoch.subsec_nanos();

		// Basic datetime formatting (simplified)
		let days_since_epoch = secs / 86400;
		let seconds_today = secs % 86400;
		let hours = seconds_today / 3600;
		let minutes = (seconds_today % 3600) / 60;
		let seconds = seconds_today % 60;

		// Calculate date (simplified - not accounting for leap years properly)
		let year = 1970 + (days_since_epoch / 365) as i32;
		let day_of_year = (days_since_epoch % 365) as u32;

		// Very simplified month/day calculation
		let month = 1 + (day_of_year / 30).min(11);
		let day = 1 + (day_of_year % 30).min(30);

		format!(
			"{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
			year,
			month,
			day,
			hours,
			minutes,
			seconds,
			nanos / 1000
		)
	} else {
		format!("{}", micros)
	}
}
