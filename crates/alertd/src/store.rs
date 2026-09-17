//! Where a check remembers a reading between sweeps.
//!
//! A separate abstraction from the substrate: what a check can find out and
//! where it may remember things are independent questions, and a process
//! observing applications remotely supplies its own storage without having to
//! be the thing that reads them.
//!
//! The store a check is handed is already scoped to the subject it reports for,
//! so several applications driven from one process never read or write each
//! other's history and a check's baseline is always a baseline for the
//! application it is grading.
//!
//! spec: SUB#check-state

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use tokio::task::spawn_blocking;
use tracing::debug;

use crate::subject::Subject;

/// Whether a stored reading survives its application's compute being switched
/// off.
///
/// Declared as the reading is stored rather than separately from it: a check
/// cannot record something without saying which it is, so state that should not
/// outlive a sleep is never retained by omission, and the declaration cannot
/// drift from the thing it describes.
///
/// spec: SUB#check-state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifetime {
	/// Read from something that restarts when the application's compute does.
	///
	/// Those counters restart from zero on waking, so a retained baseline would
	/// read the fresh counters as a reset or, worse, as a plausible delta.
	UntilCompute,
	/// Measures something the application's own data holds.
	///
	/// Kept, so a quantity that moved while the application slept is still
	/// visible as having moved when it wakes.
	Durable,
}

impl Lifetime {
	pub const ALL: [Self; 2] = [Self::UntilCompute, Self::Durable];

	fn as_str(self) -> &'static str {
		match self {
			Self::UntilCompute => "until-compute",
			Self::Durable => "durable",
		}
	}
}

/// Where a check keeps what it remembers between sweeps.
///
/// Scoped to one subject by whoever hands it over, so a key namespace is the
/// check's own and naming another subject's state is not expressible.
///
/// spec: SUB#check-state
#[async_trait]
pub trait CheckStore: Send + Sync {
	async fn get(&self, key: &str) -> Option<Vec<u8>>;

	async fn put(&self, key: &str, value: &[u8], lifetime: Lifetime);

	async fn clear(&self, key: &str);

	/// Drop everything stored [`UntilCompute`](Lifetime::UntilCompute).
	///
	/// Called when a sweep observes that the application's compute is off, so a
	/// check reads no baseline taken before the sleep.
	async fn discard_until_compute(&self);
}

/// A store backed by this machine's cache directory.
///
/// One directory per subject, and within it one per lifetime, so the state of
/// two applications driven from one process cannot collide and discarding on
/// sleep is removing a directory rather than reading every entry to see which
/// of them said it should go.
pub struct FileStore {
	root: Option<PathBuf>,
}

impl FileStore {
	/// The store for `subject` under this machine's cache directory.
	///
	/// A machine with no cache directory gets a store that remembers nothing:
	/// a check with no baseline falls back to whatever it does on a cold start,
	/// which is the same thing an empty cache gives it.
	pub fn for_subject(subject: &Subject) -> Self {
		Self {
			root: dirs::cache_dir().map(|dir| {
				dir.join("bestool")
					.join("checks")
					.join(subject.key().unwrap_or("machine"))
			}),
		}
	}

	fn dir(&self, lifetime: Lifetime) -> Option<PathBuf> {
		self.root.as_ref().map(|root| root.join(lifetime.as_str()))
	}

	fn path(&self, key: &str, lifetime: Lifetime) -> Option<PathBuf> {
		Some(self.dir(lifetime)?.join(safe_key(key)))
	}
}

/// A key as a single filesystem name.
///
/// Check names are plain identifiers, so this only ever has to defend against a
/// caller that passes something else: anything outside the allowed set becomes
/// an underscore, which cannot escape the subject's directory.
fn safe_key(key: &str) -> String {
	key.chars()
		.map(|c| {
			if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
				c
			} else {
				'_'
			}
		})
		.collect()
}

