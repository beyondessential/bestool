//! Readings several checks take that are the machine's rather than one
//! application's, taken once per sweep.
//!
//! A check registered against `tamanu_app` runs once per application on the
//! host, and two of them — `canopy_certificates` and `caddy_certs` — each want
//! Canopy's entitlement answer, Caddy's live admin configuration, and the chains
//! the daemon has collected. None of those three is an application's: they are
//! one answer for the machine, and asking for them per check per application
//! costs a round trip, a config fetch and a directory of X.509 parses each time
//! for the same result.
//!
//! So a sweep builds one of these and hands it to every context it builds. Each
//! reading is taken lazily on first ask and shared from then on, which keeps a
//! sweep that runs neither check from paying for either. A fresh cache per sweep
//! is what bounds the staleness: nothing here outlives the sweep that built it.

use std::{
	collections::{BTreeMap, BTreeSet},
	future::Future,
	path::PathBuf,
	sync::Arc,
};

use bestool_canopy::{
	CanopyClient,
	certificates::StoredRefusal,
	names::{Entitlement, Refusal},
};
use serde_json::Value;
use tokio::sync::OnceCell;
use tracing::debug;

use crate::{
	checks::fmt_chain,
	ownership::{CaddySites, HostApplication, Ownership},
	runtime::{Certificate, Unavailable, caddy},
};

/// What Canopy last refused each DNS name for, as the daemon recorded it.
///
/// spec: CHK-CCO#which-dns-names-it-grades
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefusalRecord {
	/// Keyed by DNS name, lower-cased.
	refusals: BTreeMap<String, Refusal>,
}

impl RefusalRecord {
	pub fn from_stored(stored: BTreeMap<String, StoredRefusal>) -> Self {
		let refusals = stored
			.into_iter()
			.map(|(name, kept)| {
				(
					name.trim().to_ascii_lowercase(),
					Refusal {
						kind: kept.kind,
						reason: kept.reason,
					},
				)
			})
			.collect();
		Self { refusals }
	}

	pub fn refusal(&self, name: &str) -> Option<&Refusal> {
		self.refusals.get(&name.to_ascii_lowercase())
	}
}

/// One sweep's shared readings. Cheap to build; nothing is asked for until it is
/// wanted.
#[derive(Default)]
pub struct SweepCache {
	/// Where the daemon keeps its canopy state; the default location when
	/// `None`.
	store_dir: Option<PathBuf>,
	/// The applications on this machine that Caddy can be fronting, which is what
	/// a DNS name's site is attributed against.
	host_applications: Vec<HostApplication>,
	/// Caddy's live admin configuration, or `None` where its admin API could not
	/// be read.
	caddy_config: OnceCell<Option<Arc<Value>>>,
	/// The sites in that configuration, parsed once.
	caddy_sites: OnceCell<Option<Arc<CaddySites>>>,
	/// The addresses of those sites that are certified from canopy.
	caddy_subjects: OnceCell<Option<Arc<BTreeSet<String>>>>,
	/// Which application each DNS name belongs to.
	ownership: OnceCell<Option<Arc<Ownership>>>,
	/// What Canopy last refused each DNS name for, or why the daemon's record of
	/// it could not be read.
	refusals: OnceCell<Result<Arc<RefusalRecord>, String>>,
	/// The chains the daemon has collected from Canopy, read from disk, keyed by
	/// the name each covers.
	canopy_chains: OnceCell<Result<Arc<BTreeMap<String, String>>, String>>,
	/// Those chains parsed, which is the form the certificate substrate wants.
	/// Held apart from the read above because the two consumers want different
	/// shapes of the same file, and neither should pay for the other's.
	parsed_chains: OnceCell<Arc<Vec<(String, caddy::DiskCert)>>>,
	/// The validity window of each collected chain, keyed by the name it covers.
	/// Read off the parses above rather than off the PEM again.
	chain_validity: OnceCell<Arc<BTreeMap<String, (i64, i64)>>>,
	/// What Canopy last said this machine may do.
	entitlement: OnceCell<Result<Entitlement, String>>,
	/// The certificates the front end has in force, with what it serves for each.
	/// Reading them walks Caddy's store and handshakes against the names, and
	/// the check grading them runs once per application.
	certificates: OnceCell<Result<Vec<Certificate>, Unavailable>>,
}

