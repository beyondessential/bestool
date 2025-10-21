use std::{
	collections::VecDeque,
	io::Write,
	path::PathBuf,
	process::Command,
	sync::{Arc, Mutex},
	thread,
	time::Duration,
};

use completer::SqlCompleter;
use miette::{miette, IntoDiagnostic, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use psql_writer::PsqlWriter;
use rustyline::{
	history::{History as _, SearchDirection},
	Config, Editor,
};
use schema_cache::SchemaCacheManager;
use tempfile::NamedTempFile;
use thiserror::Error;
use tracing::{debug, info, trace, warn};

pub use find::find_postgres_bin;
pub use ots::prompt_for_ots;

mod completer;
mod find;
pub mod highlighter;
pub mod history;
mod ots;
mod prompt;
mod psql_writer;
mod reader;
mod schema_cache;
mod terminal;

/// Set the console codepage on Windows
///
/// This is useful for ensuring proper UTF-8 display in Windows console.
/// On non-Windows platforms, this is a no-op.
#[cfg(windows)]
pub fn set_console_codepage(codepage: u32) {
	unsafe {
		use windows_sys::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};
		SetConsoleCP(codepage);
		SetConsoleOutputCP(codepage);
	}
}

/// Set the console codepage on Windows (no-op on other platforms)
#[cfg(not(windows))]
pub fn set_console_codepage(_codepage: u32) {
	// No-op on non-Windows platforms
}

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
	/// Program executable (typically psql)
	pub program: String,

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

	/// Syntax highlighting theme
	pub theme: highlighter::Theme,
}