#[async_trait]
impl CheckStore for FileStore {
	async fn get(&self, key: &str) -> Option<Vec<u8>> {
		let paths: Vec<PathBuf> = Lifetime::ALL
			.iter()
			.filter_map(|lifetime| self.path(key, *lifetime))
			.collect();
		if paths.is_empty() {
			return None;
		}

		// Small reads, but on the filesystem: off the executor so they cannot
		// stall the checks sharing it.
		spawn_blocking(move || paths.iter().find_map(|path| std::fs::read(path).ok()))
			.await
			.unwrap_or_default()
	}

	async fn put(&self, key: &str, value: &[u8], lifetime: Lifetime) {
		let Some(path) = self.path(key, lifetime) else {
			return;
		};
		// A key rewritten under a different lifetime must not leave the old copy
		// behind for `get` to find.
		let stale: Vec<PathBuf> = Lifetime::ALL
			.iter()
			.filter(|other| **other != lifetime)
			.filter_map(|other| self.path(key, *other))
			.collect();
		let value = value.to_vec();

		let _ = spawn_blocking(move || {
			for path in stale {
				let _ = std::fs::remove_file(path);
			}
			if let Some(dir) = path.parent()
				&& let Err(err) = std::fs::create_dir_all(dir)
			{
				debug!(%err, ?dir, "could not create the check store directory");
				return;
			}
			if let Err(err) = write_atomically(&path, &value) {
				debug!(%err, ?path, "could not write check state");
			}
		})
		.await;
	}

	async fn clear(&self, key: &str) {
		let paths: Vec<PathBuf> = Lifetime::ALL
			.iter()
			.filter_map(|lifetime| self.path(key, *lifetime))
			.collect();
		let _ = spawn_blocking(move || {
			for path in paths {
				let _ = std::fs::remove_file(path);
			}
		})
		.await;
	}

	async fn discard_until_compute(&self) {
		let Some(dir) = self.dir(Lifetime::UntilCompute) else {
			return;
		};
		let _ = spawn_blocking(move || {
			if let Err(err) = std::fs::remove_dir_all(&dir)
				&& err.kind() != std::io::ErrorKind::NotFound
			{
				debug!(%err, ?dir, "could not discard check state held until compute");
			}
		})
		.await;
	}
}

/// A store that remembers nothing past the process that wrote it.
///
/// For a consumer with no business leaving anything behind — a one-shot sweep
/// from the command line — and for tests, which must not read whatever the
/// machine running them happens to have cached.
#[derive(Default)]
pub struct MemoryStore {
	entries: std::sync::Mutex<std::collections::HashMap<String, (Lifetime, Vec<u8>)>>,
}

impl MemoryStore {
	pub fn new() -> Self {
		Self::default()
	}
}

#[async_trait]
impl CheckStore for MemoryStore {
	async fn get(&self, key: &str) -> Option<Vec<u8>> {
		let entries = self.entries.lock().expect("check store lock");
		entries.get(key).map(|(_, value)| value.clone())
	}

	async fn put(&self, key: &str, value: &[u8], lifetime: Lifetime) {
		let mut entries = self.entries.lock().expect("check store lock");
		entries.insert(key.to_string(), (lifetime, value.to_vec()));
	}

	async fn clear(&self, key: &str) {
		let mut entries = self.entries.lock().expect("check store lock");
		entries.remove(key);
	}

	async fn discard_until_compute(&self) {
		let mut entries = self.entries.lock().expect("check store lock");
		entries.retain(|_, (lifetime, _)| *lifetime != Lifetime::UntilCompute);
	}
}

