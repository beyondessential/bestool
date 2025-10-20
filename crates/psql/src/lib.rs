mod completer;
pub mod history;
mod ots;
mod prompt;
mod psql_writer;
mod reader;
mod schema_cache;
mod terminal;

// Re-export for convenience
pub use ots::prompt_for_ots;

use completer::SqlCompleter;
use miette::{miette, IntoDiagnostic, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use psql_writer::PsqlWriter;
use rustyline::history::History as _;
use rustyline::{Config, Editor};
use schema_cache::SchemaCacheManager;
use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tempfile::NamedTempFile;
use thiserror::Error;
use tracing::{debug, info, trace, warn};

#[derive(Debug, Error)]
pub enum PsqlError {
	#[error("psql process terminated unexpectedly")]
	ProcessTerminated,
	#[error("failed to read from psql")]
	ReadError,
	#[error("failed to write to psql")]
	WriteError,
}

/// Configuration for the psql wrapper
#[derive(Debug, Clone)]
pub struct PsqlConfig {
	/// Path to the psql executable
	pub psql_path: PathBuf,

	/// Whether to enable write mode
	pub write: bool,

	/// Arguments to pass to psql
	pub args: Vec<String>,

	/// Existing psqlrc contents
	pub psqlrc: String,

	/// Path to the history database
	pub history_path: PathBuf,

	/// Database user for history tracking
	pub user: Option<String>,

	/// OTS (Over The Shoulder) value for write mode sessions
	pub ots: Option<String>,

	/// Whether to launch psql directly without rustyline wrapper (read-only mode only)
	pub passthrough: bool,

	/// Whether to disable schema cache population
	pub disable_schema_cache: bool,
}

impl PsqlConfig {
	fn psqlrc(&self, boundary: Option<&str>) -> Result<NamedTempFile> {
		let prompts = if let Some(boundary) = boundary {
			format!(
				"\\set PROMPT1 '<<<{boundary}|||1|||%/|||%n|||%#|||%R|||%x>>>'\n\
				\\set PROMPT2 '<<<{boundary}|||2|||%/|||%n|||%#|||%R|||%x>>>'\n\
				\\set PROMPT3 '<<<{boundary}|||3|||%/|||%n|||%#|||%R|||%x>>>'\n"
			)
		} else {
			String::new()
		};

		let mut rc = tempfile::Builder::new()
			.prefix("bestool-psql-")
			.suffix(".psqlrc")
			.tempfile()
			.into_diagnostic()?;

		write!(
			rc.as_file_mut(),
			"\\encoding UTF8\n\
			\\timing\n\
			{existing}\n\
			{ro}\n\
			{prompts}",
			existing = self.psqlrc,
			ro = if self.write {
				""
			} else {
				"SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY;"
			},
		)
		.into_diagnostic()?;

		Ok(rc)
	}

	fn pty_command(self, boundary: Option<&str>) -> Result<(CommandBuilder, NamedTempFile)> {
		let mut cmd = CommandBuilder::new(&self.psql_path);

		if self.write {
			cmd.arg("--set=AUTOCOMMIT=OFF");
		}

		let rc = self.psqlrc(boundary)?;
		cmd.env("PSQLRC", rc.path());

		for arg in &self.args {
			cmd.arg(arg);
		}
		for (key, value) in std::env::vars_os() {
			cmd.env(key, value);
		}

		Ok((cmd, rc))
	}

	fn std_command(self, boundary: Option<&str>) -> Result<(Command, NamedTempFile)> {
		let mut cmd = Command::new(&self.psql_path);

		if self.write {
			cmd.arg("--set=AUTOCOMMIT=OFF");
		}

		let rc = self.psqlrc(boundary)?;
		cmd.env("PSQLRC", rc.path());

		for arg in &self.args {
			cmd.arg(arg);
		}
		for (key, value) in std::env::vars_os() {
			cmd.env(key, value);
		}

		Ok((cmd, rc))
	}
}

pub fn run(config: PsqlConfig) -> Result<i32> {
	// Handle passthrough mode (read-only only)
	if config.passthrough {
		if config.write {
			return Err(miette!(
				"passthrough mode is only available in read-only mode"
			));
		}
		info!("launching psql in passthrough mode");
		return run_passthrough(config);
	}

	let boundary = prompt::generate_boundary();
	debug!(boundary = %boundary, "generated prompt boundary marker");

	let pty_system = NativePtySystem::default();

	let (cols, rows) = terminal::get_terminal_size();

	let pty_pair = pty_system
		.openpty(PtySize {
			rows,
			cols,
			pixel_width: 0,
			pixel_height: 0,
		})
		.map_err(|e| miette!("failed to create pty: {}", e))?;

	let pty_master = Arc::new(Mutex::new(pty_pair.master));

	terminal::spawn_resize_handler(pty_master.clone());

	let history_path = config.history_path.clone();
	let db_user = config.user.clone();
	let boundary_clone = boundary.clone();

	// Track write mode and OTS as mutable shared state for \W command
	let write_mode = Arc::new(Mutex::new(config.write));
	let ots = Arc::new(Mutex::new(config.ots.clone()));
	let write_mode_clone = write_mode.clone();
	let ots_clone = ots.clone();

	let disable_schema_cache = config.disable_schema_cache;

	let (cmd, _rc_guard) = config.pty_command(Some(&boundary))?;
	let mut child = pty_pair
		.slave
		.spawn_command(cmd)
		.map_err(|e| miette!("failed to spawn psql: {}", e))?;

	drop(pty_pair.slave);

	let reader = {
		let master = pty_master.lock().unwrap();
		master
			.try_clone_reader()
			.map_err(|e| miette!("failed to clone pty reader: {}", e))?
	};

	let writer = Arc::new(Mutex::new({
		let master = pty_master.lock().unwrap();
		master
			.take_writer()
			.map_err(|e| miette!("failed to get pty writer: {}", e))?
	}));

	// Flag to signal termination
	let running = Arc::new(Mutex::new(true));
	let running_clone = running.clone();

	// Buffer to accumulate output and track current prompt (ring buffer with max 1024 bytes)
	let output_buffer = Arc::new(Mutex::new(VecDeque::with_capacity(1024)));
	let output_buffer_clone = output_buffer.clone();

	let psql_writer = PsqlWriter::new(writer.clone(), output_buffer.clone());

	let current_prompt = Arc::new(Mutex::new(String::new()));
	let current_prompt_clone = current_prompt.clone();

	// Track the parsed prompt info for transaction state checking
	let current_prompt_info = Arc::new(Mutex::new(None));
	let current_prompt_info_clone = current_prompt_info.clone();

	// Track the last input sent to filter out echo
	let last_input = Arc::new(Mutex::new(String::new()));

	// Control whether output is printed to stdout
	let print_enabled = Arc::new(Mutex::new(true));
	let print_enabled_clone = print_enabled.clone();

	let reader_thread = reader::spawn_reader_thread(
		reader,
		boundary_clone,
		output_buffer_clone,
		current_prompt_clone,
		current_prompt_info_clone,
		last_input.clone(),
		running_clone,
		print_enabled_clone,
	);

	let history = history::History::setup(
		history_path.clone(),
		db_user,
		*write_mode.lock().unwrap(),
		ots.lock().unwrap().clone(),
	);

	let schema_cache_manager = if !disable_schema_cache {
		debug!("initializing schema cache");
		let manager =
			SchemaCacheManager::new(writer.clone(), print_enabled.clone(), write_mode.clone());

		if let Err(e) = manager.refresh() {
			warn!("failed to populate schema cache: {}", e);
		}

		Some(manager)
	} else {
		debug!("schema cache disabled by config");
		None
	};

	let mut completer = SqlCompleter::with_pty(writer.clone(), output_buffer.clone());
	if let Some(ref cache_manager) = schema_cache_manager {
		completer = completer.with_schema_cache(cache_manager.cache_arc());
	}

	let mut rl: Editor<SqlCompleter, history::History> = Editor::with_history(
		Config::builder()
			.auto_add_history(false)
			.history_ignore_dups(false)
			.unwrap()
			.build(),
		history,
	)
	.into_diagnostic()?;

	rl.set_helper(Some(completer));

	let mut last_reload = std::time::Instant::now();

	debug!("entering main event loop");

	loop {
		if last_reload.elapsed() >= Duration::from_secs(60) {
			debug!("reloading history timestamps");
			if let Err(e) = rl.history_mut().reload_timestamps() {
				warn!("failed to reload history timestamps: {}", e);
			}
			last_reload = std::time::Instant::now();
		}
		match child.try_wait().into_diagnostic()? {
			Some(status) => {
				// Process has exited
				debug!(exit_code = status.exit_code(), "psql process exited");
				reader_thread.join().ok();
				return Ok(status.exit_code() as i32);
			}
			None => {
				// Process still running
			}
		}

		// Check if reader thread is still running
		if !*running.lock().unwrap() {
			// Reader has stopped, process might have exited
			thread::sleep(Duration::from_millis(50));
			if let Some(status) = child.try_wait().into_diagnostic()? {
				return Ok(status.exit_code() as i32);
			}
		}

		// Small delay to let output accumulate
		thread::sleep(Duration::from_millis(50));

		let at_prompt = psql_writer.buffer_contains(&format!("<<<{boundary}|||"));
		if !at_prompt {
			// Not at a prompt, continue waiting
			thread::sleep(Duration::from_millis(50));
			continue;
		}

		// Use the formatted prompt for readline
		let prompt_text = current_prompt.lock().unwrap().clone();
		let readline_prompt = if prompt_text.is_empty() {
			"psql> ".to_string()
		} else {
			prompt_text
		};

		match rl.readline(&readline_prompt) {
			Ok(line) => {
				trace!("received input line");
				let trimmed = line.trim();
				if trimmed == "\\e" || trimmed.starts_with("\\e ") {
					warn!("editor command intercepted (not yet implemented)");
					// TODO: Open editor, read content, save history, send to psql
					continue;
				}

				if trimmed == "\\refresh" {
					let prompt_info = current_prompt_info.lock().unwrap().clone();
					if let Some(ref info) = prompt_info {
						if info.in_transaction() {
							eprintln!("Cannot refresh schema cache while in a transaction. Please COMMIT or ROLLBACK first.");
							continue;
						}
					}

					if let Some(ref cache_manager) = schema_cache_manager {
						info!("refreshing schema cache...");
						eprintln!("Refreshing schema cache...");
						match cache_manager.refresh() {
							Ok(()) => {
								eprintln!("Schema cache refreshed successfully");
							}
							Err(e) => {
								warn!("failed to refresh schema cache: {}", e);
								eprintln!("Failed to refresh schema cache: {}", e);
							}
						}
					} else {
						eprintln!("Schema cache is not enabled");
					}
					continue;
				}

				if trimmed == "\\W" {
					let prompt_info = current_prompt_info.lock().unwrap().clone();
					if let Some(ref info) = prompt_info {
						if info.in_transaction() && info.transaction == "*" {
							warn!("Pending transaction! Please COMMIT or ROLLBACK first");
							continue;
						}
					}

					let mut current_write_mode = write_mode_clone.lock().unwrap();
					let mut current_ots = ots_clone.lock().unwrap();

					if *current_write_mode {
						*current_write_mode = false;
						*current_ots = None;

						let cmd = "SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY;\n\\set AUTOCOMMIT on\nROLLBACK;\n";
						if let Err(e) = psql_writer.write_str(cmd) {
							warn!("failed to write to psql: {}", e);
							continue;
						}

						thread::sleep(Duration::from_millis(50));
						info!("Write mode disabled");
						thread::sleep(Duration::from_millis(5));
						eprintln!("SESSION IS NOW READ ONLY");

						let db_user = rl.history().db_user.clone();
						let sys_user = rl.history().sys_user.clone();
						rl.history_mut().set_context(db_user, sys_user, false, None);
					} else {
						drop(current_write_mode);
						drop(current_ots);

						let db_handle = rl.history().clone_db();
						match ots::prompt_for_ots_with_db(Some(db_handle), Some(&history_path)) {
							Ok(new_ots) => {
								let mut current_write_mode = write_mode_clone.lock().unwrap();
								let mut current_ots = ots_clone.lock().unwrap();

								*current_write_mode = true;
								*current_ots = Some(new_ots.clone());

								let cmd = "SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE;\n\\set AUTOCOMMIT off\nROLLBACK;\n";
								if let Err(e) = psql_writer.write_str(cmd) {
									warn!("failed to write to psql: {}", e);
									continue;
								}

								thread::sleep(Duration::from_millis(50));
								info!("Write mode enabled");
								thread::sleep(Duration::from_millis(5));
								eprintln!("AUTOCOMMIT IS OFF -- REMEMBER TO `COMMIT;` YOUR WRITES");

								let db_user = rl.history().db_user.clone();
								let sys_user = rl.history().sys_user.clone();
								rl.history_mut().set_context(
									db_user,
									sys_user,
									true,
									Some(new_ots),
								);
							}
							Err(e) => {
								eprintln!("Failed to enable write mode: {}", e);
							}
						}
					}
					continue;
				}

				if !line.trim().is_empty() {
					if let Err(e) = rl.history_mut().add(&line) {
						warn!("failed to add history entry: {}", e);
					} else {
						debug!("wrote history entry before sending to psql");
					}
				}

				// Store the input so we can filter out the echo
				*last_input.lock().unwrap() = format!("{}\n", line);

				if let Err(e) = psql_writer.write_line(&line) {
					warn!("failed to write to psql: {}", e);
					return Err(PsqlError::WriteError).into_diagnostic();
				}
			}
			Err(rustyline::error::ReadlineError::Interrupted) => {
				debug!("received Ctrl-C");
				psql_writer.send_control(3).ok(); // ASCII ETX (Ctrl-C)
			}
			Err(rustyline::error::ReadlineError::Eof) => {
				debug!("received Ctrl-D (EOF)");
				psql_writer.send_control(4).ok(); // ASCII EOT (Ctrl-D)
				break;
			}
			Err(err) => {
				return Err(err).into_diagnostic();
			}
		}
	}

	reader_thread.join().ok();

	let status = child.wait().into_diagnostic()?;

	debug!("compacting history database");
	if let Err(e) = rl.history_mut().compact() {
		warn!("failed to compact history database: {}", e);
	}

	debug!(exit_code = status.exit_code(), "exiting");
	Ok(status.exit_code() as i32)
}

/// Run psql in passthrough mode (no rustyline wrapper)
///
/// Read-only mode is enforced.
fn run_passthrough(mut config: PsqlConfig) -> Result<i32> {
	// explicitly cannot do writes without the protections of the wrapper
	config.write = false;

	let (mut cmd, _guard) = config.std_command(None)?;
	let status = cmd.status().into_diagnostic()?;

	Ok(status.code().unwrap_or(1))
}