impl SweepCache {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn with_host_applications(host_applications: Vec<HostApplication>) -> Self {
		Self {
			host_applications,
			..Self::default()
		}
	}

	pub fn host_applications(&self) -> &[HostApplication] {
		&self.host_applications
	}

	/// Read the daemon's canopy state from `dir` instead of its default
	/// location.
	#[cfg(test)]
	fn with_store_dir(mut self, dir: PathBuf) -> Self {
		self.store_dir = Some(dir);
		self
	}

	fn store_dir(&self) -> PathBuf {
		self.store_dir
			.clone()
			.unwrap_or_else(bestool_canopy::certificates::default_dir)
	}

	pub async fn caddy_config(&self) -> Option<Arc<Value>> {
		self.caddy_config
			.get_or_init(|| async { caddy::fetch_admin_config().await.map(Arc::new) })
			.await
			.clone()
	}

	/// The sites in Caddy's configuration, or `None` where it could not be read.
	pub async fn caddy_sites(&self) -> Option<Arc<CaddySites>> {
		self.caddy_sites
			.get_or_init(|| async {
				let config = self.caddy_config().await?;
				Some(Arc::new(CaddySites::from_config(&config)))
			})
			.await
			.clone()
	}

	/// The site addresses whose TLS automation policy names the daemon's
	/// certificate endpoint: the DNS names the host is certified for.
	///
	/// `None` where the configuration could not be read at all, which is how a
	/// host not running Caddy is passed over — as having no list rather than an
	/// empty one. A site that does not name the endpoint is never served from
	/// canopy, so nothing is collected for it.
	///
	/// spec: TLS#which-dns-names-are-certified
	pub async fn caddy_subjects(&self) -> Option<Arc<BTreeSet<String>>> {
		self.caddy_subjects
			.get_or_init(|| async { Some(Arc::new(self.caddy_sites().await?.hooked_addresses())) })
			.await
			.clone()
	}

	/// Which application each DNS name belongs to, or `None` where Caddy's
	/// configuration could not be read.
	///
	/// Canopy's declarations are folded in when a client is given and it
	/// answers; without them, attribution from Caddy's sites still works.
	///
	/// spec: NAM#which-application-a-dns-name-belongs-to
	pub async fn ownership(&self, canopy: Option<&CanopyClient>) -> Option<Arc<Ownership>> {
		self.ownership
			.get_or_init(|| async {
				let sites = self.caddy_sites().await?;
				let entitlement = match canopy {
					Some(canopy) => self.entitlement(canopy).await.unwrap_or_else(|err| {
						debug!(%err, "attributing DNS names without canopy's declarations");
						Entitlement::default()
					}),
					None => Entitlement::default(),
				};
				Some(Arc::new(Ownership::resolve(
					&sites,
					self.host_applications(),
					&entitlement,
				)))
			})
			.await
			.clone()
	}

	/// What Canopy last refused each DNS name for, as the daemon recorded it, or
	/// why that record could not be read.
	///
	/// Read from where the daemon keeps it rather than asked of the running
	/// daemon: the record decides which DNS names go ungraded, and anything
	/// holding the daemon's port could answer for it.
	///
	/// spec: CHK-CCO#which-dns-names-it-grades
	pub async fn refusals(&self) -> Result<Arc<RefusalRecord>, String> {
		self.refusals
			.get_or_init(|| async {
				bestool_canopy::certificates::read_refusals(&self.store_dir())
					.await
					.map(|stored| Arc::new(RefusalRecord::from_stored(stored)))
					.map_err(|err| format!("{err}"))
			})
			.await
			.clone()
	}

	/// The front end's certificates, read by `read` on the first ask in the sweep
	/// and shared from then on.
	pub(crate) async fn certificates(
		&self,
		read: impl Future<Output = Result<Vec<Certificate>, Unavailable>>,
	) -> Result<Vec<Certificate>, Unavailable> {
		self.certificates.get_or_init(|| read).await.clone()
	}