/// Write through a temporary file in the same directory, so a sweep killed
/// mid-write leaves the previous state rather than a truncated file a check
/// would read as corrupt and throw away.
fn write_atomically(path: &Path, value: &[u8]) -> std::io::Result<()> {
	let tmp = path.with_extension("tmp");
	std::fs::write(&tmp, value)?;
	std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::subject::{ApplicationKind, ApplicationRef};

	fn store(root: &Path, subject: &Subject) -> FileStore {
		FileStore {
			root: Some(root.join(subject.key().unwrap_or("machine"))),
		}
	}

	fn tempdir() -> PathBuf {
		let dir = std::env::temp_dir().join(format!(
			"bestool-store-test-{}-{:?}",
			std::process::id(),
			std::thread::current().id()
		));
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).expect("a temp dir");
		dir
	}

	/// Two applications driven from one process must never read each other's
	/// history: a baseline is always a baseline for the application being
	/// graded.
	///
	/// spec: SUB#check-state
	#[tokio::test]
	async fn one_subject_cannot_read_another_s_state() {
		let root = tempdir();
		let central = Subject::Application(ApplicationRef::tamanu(ApplicationKind::TamanuCentral));
		let facility =
			Subject::Application(ApplicationRef::tamanu(ApplicationKind::TamanuFacility));

		store(&root, &central)
			.put("http_errors", b"central", Lifetime::UntilCompute)
			.await;
		store(&root, &facility)
			.put("http_errors", b"facility", Lifetime::UntilCompute)
			.await;

		assert_eq!(
			store(&root, &central).get("http_errors").await.as_deref(),
			Some(&b"central"[..])
		);
		assert_eq!(
			store(&root, &facility).get("http_errors").await.as_deref(),
			Some(&b"facility"[..])
		);

		let _ = std::fs::remove_dir_all(&root);
	}

	/// The machine's own state sits apart from every application's, so a check
	/// that reports for the machine is not competing with one reporting for an
	/// application that happens to share its name.
	#[tokio::test]
	async fn the_machine_has_its_own_state() {
		let root = tempdir();
		let machine = Subject::Machine;
		let app = Subject::Application(ApplicationRef::local_postgres(5432));

		store(&root, &machine)
			.put("ips", b"machine", Lifetime::Durable)
			.await;
		assert!(store(&root, &app).get("ips").await.is_none());

		let _ = std::fs::remove_dir_all(&root);
	}

	/// State read from something that restarts with the compute goes when the
	/// compute does; state measuring the application's own data stays, so a
	/// quantity that moved during the sleep is still visible as having moved.
	///
	/// spec: SUB#check-state
	#[tokio::test]
	async fn a_sleep_drops_only_what_the_compute_carried() {
		let root = tempdir();
		let subject = Subject::Application(ApplicationRef::tamanu(ApplicationKind::TamanuCentral));
		let store = store(&root, &subject);

		store
			.put("http_errors", b"counters", Lifetime::UntilCompute)
			.await;
		store.put("fhir_jobs", b"queue", Lifetime::Durable).await;

		store.discard_until_compute().await;

		assert!(store.get("http_errors").await.is_none());
		assert_eq!(store.get("fhir_jobs").await.as_deref(), Some(&b"queue"[..]));

		let _ = std::fs::remove_dir_all(&root);
	}

	/// A check that changes its mind about a reading's lifetime must not leave
	/// the old copy behind for the next read to find.
	#[tokio::test]
	async fn rewriting_under_a_new_lifetime_moves_the_value() {
		let root = tempdir();
		let subject = Subject::Application(ApplicationRef::tamanu(ApplicationKind::TamanuCentral));
		let store = store(&root, &subject);

		store.put("thing", b"old", Lifetime::Durable).await;
		store.put("thing", b"new", Lifetime::UntilCompute).await;
		assert_eq!(store.get("thing").await.as_deref(), Some(&b"new"[..]));

		store.discard_until_compute().await;
		assert!(store.get("thing").await.is_none());

		let _ = std::fs::remove_dir_all(&root);
	}

	/// A store with nowhere to write remembers nothing rather than failing: a
	/// check with no baseline does whatever it does on a cold start.
	#[tokio::test]
	async fn a_store_with_no_cache_directory_remembers_nothing() {
		let store = FileStore { root: None };
		store.put("thing", b"value", Lifetime::Durable).await;
		assert!(store.get("thing").await.is_none());
		store.clear("thing").await;
		store.discard_until_compute().await;
	}

	#[test]
	fn a_key_cannot_escape_its_subject_s_directory() {
		assert_eq!(safe_key("../../etc/passwd"), ".._.._etc_passwd");
		assert_eq!(safe_key("http_errors"), "http_errors");
	}
}
