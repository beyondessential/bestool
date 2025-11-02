use std::{borrow::Cow, path::Path};

use rustyline::history::{History as RustylineHistory, SearchDirection, SearchResult};
use tracing::trace;

impl RustylineHistory for super::Audit {
	fn get(
		&self,
		index: usize,
		_dir: SearchDirection,
	) -> rustyline::Result<Option<SearchResult<'_>>> {
		let timestamp = match self.hist_index_get(index as u64) {
			Ok(Some(ts)) => ts,
			Ok(None) => return Ok(None),
			Err(e) => {
				return Err(rustyline::error::ReadlineError::Io(std::io::Error::other(
					e.to_string(),
				)));
			}
		};

		let entry = self.get_entry(timestamp).map_err(|e| {
			rustyline::error::ReadlineError::Io(std::io::Error::other(e.to_string()))
		})?;

		// Entry may have been deleted by another process
		let entry = match entry {
			Some(e) => e,
			None => return Ok(None),
		};

		Ok(Some(SearchResult {
			entry: Cow::Owned(entry.query),
			idx: index,
			pos: 0,
		}))
	}

	fn add(&mut self, _line: &str) -> rustyline::Result<bool> {
		trace!("Audit::add called and ignored");
		Ok(true)
	}

	fn add_owned(&mut self, _line: String) -> rustyline::Result<bool> {
		trace!("Audit::add_owned called and ignored");
		Ok(true)
	}

	fn len(&self) -> usize {
		self.hist_index_len().unwrap_or(0) as usize
	}

	fn is_empty(&self) -> bool {
		self.hist_index_len().unwrap_or(0) == 0
	}

	fn set_max_len(&mut self, _len: usize) -> rustyline::Result<()> {
		// No-op: we don't clear audit logs through rustyline
		Ok(())
	}

	fn ignore_dups(&mut self, _yes: bool) -> rustyline::Result<()> {
		// No-op: we never ignore duplicates
		Ok(())
	}

	fn ignore_space(&mut self, _yes: bool) {
		// No-op: we never ignore entries
	}

	fn save(&mut self, _path: &Path) -> rustyline::Result<()> {
		// No-op: already persisted to database
		Ok(())
	}

	fn append(&mut self, _path: &Path) -> rustyline::Result<()> {
		// No-op: already persisted to database
		Ok(())
	}

	fn load(&mut self, _path: &Path) -> rustyline::Result<()> {
		// No-op: loaded from database
		Ok(())
	}

	fn clear(&mut self) -> rustyline::Result<()> {
		// No-op: we don't clear audit logs
		Ok(())
	}

	fn search(
		&self,
		term: &str,
		start: usize,
		dir: SearchDirection,
	) -> rustyline::Result<Option<SearchResult<'_>>> {
		let len = self.hist_index_len().map_err(|e| {
			rustyline::error::ReadlineError::Io(std::io::Error::other(e.to_string()))
		})? as usize;

		let range: Box<dyn Iterator<Item = usize>> = match dir {
			SearchDirection::Forward => {
				if start >= len {
					return Ok(None);
				}
				Box::new(start..len)
			}
			SearchDirection::Reverse => {
				if start >= len {
					return Ok(None);
				}
				Box::new((0..=start).rev())
			}
		};

		for idx in range {
			let timestamp = match self.hist_index_get(idx as u64) {
				Ok(Some(ts)) => ts,
				Ok(None) => continue,
				Err(e) => {
					return Err(rustyline::error::ReadlineError::Io(std::io::Error::other(
						e.to_string(),
					)));
				}
			};

			let entry = self.get_entry(timestamp).map_err(|e| {
				rustyline::error::ReadlineError::Io(std::io::Error::other(e.to_string()))
			})?;

			let entry = match entry {
				Some(e) => e,
				None => continue,
			};

			if let Some(pos) = entry.query.find(term) {
				return Ok(Some(SearchResult {
					entry: Cow::Owned(entry.query),
					idx,
					pos,
				}));
			}
		}

		Ok(None)
	}

	fn starts_with(
		&self,
		term: &str,
		start: usize,
		dir: SearchDirection,
	) -> rustyline::Result<Option<SearchResult<'_>>> {
		let len = self.hist_index_len().map_err(|e| {
			rustyline::error::ReadlineError::Io(std::io::Error::other(e.to_string()))
		})? as usize;

		let range: Box<dyn Iterator<Item = usize>> = match dir {
			SearchDirection::Forward => {
				if start >= len {
					return Ok(None);
				}
				Box::new(start..len)
			}
			SearchDirection::Reverse => {
				if start >= len {
					return Ok(None);
				}
				Box::new((0..=start).rev())
			}
		};

		for idx in range {
			let timestamp = match self.hist_index_get(idx as u64) {
				Ok(Some(ts)) => ts,
				Ok(None) => continue,
				Err(e) => {
					return Err(rustyline::error::ReadlineError::Io(std::io::Error::other(
						e.to_string(),
					)));
				}
			};

			let entry = self.get_entry(timestamp).map_err(|e| {
				rustyline::error::ReadlineError::Io(std::io::Error::other(e.to_string()))
			})?;

			let entry = match entry {
				Some(e) => e,
				None => continue,
			};

			if entry.query.starts_with(term) {
				return Ok(Some(SearchResult {
					entry: Cow::Owned(entry.query),
					idx,
					pos: 0,
				}));
			}
		}

		Ok(None)
	}
}
