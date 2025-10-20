pub mod history;

use miette::{miette, IntoDiagnostic, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use rand::Rng;
use rustyline::history::History as HistoryTrait;
use rustyline::{Config, Editor};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tempfile::NamedTempFile;
use thiserror::Error;
use tracing::{debug, trace, warn};

/// Generate a random boundary marker for prompt detection
fn generate_boundary() -> String {
	use std::fmt::Write;

	let mut rng = rand::thread_rng();
	let random_bytes: [u8; 16] = rng.gen();

	let mut result = String::with_capacity(32);
	for byte in random_bytes {
		write!(&mut result, "{:02x}", byte).unwrap();
	}
	result
}

/// Information parsed from a psql prompt
#[derive(Debug, Clone)]
struct PromptInfo {
	database: String,
	#[allow(dead_code)]
	username: String,
	user_type: String,   // "#" for superuser, ">" for regular
	status: String,      // "=" normal, "!" disconnected, "^" single-line
	transaction: String, // "" none, "*" in transaction, "!" failed transaction, "?" unknown
	prompt_type: u8,     // 1 = PROMPT1 (normal), 2 = PROMPT2 (continuation), 3 = PROMPT3 (COPY)
}

impl PromptInfo {
	/// Parse from our custom format: <<<BOUNDARY|||type|||db|||user|||usertype|||status|||transaction>>>
	fn parse(line: &str, boundary: &str) -> Option<Self> {
		let marker_start = format!("<<<{}|||", boundary);
		let marker_end = ">>>";

		let start = line.find(&marker_start)?;
		let end = line.find(marker_end)?;

		if end <= start {
			return None;
		}

		let content = &line[start + marker_start.len()..end];
		let parts: Vec<&str> = content.split("|||").collect();

		if parts.len() != 6 {
			return None;
		}

		let prompt_type = parts[0].parse::<u8>().ok()?;

		Some(PromptInfo {
			database: parts[1].to_string(),
			username: parts[2].to_string(),
			user_type: parts[3].to_string(),
			status: parts[4].to_string(),
			transaction: parts[5].to_string(),
			prompt_type,
		})
	}

	/// Format as a standard psql prompt
	fn format_prompt(&self) -> String {
		match self.prompt_type {
			2 => {
				// PROMPT2: continuation prompt (multi-line queries)
				format!(
					"{}{}{}{} ",
					self.database, self.status, self.transaction, "-"
				)
			}
			3 => {
				// PROMPT3: COPY mode prompt
				">> ".to_string()
			}
			_ => {
				// PROMPT1: normal prompt
				format!(
					"{}{}{}{} ",
					self.database, self.status, self.transaction, self.user_type
				)
			}
		}
	}
}

#[cfg(unix)]
use signal_hook::consts::SIGWINCH;
#[cfg(unix)]
use signal_hook::iterator::Signals;

#[cfg(windows)]
use windows_sys::Win32::Foundation::HANDLE;
#[cfg(windows)]
use windows_sys::Win32::System::Console::{
	GetConsoleScreenBufferInfo, GetStdHandle, CONSOLE_SCREEN_BUFFER_INFO, STD_OUTPUT_HANDLE,
	WINDOW_BUFFER_SIZE_EVENT,
};

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
}

impl PsqlConfig {
	fn command(self, boundary: &str) -> Result<(CommandBuilder, NamedTempFile)> {
		let mut cmd = CommandBuilder::new(&self.psql_path);

		if self.write {
			cmd.arg("--set=AUTOCOMMIT=OFF");
		}

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
			\\set PROMPT1 '<<<{boundary}|||1|||%/|||%n|||%#|||%R|||%x>>>'\n\
			\\set PROMPT2 '<<<{boundary}|||2|||%/|||%n|||%#|||%R|||%x>>>'\n\
			\\set PROMPT3 '<<<{boundary}|||3|||%/|||%n|||%#|||%R|||%x>>>'\n",
			existing = self.psqlrc,
			ro = if self.write {
				""
			} else {
				"SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY;"
			},
		)
		.into_diagnostic()?;
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
	let boundary = generate_boundary();
	debug!(boundary = %boundary, "generated prompt boundary marker");

