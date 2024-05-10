use tracing::instrument;

use crate::{
	commands::Command,
	error::{Error, Result},
};

impl crate::Driver {
	/// Probe how many bytes we can send at once.
	#[instrument(level = "trace", skip(self))]
	pub fn probe_buffer_length(&mut self) -> Result<()> {
		self.flush_buffer()?;

		let mut n = 2048;

		// increase exponentially until we hit the limit
		loop {
			let data = vec![0; n];
			let result = self.write_data(&data);
			self.command(Command::Nop)?;
			n *= 2;
			match result {
				Ok(_) => {}
				Err(Error::Spi(rppal::spi::Error::Io(_))) => {
					break;
				}
				Err(e) => {
					return Err(e);
				}
			}
		}

		// decrease linearly until we can send again
		loop {
			n -= 64;
			let data = vec![0; n];
			let result = self.write_data(&data);
			self.command(Command::Nop)?;
			match result {
				Ok(_) => {
					break;
				}
				Err(Error::Spi(rppal::spi::Error::Io(_))) => {
					continue;
				}
				Err(e) => {
					return Err(e);
				}
			}
		}

		tracing::debug!(n, "probed max usable spi buffer length");
		self.buffer = Vec::with_capacity(n);
		Ok(())
	}

	/// Clear the internal image buffer.
	#[instrument(level = "trace", skip(self))]
	pub fn clear_buffer(&mut self) {
		self.buffer.clear();
	}

	/// Flush the internal image buffer to the display, then clear it.
	///
	/// This is a no-op if the buffer is empty.
	#[instrument(level = "trace", skip(self))]
	pub fn flush_buffer(&mut self) -> Result<()> {
		if self.buffer.is_empty() {
			return Ok(());
		}

		let new = Vec::with_capacity(self.buffer.capacity());
		let buf = std::mem::replace(&mut self.buffer, new);
		self.write_data(&buf)?;
		Ok(())
	}

	/// Write some data to the internal buffer.
	///
	/// If the buffer is full, it will be flushed to the display repeatedly until all the data has
	/// been processed. The buffer is never increased in size.
	#[instrument(level = "trace", skip(self, bytes))]
	pub fn write_data_buffered(&mut self, bytes: &[u8]) -> Result<()> {
		let remaining = self.buffer.capacity() - self.buffer.len();
		if bytes.len() > remaining {
			self.flush_buffer()?;
		}

		for chunk in bytes.chunks(self.buffer.capacity()) {
			self.buffer.extend_from_slice(chunk);
			if self.buffer.len() == self.buffer.capacity() {
				self.flush_buffer()?;
			}
		}

		Ok(())
	}
}
