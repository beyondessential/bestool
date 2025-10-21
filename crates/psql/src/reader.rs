//! PTY reader thread for handling output from psql
//!
//! ## Windows CPR (Cursor Position Report) Handling
//!
//! On Windows, psql sends a CPR escape sequence `\x1b[6n` to detect terminal capabilities
//! and cursor position. This is a standard ANSI escape sequence that terminals respond to
//! with `\x1b[{row};{col}R` (e.g., `\x1b[1;1R` for row 1, column 1).
//!
//! Without a proper response, psql will hang indefinitely waiting for the terminal to reply.
//! This reader thread detects CPR requests on Windows and sends back a valid response to
//! prevent psql from spinning and blocking on startup.
//!
//! The fix:
//! 1. Detect `\x1b[6n` in the data read from the PTY
//! 2. Send back `\x1b[1;1R` (cursor at row 1, column 1) to satisfy psql
//! 3. Remove the CPR sequence from the output so it doesn't get printed
//!
//! This issue is Windows-specific because the PTY implementation behaves differently
//! from Unix pseudoterminals, which typically handle these sequences transparently.

use crate::prompt::PromptInfo;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
#[cfg(windows)]
use tracing::debug;
use tracing::trace;

static DEBUG_PTY: OnceLock<bool> = OnceLock::new();

fn is_debug_pty() -> bool {
	*DEBUG_PTY.get_or_init(|| std::env::var("DEBUG_PTY").is_ok())
}

/// Parameters for the reader thread
pub struct ReaderThreadParams {
	pub reader: Box<dyn Read + Send>,
	pub boundary: String,
	pub output_buffer: Arc<Mutex<VecDeque<u8>>>,
	pub current_prompt: Arc<Mutex<String>>,
	pub current_prompt_info: Arc<Mutex<Option<PromptInfo>>>,
	pub last_input: Arc<Mutex<String>>,
	pub running: Arc<Mutex<bool>>,
	pub print_enabled: Arc<Mutex<bool>>,
	pub writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

/// Spawn a background thread to read from the PTY and handle output
///
/// This thread:
/// - Reads data from the PTY master
/// - Filters out echoed input
/// - Detects and formats prompts
/// - Maintains a ring buffer for prompt detection
/// - Writes output to stdout
pub fn spawn_reader_thread(params: ReaderThreadParams) -> JoinHandle<()> {
	let ReaderThreadParams {
		mut reader,
		boundary,
		output_buffer,
		current_prompt,
		current_prompt_info,
		last_input,
		running,
		print_enabled,
		writer,
	} = params;

	// Suppress unused warning on non-Windows platforms
	#[cfg(not(windows))]
	let _ = &writer;

	thread::spawn(move || {
		let mut buf = [0u8; 4096];
		let mut skip_next = 0usize;

		loop {
			match reader.read(&mut buf) {
				Ok(0) => break, // EOF
				Ok(n) => {
					let mut data = String::from_utf8_lossy(&buf[..n]).to_string();
					trace!(data, "read some data");

					// On Windows, respond to CPR (Cursor Position Report) requests
					// psql sends \x1b[6n to query cursor position and waits for a response
					#[cfg(windows)]
					if data.contains("\x1b[6n") {
						debug!("detected CPR request, sending response");
						// Respond with cursor position (row 1, col 1)
						// Format: ESC [ row ; col R
						let response = b"\x1b[1;1R";
						if let Ok(mut w) = writer.lock() {
							let _ = w.write_all(response);
							let _ = w.flush();
						}
						// Remove CPR from data so it doesn't get printed
						data = data.replace("\x1b[6n", "");
					}

					// Store in buffer for prompt detection (ring buffer, keeps last 1024 bytes)
					let mut buffer = output_buffer.lock().unwrap();
					for &byte in &buf[..n] {
						if buffer.len() >= 1024 {
							buffer.pop_front();
						}
						buffer.push_back(byte);
					}
					drop(buffer);

					// Filter out echoed input if we're expecting echo
					if skip_next > 0 {
						let to_skip = skip_next.min(data.len());
						data.drain(..to_skip);
						skip_next -= to_skip;
					} else {
						let expected_echo = last_input.lock().unwrap().clone();
						if !expected_echo.is_empty() {
							// PTY converts \n to \r\n
							let normalized = expected_echo.replace('\n', "\r\n");
							if data.starts_with(&normalized) {
								data.drain(..normalized.len());
								last_input.lock().unwrap().clear();
							} else if normalized.starts_with(&data) {
								// Partial match - skip this chunk and continue
								skip_next = normalized.len() - data.len();
								data.clear();
							}
						}
					}

					if !data.is_empty() {
						// Check accumulated buffer for prompt boundary marker (handles split reads)
						let buffer_str = {
							let buffer = output_buffer.lock().unwrap();
							let buffer_vec: Vec<u8> = buffer.iter().copied().collect();
							String::from_utf8_lossy(&buffer_vec).to_string()
						};

						if let Some(prompt_info) = PromptInfo::parse(&buffer_str, &boundary) {
							// Replace the boundary marker with formatted prompt in current data chunk
							let formatted = prompt_info.format_prompt();
							let marker = format!("<<<{}|||", boundary);
							if let Some(start) = data.find(&marker) {
								if let Some(end) = data[start..].find(">>>") {
									let full_marker_end = start + end + 3;
									data.replace_range(start..full_marker_end, &formatted);
								}
							}

							*current_prompt.lock().unwrap() = formatted;
							*current_prompt_info.lock().unwrap() = Some(prompt_info);
						}

						if *print_enabled.lock().unwrap() {
							use std::io::Write;
							if is_debug_pty() {
								// Wrap output with cyan [PTY] marker for debugging
								for line in data.lines() {
									println!("\x1b[36m[PTY]\x1b[0m {}", line);
								}
								// Handle trailing data without newline
								if !data.ends_with('\n') && !data.ends_with('\r') {
									if let Some(_last_line) = data.lines().last() {
										// Already printed
									} else {
										print!("\x1b[36m[PTY]\x1b[0m {}", data);
									}
								}
							} else {
								print!("{}", data);
							}
							std::io::stdout().flush().ok();
						}
					}
				}
				Err(_) => break,
			}
		}
		*running.lock().unwrap() = false;
	})
}