	/// The collected chains, or why they could not be read.
	pub async fn canopy_chains(&self) -> Result<Arc<BTreeMap<String, String>>, String> {
		self.canopy_chains
			.get_or_init(|| async {
				bestool_canopy::certificates::load_chains(&self.store_dir())
					.await
					.map(Arc::new)
					.map_err(|err| format!("{err}"))
			})
			.await
			.clone()
	}

	/// The collected chains, parsed.
	///
	/// Parsing X.509 is CPU work and there is one certificate per name, so it
	/// runs on the blocking pool for the same reason the Caddy store scan beside
	/// it does: one certificate reading must not stall the other checks sharing
	/// the executor.
	pub async fn parsed_canopy_chains(&self) -> Arc<Vec<(String, caddy::DiskCert)>> {
		self.parsed_chains
			.get_or_init(|| async {
				let chains = match self.canopy_chains().await {
					Ok(chains) => chains,
					Err(err) => {
						debug!(%err, "could not read the canopy chain store");
						return Arc::new(Vec::new());
					}
				};
				let parsed = tokio::task::spawn_blocking(move || {
					chains
						.iter()
						.filter_map(|(name, chain)| {
							Some((name.clone(), caddy::parse_cert(chain.as_bytes())?))
						})
						.collect::<Vec<_>>()
				})
				.await
				.unwrap_or_else(|err| {
					debug!(%err, "parsing the canopy chain store did not complete");
					Vec::new()
				});
				Arc::new(parsed)
			})
			.await
			.clone()
	}

	/// When each collected chain is valid from and until, as seconds since the
	/// epoch.
	///
	/// Taken off the parses the sweep already did on the blocking pool, so a
	/// check grading how far a chain has run down does not decode the same PEM
	/// again on a runtime thread — once per name, per application.
	pub async fn canopy_chain_validity(&self) -> Arc<BTreeMap<String, (i64, i64)>> {
		self.chain_validity
			.get_or_init(|| async {
				let parsed = self.parsed_canopy_chains().await;
				Arc::new(
					parsed
						.iter()
						.map(|(name, cert)| (name.clone(), cert.validity()))
						.collect(),
				)
			})
			.await
			.clone()
	}

	/// What Canopy says this machine may do, or why it could not be asked.
	///
	/// A sweep with no Canopy client cannot ask at all, which is not the same as
	/// an ask that failed: the caller distinguishes them, so that case does not
	/// reach here.
	pub async fn entitlement(&self, canopy: &CanopyClient) -> Result<Entitlement, String> {
		self.entitlement
			.get_or_init(|| async {
				canopy
					.names_entitlements()
					.await
					.map(|wire| Entitlement::from_wire(&wire))
					.map_err(|err| fmt_chain(&err))
			})
			.await
			.clone()
	}
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::{AtomicUsize, Ordering};

	use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

	use bestool_canopy::names::RefusalKind;

	use super::*;

	/// Both readings come from one fetch of Caddy's admin API, and a second ask
	/// re-fetches nothing: the checks that want it run once per application, so
	/// a two-application host would otherwise fetch the same configuration four
	/// times.
	#[tokio::test]
	async fn caddys_configuration_is_fetched_once_for_the_sweep() {
		// The admin API's address is fixed at localhost:2019, so this can only
		// exercise the caching where that port is free to bind.
		let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:2019").await else {
			return;
		};

		let hits = Arc::new(AtomicUsize::new(0));
		let counted = hits.clone();
		tokio::spawn(async move {
			const BODY: &str = r#"{"apps":{"http":{"servers":{"s":{"routes":[{"match":[{"host":["app.example.com","plain.example.com"]}]}]}}},"tls":{"automation":{"policies":[{"subjects":["app.example.com"],"get_certificate":[{"via":"http","url":"http://127.0.0.1:8271/certificate"}]}]}}}}"#;
			while let Ok((mut stream, _)) = listener.accept().await {
				counted.fetch_add(1, Ordering::SeqCst);
				let mut scratch = [0u8; 1024];
				let _ = stream.read(&mut scratch).await;
				let _ = stream
					.write_all(
						format!(
							"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
							BODY.len()
						)
						.as_bytes(),
					)
					.await;
				let _ = stream.shutdown().await;
			}
		});

