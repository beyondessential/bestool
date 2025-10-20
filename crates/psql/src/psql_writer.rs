//! Writer wrapper for psql that manages output buffer and writer state together
//!
//! This module provides a wrapper that automatically clears the output buffer
//! when writing to psql, preventing stale buffer content from affecting prompt detection.

use miette::{IntoDiagnostic, Result};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, Mutex};
use tracing::trace;

/// Wrapper that manages the psql writer and output buffer together
///
/// This ensures that whenever we write to psql, we automatically clear the output buffer
/// to prevent stale content from interfering with prompt detection.
pub struct PsqlWriter {
	writer: Arc<Mutex<Box<dyn Write + Send>>>,
	output_buffer: Arc<Mutex<VecDeque<u8>>>,
}

impl PsqlWriter {
	/// Create a new PsqlWriter
	pub fn new(
		writer: Arc<Mutex<Box<dyn Write + Send>>>,
		output_buffer: Arc<Mutex<VecDeque<u8>>>,
	) -> Self {
		Self {
			writer,
			output_buffer,
		}
	}

	/// Write data to psql, automatically clearing the output buffer
	///
	/// This ensures that after we send a command, we don't have stale buffer
	/// content that might be incorrectly detected as a prompt.
	pub fn write(&self, data: &[u8]) -> Result<()> {
		self.output_buffer.lock().unwrap().clear();

		let mut writer = self.writer.lock().unwrap();
		writer.write_all(data).into_diagnostic()?;
		writer.flush().into_diagnostic()?;

		Ok(())
	}

	/// Write a string to psql, automatically clearing the output buffer
	pub fn write_str(&self, s: &str) -> Result<()> {
		self.write(s.as_bytes())
	}

	/// Write a line to psql (appends newline), automatically clearing the output buffer
	pub fn write_line(&self, line: &str) -> Result<()> {
		let data = format!("{}\n", line);
		self.write(data.as_bytes())
	}

	/// Write raw bytes to psql WITHOUT clearing the buffer
	///
	/// This is used for forwarding stdin directly to the PTY (e.g., for pager interaction)
	/// where we don't want to clear the buffer and interfere with output detection.
	pub fn write_bytes(&self, data: &[u8]) -> Result<()> {
		let mut writer = self.writer.lock().unwrap();
		writer.write_all(data).into_diagnostic()?;
		writer.flush().into_diagnostic()?;
		Ok(())
	}

	/// Send a control character to psql (e.g., Ctrl-C, Ctrl-D)
	pub fn send_control(&self, byte: u8) -> Result<()> {
		let mut writer = self.writer.lock().unwrap();
		writer.write_all(&[byte]).into_diagnostic()?;
		writer.flush().into_diagnostic()?;
		Ok(())
	}

	/// Check if the buffer contains a specific pattern
	pub fn buffer_contains(&self, pattern: &str) -> bool {
		let buffer = self.output_buffer.lock().unwrap();
		let buffer_vec: Vec<u8> = buffer.iter().copied().collect();
		let buffer_str = String::from_utf8_lossy(&buffer_vec);
		let res = buffer_str.contains(pattern);
		trace!(buffer=%buffer_str, matches=res, "buffer_contains");
		res
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::io::Cursor;

	#[test]
	fn test_write_clears_buffer() {
		let cursor = Cursor::new(Vec::new());
		let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(cursor)));
		let buffer = Arc::new(Mutex::new(VecDeque::new()));

		buffer.lock().unwrap().extend(b"stale data");
		assert_eq!(buffer.lock().unwrap().len(), 10);

		let psql_writer = PsqlWriter::new(writer, buffer.clone());

		psql_writer.write(b"test").unwrap();
		assert_eq!(buffer.lock().unwrap().len(), 0);
	}

	#[test]
	fn test_buffer_contains() {
		let cursor = Cursor::new(Vec::new());
		let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(Box::new(cursor)));
		let buffer = Arc::new(Mutex::new(VecDeque::new()));

		buffer.lock().unwrap().extend(b"<<<BOUNDARY|||");

		let psql_writer = PsqlWriter::new(writer, buffer);

		assert!(psql_writer.buffer_contains("<<<BOUNDARY|||"));
		assert!(!psql_writer.buffer_contains("not present"));
	}
}