	let pty_system = NativePtySystem::default();

	let (cols, rows) = terminal_size::terminal_size()
		.map(|(w, h)| (w.0, h.0))
		.unwrap_or((80, 24));

	debug!(cols, rows, "terminal size detected");

	let pty_pair = pty_system
		.openpty(PtySize {
			rows,
			cols,
			pixel_width: 0,
			pixel_height: 0,
		})
		.map_err(|e| miette!("failed to create pty: {}", e))?;

	let pty_master = Arc::new(Mutex::new(pty_pair.master));

	#[cfg(unix)]
	{
		let pty_master_clone = pty_master.clone();
		thread::spawn(move || {
			debug!("starting SIGWINCH handler thread");
			let mut signals = match Signals::new(&[SIGWINCH]) {
				Ok(s) => s,
				Err(e) => {
					warn!("failed to register SIGWINCH handler: {}", e);
					return;
				}
			};

			for _ in signals.forever() {
				if let Some((w, h)) = terminal_size::terminal_size() {
					let new_size = PtySize {
						rows: h.0,
						cols: w.0,
						pixel_width: 0,
						pixel_height: 0,
					};

					debug!(cols = w.0, rows = h.0, "terminal resized (SIGWINCH)");
					if let Ok(master) = pty_master_clone.lock() {
						let _ = master.resize(new_size);
					}
				}
			}
		});
	}

	#[cfg(windows)]
	{
		let pty_master_clone = pty_master.clone();
		thread::spawn(move || unsafe {
			debug!("starting Windows console resize handler thread");
			let stdout_handle: HANDLE = GetStdHandle(STD_OUTPUT_HANDLE);
			if stdout_handle == 0 || stdout_handle == -1i32 as HANDLE {
				warn!("failed to get stdout handle for resize detection");
				return;
			}

			let mut last_size = (cols, rows);

			loop {
				thread::sleep(Duration::from_millis(200));

				let mut csbi: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
				if GetConsoleScreenBufferInfo(stdout_handle, &mut csbi) == 0 {
					continue;
				}

				let new_cols = (csbi.srWindow.Right - csbi.srWindow.Left + 1) as u16;
				let new_rows = (csbi.srWindow.Bottom - csbi.srWindow.Top + 1) as u16;

				if (new_cols, new_rows) != last_size {
					last_size = (new_cols, new_rows);

					let new_size = PtySize {
						rows: new_rows,
						cols: new_cols,
						pixel_width: 0,
						pixel_height: 0,
					};

					debug!(
						cols = new_cols,
						rows = new_rows,
						"terminal resized (Windows)"
					);
					if let Ok(master) = pty_master_clone.lock() {
						let _ = master.resize(new_size);
					}
				}
			}
		});
	}

	let history_path = config.history_path.clone();
	let db_user = config.user.clone();
	let write_mode = config.write;
	let ots = config.ots.clone();
	let boundary_clone = boundary.clone();

	let (cmd, _rc_guard) = config.command(&boundary)?;
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

	let current_prompt = Arc::new(Mutex::new(String::new()));
	let current_prompt_clone = current_prompt.clone();

	// Track the last input sent to filter out echo
	let last_input = Arc::new(Mutex::new(String::new()));
	let last_input_clone = last_input.clone();