		let cache = SweepCache::new();
		assert!(cache.caddy_config().await.is_some());
		let subjects = cache.caddy_subjects().await.unwrap();
		assert!(subjects.contains("app.example.com"));
		assert!(
			!subjects.contains("plain.example.com"),
			"a site that does not name the daemon is not certified"
		);
		assert!(Arc::ptr_eq(
			&subjects,
			&cache.caddy_subjects().await.unwrap()
		));
		assert!(cache.ownership(None).await.is_some());
		assert_eq!(hits.load(Ordering::SeqCst), 1);
	}

	/// A chain store that cannot be read is an error each asker sees, not one
	/// swallowed by the first — and it is still only read once.
	#[tokio::test]
	async fn the_chain_store_is_read_once_and_its_answer_shared() {
		let cache = SweepCache::new();
		let first = cache.canopy_chains().await;
		let second = cache.canopy_chains().await;
		match (first, second) {
			(Ok(a), Ok(b)) => assert!(Arc::ptr_eq(&a, &b)),
			(Err(a), Err(b)) => assert_eq!(a, b),
			_ => panic!("the same read answered two different ways"),
		}
	}

	/// The front end's certificates are read once however many applications'
	/// runs grade them.
	#[tokio::test]
	async fn the_certificates_are_read_once_for_the_sweep() {
		let cache = SweepCache::new();
		let reads = AtomicUsize::new(0);
		let read = || async {
			reads.fetch_add(1, Ordering::SeqCst);
			Err(Unavailable::new("no front end"))
		};
		let first = cache.certificates(read()).await;
		let second = cache.certificates(read()).await;
		assert_eq!(first, second);
		assert_eq!(reads.load(Ordering::SeqCst), 1);
	}

	fn kept(kind: RefusalKind, reason: &str) -> StoredRefusal {
		StoredRefusal {
			kind,
			reason: reason.into(),
			application_type: "tamanu-central".into(),
			asked_at: None,
		}
	}

	/// The record is read once however many applications' runs want it, and is
	/// read as typed refusals.
	#[tokio::test]
	async fn the_refusal_record_is_read_once_for_the_sweep() {
		let dir = tempfile::tempdir().unwrap();
		let mut stored = BTreeMap::new();
		stored.insert(
			"Wait.example.com".to_owned(),
			kept(RefusalKind::Undeclared, "needs declaring"),
		);
		stored.insert(
			"mismatch.example.com".to_owned(),
			kept(RefusalKind::Other, "this machine hosts tamanu-central"),
		);
		bestool_canopy::certificates::store_refusals(dir.path(), &stored)
			.await
			.unwrap();
		let cache = SweepCache::new().with_store_dir(dir.path().to_path_buf());

		let first = cache.refusals().await.unwrap();
		bestool_canopy::certificates::store_refusals(dir.path(), &BTreeMap::new())
			.await
			.unwrap();
		let second = cache.refusals().await.unwrap();
		assert!(Arc::ptr_eq(&first, &second));

		let waiting = first.refusal("wait.example.com").unwrap();
		assert_eq!(waiting.kind, RefusalKind::Undeclared);
		assert_eq!(waiting.reason, "needs declaring");
		assert_eq!(
			first.refusal("MISMATCH.example.com").unwrap().kind,
			RefusalKind::Other
		);
		assert!(first.refusal("fine.example.com").is_none());
	}

	/// No record is a record of nothing refused.
	#[tokio::test]
	async fn no_record_is_nothing_refused() {
		let dir = tempfile::tempdir().unwrap();
		let cache = SweepCache::new().with_store_dir(dir.path().to_path_buf());
		assert_eq!(*cache.refusals().await.unwrap(), RefusalRecord::default());
	}

	/// A record that cannot be read is an answer the checks branch on, not one
	/// taken for nothing refused.
	#[tokio::test]
	async fn an_unreadable_record_is_an_error_value() {
		let dir = tempfile::tempdir().unwrap();
		tokio::fs::write(
			dir.path().join("canopy-certificate-refusals.json"),
			b"not json",
		)
		.await
		.unwrap();
		let cache = SweepCache::new().with_store_dir(dir.path().to_path_buf());
		let err = cache.refusals().await.unwrap_err();
		assert_eq!(cache.refusals().await.unwrap_err(), err);
	}
}
