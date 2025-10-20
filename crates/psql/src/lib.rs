pub mod history;

use miette::{miette, IntoDiagnostic, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use rustyline::{Config, Editor};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tempfile::NamedTempFile;
use thiserror::Error;

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
}

impl PsqlConfig {
	fn command(self) -> Result<(CommandBuilder, NamedTempFile)> {
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
			"\\encoding UTF8\n\\timing\n{existing}\n{ro}",
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
	let pty_system = NativePtySystem::default();

	// Get terminal size or use sensible defaults
	let (cols, rows) = terminal_size::terminal_size()
		.map(|(w, h)| (w.0, h.0))
		.unwrap_or((80, 24));

	let pty_pair = pty_system
		.openpty(PtySize {
			rows,
			cols,
			pixel_width: 0,
			pixel_height: 0,
		})
		.map_err(|e| miette!("failed to create pty: {}", e))?;

	// Set up resize handler (Arc<Mutex> on all platforms for resize support)
	let pty_master = Arc::new(Mutex::new(pty_pair.master));

	#[cfg(unix)]
	{
		let pty_master_clone = pty_master.clone();
		thread::spawn(move || {
			let mut signals = match Signals::new(&[SIGWINCH]) {
				Ok(s) => s,
				Err(_) => return,
			};

			for _ in signals.forever() {
				// Get new terminal size
				if let Some((w, h)) = terminal_size::terminal_size() {
					let new_size = PtySize {
						rows: h.0,
						cols: w.0,
						pixel_width: 0,
						pixel_height: 0,
					};

					// Update PTY size
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
			let stdout_handle: HANDLE = GetStdHandle(STD_OUTPUT_HANDLE);
			if stdout_handle == 0 || stdout_handle == -1i32 as HANDLE {
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

					if let Ok(master) = pty_master_clone.lock() {
						let _ = master.resize(new_size);
					}
				}
			}
		});
	}

	// Extract values before config is consumed
	let history_path = config.history_path.clone();
	let db_user = config.user.clone();
	let write_mode = config.write;

	let (cmd, _rc_guard) = config.command()?;
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

	// Buffer to accumulate output
	let output_buffer = Arc::new(Mutex::new(Vec::new()));
	let output_buffer_clone = output_buffer.clone();

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
					let mut data = String::from_utf8_lossy(&buf[..n]).to_string();

					// Filter out echoed input if we're expecting echo
					if skip_next > 0 {
						let to_skip = skip_next.min(data.len());
						data.drain(..to_skip);
						skip_next -= to_skip;
					} else {
						// Check if this contains the echo we're waiting for
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

					// Print remaining data to stdout
					if !data.is_empty() {
						print!("{}", data);
						std::io::stdout().flush().ok();

						// Also store in buffer for prompt detection
						let mut buffer = output_buffer_clone.lock().unwrap();
						buffer.extend_from_slice(data.as_bytes());
						// Keep only last 1024 bytes for prompt detection
						if buffer.len() > 1024 {
							let drain_len = buffer.len() - 1024;
							buffer.drain(0..drain_len);
						}
					}
				}
				Err(_) => break,
			}
		}
		*running_clone.lock().unwrap() = false;
	});

	// Set up history with database backend
	let db_user = db_user.unwrap_or_else(|| {
		std::env::var("USER")
			.or_else(|_| std::env::var("USERNAME"))
			.unwrap_or_else(|_| "unknown".to_string())
	});

	let sys_user = std::env::var("USER")
		.or_else(|_| std::env::var("USERNAME"))
		.unwrap_or_else(|_| "unknown".to_string());

	// Open or create history database
	let mut history = history::History::open(history_path).unwrap_or_else(|e| {
		eprintln!("Warning: Could not open history database: {}", e);
		eprintln!("Creating new history database...");
		// Create a temporary in-memory fallback (this will still fail, but provides a better error)
		history::History::open(
			std::env::temp_dir().join(format!("bestool-psql-fallback-{}.redb", std::process::id())),
		)
		.expect("Failed to create fallback history database")
	});

	history.set_context(db_user.clone(), sys_user.clone(), write_mode);

	let mut rl: Editor<(), history::History> = Editor::with_history(
		Config::builder()
			.auto_add_history(true)
			.history_ignore_dups(false)
			.unwrap()
			.build(),
		history,
	)
	.into_diagnostic()?;

	loop {
		// Check if child process is still running
		match child.try_wait().into_diagnostic()? {
			Some(status) => {
				// Process has exited
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

		// Check if we're at a prompt by looking at the output buffer
		let buffer = output_buffer.lock().unwrap().clone();
		let last_line = strip_ansi_escapes::strip(
			buffer
				.split(|b| *b == b'\r' || *b == b'\n')
				.last()
				.unwrap_or_default(),
		);
		let text = String::from_utf8_lossy(&last_line);
		let at_prompt = text.ends_with("=> ")
			|| text.ends_with("-> ")
			|| text.ends_with("=# ")
			|| text.ends_with("-# ")
			|| text.ends_with("(# ");

		if !at_prompt {
			// Not at a prompt, continue waiting
			thread::sleep(Duration::from_millis(50));
			continue;
		}

		// Clear the buffer since we've detected a prompt
		output_buffer.lock().unwrap().clear();

		// Read a line from the user
		match rl.readline(&text) {
			Ok(line) => {
				// Check if user wants to use a graphical editor
				let trimmed = line.trim();
				if trimmed == "\\e" || trimmed.starts_with("\\e ") {
					// Intercept the \e command
					eprintln!("Editor command intercepted (not yet implemented)");
					// TODO: Open editor, read content, send to psql
					continue;
				}

				// Send the line to psql
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
				// Ctrl-C - send interrupt to psql
				let mut writer = writer.lock().unwrap();
				writer.write_all(&[3]).ok(); // ASCII ETX (Ctrl-C)
				writer.flush().ok();
			}
			Err(rustyline::error::ReadlineError::Eof) => {
				// Ctrl-D - send EOF to psql
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

	// Wait for reader thread to finish
	reader_thread.join().ok();

	// Wait for child to exit
	let status = child.wait().into_diagnostic()?;

	// Compact the history database before exiting
	if let Err(e) = rl.history_mut().compact() {
		eprintln!("Warning: Failed to compact history database: {}", e);
	}

	Ok(status.exit_code() as i32)
}
