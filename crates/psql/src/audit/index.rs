use miette::{IntoDiagnostic, Result};
use redb::{ReadableDatabase, ReadableTable, ReadableTableMetadata};
use tracing::instrument;

impl super::Audit {
	/// Get the length of the history index (number of history entries)
	#[instrument(level = "trace", skip(self))]
	pub(crate) fn hist_index_len(&self) -> Result<u64> {
		let read_txn = self.db.begin_read().into_diagnostic()?;
		let table = match read_txn.open_table(super::INDEX_TABLE) {
			Ok(table) => table,
			Err(_) => return Ok(0),
		};
		table.len().into_diagnostic()
	}

	/// Get timestamp at a given history index
	#[instrument(level = "trace", skip(self))]
	pub(crate) fn hist_index_get(&self, index: u64) -> Result<Option<u64>> {
		let read_txn = self.db.begin_read().into_diagnostic()?;
		let table = match read_txn.open_table(super::INDEX_TABLE) {
			Ok(table) => table,
			Err(_) => return Ok(None),
		};
		Ok(table.get(index).into_diagnostic()?.map(|v| v.value()))
	}

	/// Add a timestamp to the history index
	#[instrument(level = "trace", skip(self))]
	pub(crate) fn hist_index_push(&self, timestamp: u64) -> Result<()> {
		let write_txn = self.db.begin_write().into_diagnostic()?;
		{
			let mut index_table = write_txn.open_table(super::INDEX_TABLE).into_diagnostic()?;
			let new_index = index_table.len().into_diagnostic()?;
			index_table.insert(new_index, timestamp).into_diagnostic()?;
		}
		write_txn.commit().into_diagnostic()?;
		Ok(())
	}

	/// Remove entries from the start of the history index and shift remaining entries
	#[instrument(level = "trace", skip(self))]
	pub(crate) fn hist_index_remove_prefix(&self, count: u64) -> Result<()> {
		let write_txn = self.db.begin_write().into_diagnostic()?;
		{
			let mut index_table = write_txn.open_table(super::INDEX_TABLE).into_diagnostic()?;
			let index_len = index_table.len().into_diagnostic()?;

			if count == 0 || count > index_len {
				return Ok(());
			}

			let remaining = index_len - count;

			// First collect all timestamps that need to be shifted
			let mut timestamps_to_shift = Vec::with_capacity(remaining as usize);
			for i in 0..remaining {
				if let Some(timestamp) = index_table.get(i + count).into_diagnostic()? {
					timestamps_to_shift.push(timestamp.value());
				}
			}

			// Now shift them
			for (i, ts) in timestamps_to_shift.into_iter().enumerate() {
				index_table.insert(i as u64, ts).into_diagnostic()?;
			}

			// Remove old indices at the end
			for i in remaining..index_len {
				index_table.remove(i).into_diagnostic()?;
			}
		}
		write_txn.commit().into_diagnostic()?;
		Ok(())
	}
}
