//! Audit psql history command.

use bestool_psql::export::ExportEntry;
use clap::Parser;
use miette::Result;

use crate::actions::Context;

/// Audit and inspect bestool tamanu psql command history.
#[derive(Debug, Clone, Parser)]
pub struct AuditPsqlArgs {
	/// Path to history database (default: ~/.local/state/bestool-psql/history.redb)
	#[arg(long)]
	history_path: Option<std::path::PathBuf>,

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
}

pub async fn run(ctx: Context<AuditPsqlArgs>) -> Result<()> {
	let history_path = if let Some(ref path) = ctx.args_top.history_path {
		path.clone()
	} else {
		bestool_psql::history::History::default_path()?
	};

	let history = bestool_psql::history::History::open(&history_path)?;
	let entries = history.list()?;

	let filter_regex = if let Some(pattern) = &ctx.args_top.filter {
		Some(regex::Regex::new(pattern).map_err(|e| miette::miette!("Invalid regex: {}", e))?)
	} else {
		Some(regex::Regex::new(r"^\\q\s*$").map_err(|e| miette::miette!("Invalid regex: {}", e))?)
	};

	let filtered: Vec<_> = entries
		.into_iter()
		.rev() // Reverse to show most recent first
		.filter(|(_, entry)| {
			if let Some(ref re) = filter_regex {
				if ctx.args_top.filter.is_some() {
					if !re.is_match(&entry.query) {
						return false;
					}
				} else if re.is_match(&entry.query) {
    						return false;
    					}
			}
			if let Some(user) = &ctx.args_top.db_user
				&& &entry.db_user != user
			{
				return false;
			}
			if let Some(user) = &ctx.args_top.sys_user
				&& &entry.sys_user != user
			{
				return false;
			}
			if ctx.args_top.write_only && !entry.writemode {
				return false;
			}
			true
		})
		.take(ctx.args_top.limit)
		.collect();

	if filtered.is_empty() {
		if !ctx.args_top.json {
			println!("No matching history entries found");
		}
		return Ok(());
	}

	if ctx.args_top.json {
		for (timestamp, entry) in filtered {
			let export_entry = ExportEntry::from_history(timestamp, entry);
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

	Ok(())
}