	let reader_thread = thread::spawn(move || {
		let mut reader = reader;
		let mut buf = [0u8; 4096];
		let mut skip_next = 0usize;

		loop {
			match reader.read(&mut buf) {
				Ok(0) => break, // EOF
				Ok(n) => {
					// Store in buffer for prompt detection
					let mut buffer = output_buffer_clone.lock().unwrap();
					for &byte in &buf[..n] {
						if buffer.len() >= 1024 {
							buffer.pop_front();
						}
						buffer.push_back(byte);
					}

					let mut data = String::from_utf8_lossy(&buf[..n]).to_string();
					trace!(data, "read some data");

					// Filter out echoed input if we're expecting echo
					if skip_next > 0 {
						let to_skip = skip_next.min(data.len());
						data.drain(..to_skip);
						skip_next -= to_skip;
					} else {
						let expected_echo = last_input_clone.lock().unwrap().clone();
						if !expected_echo.is_empty() {
							// PTY converts \n to \r\n
							let normalized = expected_echo.replace('\n', "\r\n");
							if data.starts_with(&normalized) {
								data.drain(..normalized.len());
								last_input_clone.lock().unwrap().clear();
							} else if normalized.starts_with(&data) {
								// Partial match - skip this chunk and continue
								skip_next = normalized.len() - data.len();
								data.clear();
							}
						}
					}

					if !data.is_empty() {
						// Check if this contains our prompt boundary marker
						if let Some(prompt_info) = PromptInfo::parse(&data, &boundary_clone) {
							// Replace the boundary marker with formatted prompt
							let formatted = prompt_info.format_prompt();
							let marker = format!("<<<{}|||", boundary_clone);
							if let Some(start) = data.find(&marker) {
								if let Some(end) = data[start..].find(">>>") {
									let full_marker_end = start + end + 3;
									data.replace_range(start..full_marker_end, &formatted);
								}
							}

							// Store the formatted prompt
							*current_prompt_clone.lock().unwrap() = formatted;
						}

						print!("{}", data);
						std::io::stdout().flush().ok();
					}
				}
				Err(_) => break,
			}
		}
		*running_clone.lock().unwrap() = false;
	});

	let db_user = db_user.unwrap_or_else(|| {
		std::env::var("USER")
			.or_else(|_| std::env::var("USERNAME"))
			.unwrap_or_else(|_| "unknown".to_string())
	});

	let sys_user = std::env::var("USER")
		.or_else(|_| std::env::var("USERNAME"))
		.unwrap_or_else(|_| "unknown".to_string());

	let mut history = history::History::open(history_path).unwrap_or_else(|e| {
		warn!("could not open history database: {}", e);
		debug!("creating fallback history database");
		// Create a temporary in-memory fallback (this will still fail, but provides a better error)
		history::History::open(
			std::env::temp_dir().join(format!("bestool-psql-fallback-{}.redb", std::process::id())),
		)
		.expect("Failed to create fallback history database")
	});

	history.set_context(db_user.clone(), sys_user.clone(), write_mode, ots);

	debug!(db_user, sys_user, write_mode, "history context set");

	let mut rl: Editor<(), history::History> = Editor::with_history(
		Config::builder()
			.auto_add_history(false)
			.history_ignore_dups(false)
			.unwrap()
			.build(),
		history,
	)
	.into_diagnostic()?;

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

		// Check if we're at a prompt by looking for our boundary marker in the output buffer
		let mut buffer = output_buffer.lock().unwrap();
		let buffer_vec: Vec<u8> = buffer.iter().copied().collect();
		let buffer_str = String::from_utf8_lossy(&buffer_vec);
		let at_prompt = buffer_str.contains(&format!("<<<{boundary}|||"));
		trace!(at_prompt, %buffer_str, "buffer");

		if !at_prompt {
			// Not at a prompt, continue waiting
			thread::sleep(Duration::from_millis(50));
			continue;
		}

		// Clear the buffer since we've detected a prompt
		buffer.clear();
		drop(buffer);

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

				if !line.trim().is_empty() {
					if let Err(e) = rl.history_mut().add(&line) {
						warn!("failed to add history entry: {}", e);
					} else {
						debug!("wrote history entry before sending to psql");
					}
				}

				let input = format!("{}\n", line);

				// Store the input so we can filter out the echo
				*last_input.lock().unwrap() = input.clone();

				let mut writer = writer.lock().unwrap();
				if let Err(_) = writer.write_all(input.as_bytes()) {
					return Err(PsqlError::WriteError).into_diagnostic();
				}
				writer.flush().into_diagnostic()?;
			}
			Err(rustyline::error::ReadlineError::Interrupted) => {
				debug!("received Ctrl-C");
				let mut writer = writer.lock().unwrap();
				writer.write_all(&[3]).ok(); // ASCII ETX (Ctrl-C)
				writer.flush().ok();
			}
			Err(rustyline::error::ReadlineError::Eof) => {
				debug!("received Ctrl-D (EOF)");
				let mut writer = writer.lock().unwrap();
				writer.write_all(&[4]).ok(); // ASCII EOT (Ctrl-D)
				writer.flush().ok();
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
