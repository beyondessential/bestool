//! PTY reader thread for handling output from psql

use crate::prompt::PromptInfo;
use std::collections::VecDeque;
use std::io::Read;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use tracing::trace;

/// Spawn a background thread to read from the PTY and handle output
///
/// This thread:
/// - Reads data from the PTY master
/// - Filters out echoed input
/// - Detects and formats prompts
/// - Maintains a ring buffer for prompt detection
/// - Writes output to stdout
pub fn spawn_reader_thread(
	mut reader: Box<dyn Read + Send>,
	boundary: String,
	output_buffer: Arc<Mutex<VecDeque<u8>>>,
	current_prompt: Arc<Mutex<String>>,
	current_prompt_info: Arc<Mutex<Option<PromptInfo>>>,
	last_input: Arc<Mutex<String>>,
	running: Arc<Mutex<bool>>,
) -> JoinHandle<()> {
	thread::spawn(move || {
		let mut buf = [0u8; 4096];
		let mut skip_next = 0usize;

		loop {
			match reader.read(&mut buf) {
				Ok(0) => break, // EOF
				Ok(n) => {
					// Store in buffer for prompt detection (ring buffer, keeps last 1024 bytes)
					let mut buffer = output_buffer.lock().unwrap();
					for &byte in &buf[..n] {
						if buffer.len() >= 1024 {
							buffer.pop_front();
						}
						buffer.push_back(byte);
					}
					drop(buffer);

					let mut data = String::from_utf8_lossy(&buf[..n]).to_string();
					trace!(data, "read some data");

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
						// Check if this contains our prompt boundary marker
						if let Some(prompt_info) = PromptInfo::parse(&data, &boundary) {
							// Replace the boundary marker with formatted prompt
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

						print!("{}", data);
						use std::io::Write;
						std::io::stdout().flush().ok();
					}
				}
				Err(_) => break,
			}
		}
		*running.lock().unwrap() = false;
	})
}