impl PsqlConfig {
	fn psqlrc(&self, boundary: Option<&str>, disable_pager: bool) -> Result<NamedTempFile> {
		let prompts = if let Some(boundary) = boundary {
			format!(
				"\\set PROMPT1 '<<<{boundary}|||1|||%/|||%n|||%#|||%R|||%x>>>'\n\
				\\set PROMPT2 '<<<{boundary}|||2|||%/|||%n|||%#|||%R|||%x>>>'\n\
				\\set PROMPT3 '<<<{boundary}|||3|||%/|||%n|||%#|||%R|||%x>>>'\n"
			)
		} else {
			String::new()
		};

		let pager_setting = if disable_pager {
			"\\pset pager off\n"
		} else {
			""
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
			{pager_setting}\
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
		let mut cmd = CommandBuilder::new(crate::find_postgres_bin(&self.program)?);

		if self.write {
			cmd.arg("--set=AUTOCOMMIT=OFF");
		}

		// Disable pager on Windows as it doesn't work properly with PTY
		let rc = self.psqlrc(boundary, cfg!(windows))?;
		cmd.env("PSQLRC", rc.path());

		if cfg!(windows) {
			cmd.env("PAGER", "cat");
		}
		// On Unix, allow pager - we'll handle stdin forwarding when not at prompt

		for arg in &self.args {
			cmd.arg(arg);
		}

		Ok((cmd, rc))
	}

	fn std_command(
		self,
		boundary: Option<&str>,
		disable_pager: bool,
	) -> Result<(Command, NamedTempFile)> {
		let mut cmd = Command::new(crate::find_postgres_bin(&self.program)?);

		if self.write {
			cmd.arg("--set=AUTOCOMMIT=OFF");
		}

		let rc = self.psqlrc(boundary, disable_pager)?;
		cmd.env("PSQLRC", rc.path());
		if disable_pager {
			cmd.env("PAGER", "cat");
		}

		for arg in &self.args {
			cmd.arg(arg);
		}

		Ok((cmd, rc))
	}
}

/// Set terminal to raw mode for pager interaction
#[cfg(unix)]
struct RawMode {
	term_fd: i32,
	original_termios: libc::termios,
	stdin_fd: i32,
	original_flags: i32,
}

#[cfg(unix)]
impl RawMode {
	fn enable() -> Option<Self> {
		use std::os::unix::io::AsRawFd;

		let stdin_fd = std::io::stdin().as_raw_fd();

		// Get the controlling terminal
		let tty_fd = unsafe { libc::open(c"/dev/tty".as_ptr(), libc::O_RDWR) };
		let term_fd = if tty_fd >= 0 {
			tty_fd
		} else {
			libc::STDOUT_FILENO
		};

		// Save original terminal settings
		let mut original_termios: libc::termios = unsafe { std::mem::zeroed() };
		if unsafe { libc::tcgetattr(term_fd, &mut original_termios) } != 0 {
			if tty_fd >= 0 {
				unsafe { libc::close(tty_fd) };
			}
			return None;
		}

		// Save original stdin flags
		let original_flags = unsafe { libc::fcntl(stdin_fd, libc::F_GETFL) };
		if original_flags < 0 {
			if tty_fd >= 0 {
				unsafe { libc::close(tty_fd) };
			}
			return None;
		}

		// Set raw mode for immediate character input without echo
		let mut raw_termios = original_termios;
		unsafe {
			libc::cfmakeraw(&mut raw_termios);
			// Explicitly disable echo to prevent doubled input
			raw_termios.c_lflag &= !libc::ECHO;
			raw_termios.c_lflag &= !libc::ECHONL;
			libc::tcsetattr(term_fd, libc::TCSANOW, &raw_termios);

			// Set stdin non-blocking mode
			libc::fcntl(stdin_fd, libc::F_SETFL, original_flags | libc::O_NONBLOCK);
		}

		Some(RawMode {
			term_fd,
			original_termios,
			stdin_fd,
			original_flags,
		})
	}
}

#[cfg(unix)]
impl Drop for RawMode {
	fn drop(&mut self) {
		// Restore original terminal settings
		unsafe {
			libc::tcsetattr(self.term_fd, libc::TCSANOW, &self.original_termios);
			libc::fcntl(self.stdin_fd, libc::F_SETFL, self.original_flags);
			if self.term_fd != libc::STDOUT_FILENO {
				libc::close(self.term_fd);
			}
		}
	}
}

/// Forward stdin to PTY in raw mode for pager interaction
#[cfg(unix)]
fn forward_stdin_to_pty(psql_writer: &PsqlWriter) {
	use std::io::Read;

	let stdin_handle = std::io::stdin();
	let mut stdin_lock = stdin_handle.lock();

	// Read and forward input
	let mut buf = [0u8; 1024];
	match stdin_lock.read(&mut buf) {
		Ok(n) if n > 0 => {
			if std::env::var("DEBUG_PTY").is_ok() {
				use std::io::Write;
				let data = String::from_utf8_lossy(&buf[..n]);
				eprintln!("\x1b[33m[FWD]\x1b[0m forwarding {} bytes: {:?}", n, data);
				std::io::stderr().flush().ok();
			}
			if let Err(e) = psql_writer.write_bytes(&buf[..n]) {
				warn!("failed to forward stdin to pty: {}", e);
			}
		}
		_ => {}
	}
}

#[cfg(windows)]
fn forward_stdin_to_pty(psql_writer: &PsqlWriter) {
	use windows_sys::Win32::System::Console::{
		GetStdHandle, PeekConsoleInputW, ReadConsoleInputW, INPUT_RECORD, STD_INPUT_HANDLE,
	};

	unsafe {
		let stdin_handle = GetStdHandle(STD_INPUT_HANDLE);
		if !stdin_handle.is_null() && stdin_handle as i32 != -1 {
			let mut num_events: u32 = 0;
			let mut buffer: [INPUT_RECORD; 1] = std::mem::zeroed();

			// Peek to see if there are any console input events available
			if PeekConsoleInputW(stdin_handle, buffer.as_mut_ptr(), 1, &mut num_events) != 0
				&& num_events > 0
			{
				// Read the input events
				let mut num_read: u32 = 0;
				if ReadConsoleInputW(stdin_handle, buffer.as_mut_ptr(), 1, &mut num_read) != 0
					&& num_read > 0
				{
					// Convert INPUT_RECORD to bytes if it's a key event
					let record = &buffer[0];
					// EventType == 1 means KEY_EVENT
					if record.EventType == 1 {
						let key_event = record.Event.KeyEvent;
						// Only process key down events
						if key_event.bKeyDown != 0 {
							let ch = key_event.uChar.UnicodeChar;
							if ch != 0 {
								// Convert UTF-16 char to bytes
								let mut utf8_buf = [0u8; 4];
								if let Some(c) = char::from_u32(ch as u32) {
									let utf8_str = c.encode_utf8(&mut utf8_buf);
									if std::env::var("DEBUG_PTY").is_ok() {
										use std::io::Write;
										eprint!(
											"\x1b[33m[FWD]\x1b[0m forwarding char: {:?}\n",
											utf8_str
										);
										std::io::stderr().flush().ok();
									}
									if let Err(e) = psql_writer.write_bytes(utf8_str.as_bytes()) {
										warn!("failed to forward stdin to pty: {}", e);
									}
								}
							}
						}
					}
				}
			}
		}
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

	// Warn if running in cmd.exe on Windows (output is broken there)
	#[cfg(windows)]
	if std::env::var("PSModulePath").is_err() {
		use tracing::warn;
		warn!(
			"Running in cmd.exe detected. Output may be broken. Consider using PowerShell instead."
		);
	}

	// Extract theme before config is moved
	let theme = config.theme;

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

	let reader_thread = reader::spawn_reader_thread(reader::ReaderThreadParams {
		reader,
		boundary: boundary_clone,
		output_buffer: output_buffer_clone,
		current_prompt: current_prompt_clone,
		current_prompt_info: current_prompt_info_clone,
		last_input: last_input.clone(),
		running: running_clone,
		print_enabled: print_enabled_clone,
		writer: writer.clone(),
	});

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

	let mut completer =
		SqlCompleter::with_pty_and_theme(writer.clone(), output_buffer.clone(), theme);
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

	#[cfg(unix)]
	let mut raw_mode: Option<RawMode> = None;

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
			// Not at a prompt - could be in a pager or query is running
			// Enable raw mode once and keep it active until we return to prompt
			#[cfg(unix)]
			if raw_mode.is_none() {
				raw_mode = RawMode::enable();
			}

			// Forward stdin to PTY for pager interaction
			forward_stdin_to_pty(&psql_writer);
			thread::sleep(Duration::from_millis(50));
			continue;
		}

		// We're at a prompt - disable raw mode if it was enabled
		#[cfg(unix)]
		if raw_mode.is_some() {
			raw_mode = None; // Drop will restore terminal
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
					debug!("editor command intercepted");

					// Get the initial content - either from argument or from history
					let initial_content = if trimmed == "\\e" {
						// Get the last command from history
						let hist_len = rl.history().len();
						if hist_len > 0 {
							match rl.history().get(hist_len - 1, SearchDirection::Forward) {
								Ok(Some(result)) => result.entry.to_string(),
								_ => String::new(),
							}
						} else {
							String::new()
						}
					} else {
						// User provided content after \e
						trimmed
							.strip_prefix("\\e ")
							.unwrap_or("")
							.trim()
							.to_string()
					};

					// Open editor with the content
					match edit::edit(&initial_content) {
						Ok(edited_content) => {
							let edited_trimmed = edited_content.trim();

							// Only send if content is not empty
							if !edited_trimmed.is_empty() {
								info!("sending edited content to psql");

								// Add to history
								if let Err(e) = rl.history_mut().add(&edited_content) {
									warn!("failed to add history entry: {}", e);
								} else {
									debug!("wrote history entry before sending to psql");
								}

								// Store the input so we can filter out the echo
								*last_input.lock().unwrap() = format!("{}\n", edited_content);

								// Send to psql
								if let Err(e) = psql_writer.write_line(&edited_content) {
									warn!("failed to write to psql: {}", e);
									return Err(PsqlError::WriteError).into_diagnostic();
								}
							} else {
								debug!("editor returned empty content, skipping");
							}
						}
						Err(e) => {
							warn!("editor failed: {}", e);
							eprintln!("Editor failed: {}", e);
						}
					}
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

						#[cfg(windows)]
						let cmd = "SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY;\r\n\\set AUTOCOMMIT on\r\nROLLBACK;\r\n";
						#[cfg(not(windows))]
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

								#[cfg(windows)]
								let cmd = "SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE;\r\n\\set AUTOCOMMIT off\r\nROLLBACK;\r\n";
								#[cfg(not(windows))]
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
				#[cfg(windows)]
				{
					// On Windows, send \q command instead of Ctrl-D as it's more reliable
					psql_writer.write_line("\\q").ok();
				}
				#[cfg(not(windows))]
				{
					psql_writer.send_control(4).ok(); // ASCII EOT (Ctrl-D)
				}
				break;
			}
			Err(err) => {
				return Err(err).into_diagnostic();
			}
		}
	}

	reader_thread.join().ok();

	// On Windows, give the process a chance to exit gracefully, but force kill if needed
	#[cfg(windows)]
	let status = {
		use std::time::Duration;

		// Wait up to 2 seconds for graceful exit
		let mut attempts = 0;
		loop {
			if let Some(status) = child.try_wait().into_diagnostic()? {
				break status;
			}
			if attempts >= 20 {
				// After 2 seconds, force kill with Ctrl-C
				debug!("process didn't exit gracefully, sending Ctrl-C");
				psql_writer.send_control(3).ok();
				thread::sleep(Duration::from_millis(500));
				if let Some(status) = child.try_wait().into_diagnostic()? {
					break status;
				}
				// If still not dead, wait indefinitely
				break child.wait().into_diagnostic()?;
			}
			thread::sleep(Duration::from_millis(100));
			attempts += 1;
		}
	};

	#[cfg(not(windows))]
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

	let (mut cmd, _guard) = config.std_command(None, false)?;
	let status = cmd.status().into_diagnostic()?;

	Ok(status.code().unwrap_or(1))
}
