use bestool_psql::export::{timestamp_to_rfc3339, ExportEntry};
use bestool_psql::history::History;
use clap::{Parser, Subcommand};
use lloggs::{LoggingArgs, PreArgs, WorkerGuard};
use miette::{miette, IntoDiagnostic, Result};
use std::path::PathBuf;
use tracing::debug;

/// Manage bestool-psql query history
#[derive(Debug, Parser)]
#[command(name = "psql-history")]
#[command(about = "Manage bestool-psql query history")]
#[command(
	after_help = "Want more detail? Try the long '--help' flag!",
	after_long_help = "Didn't expect this much output? Use the short '-h' flag to get short help."
)]
struct Args {
	#[command(flatten)]
	logging: LoggingArgs,

	/// Path to history database (default: ~/.local/state/bestool-psql/history.redb)
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

		/// Output as JSON (one entry per line)
		#[arg(long)]
		json: bool,

		/// Filter queries by regex pattern
		#[arg(long)]
		filter: Option<String>,
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
}

fn get_args() -> Result<(Args, WorkerGuard)> {
	let log_guard = PreArgs::parse().setup().map_err(|err| miette!("{err}"))?;

	debug!("parsing arguments");
	let args = Args::parse();

	let log_guard = match log_guard {
		Some(g) => g,
		None => args
			.logging
			.setup(|v| match v {
				0 => "info",
				1 => "info,bestool_psql=debug",
				2 => "debug",
				3 => "debug,bestool_psql=trace",
				_ => "trace",
			})
			.map_err(|err| miette!("{err}"))?,
	};

	debug!(?args, "got arguments");
	Ok((args, log_guard))
}

fn main() -> Result<()> {
	let (args, _guard) = get_args()?;

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

	let mut history = History::open(history_path)?;

	match args.command {
		Commands::List {
			limit,
			reverse,
			json,
			filter,
		} => {
			let mut entries = history.list()?;

			let (filter_regex, is_inclusive) = if let Some(pattern) = filter {
				(regex::Regex::new(&pattern).into_diagnostic()?, true)
			} else {
				(regex::Regex::new(r"^\\q\s*$").into_diagnostic()?, false)
			};

			entries.retain(|(_, entry)| {
				if is_inclusive {
					filter_regex.is_match(&entry.query)
				} else {
					!filter_regex.is_match(&entry.query)
				}
			});

			if reverse {
				entries.reverse();
			}

			if let Some(lim) = limit {
				entries.truncate(lim);
			}

			if entries.is_empty() {
				if !json {
					println!("No history entries found.");
				}
				return Ok(());
			}

			if json {
				for (timestamp, entry) in entries {
					let export_entry = ExportEntry::from_history(timestamp, entry);
					let json_str = serde_json::to_string(&export_entry).into_diagnostic()?;
					println!("{}", json_str);
				}
			} else {
				for (timestamp, entry) in entries {
					let datetime = timestamp_to_rfc3339(timestamp);
					let mode = if entry.writemode { "WRITE" } else { "READ" };
					println!(
						"[{}] {} - db={} sys={}",
						datetime, mode, entry.db_user, entry.sys_user
					);
					if !entry.tailscale.is_empty() {
						print!("  tailscale=");
						for peer in &entry.tailscale {
							print!("{}:{} ", peer.user, peer.device);
						}
						println!();
					}
					println!("  {}", entry.query);
					println!();
				}
			}
		}

		Commands::Recent { limit } => {
			let entries = history.recent(limit)?;

			if entries.is_empty() {
				println!("No history entries found.");
				return Ok(());
			}

			for (timestamp, entry) in entries {
				let datetime = timestamp_to_rfc3339(timestamp);
				let mode = if entry.writemode { "WRITE" } else { "READ" };
				println!(
					"[{}] {} - db:{} sys:{}",
					datetime, mode, entry.db_user, entry.sys_user
				);
				if !entry.tailscale.is_empty() {
					print!("  tailscale:");
					for peer in &entry.tailscale {
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

			history.clear_all()?;
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
				if !entry.tailscale.is_empty() {
					for peer in &entry.tailscale {
						tailscale_users.insert(format!("{}@{}", peer.user, peer.device));
					}
				}
			}

			let oldest = timestamp_to_rfc3339(entries.first().unwrap().0);
			let newest = timestamp_to_rfc3339(entries.last().unwrap().0);

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
	}

	Ok(())
}
