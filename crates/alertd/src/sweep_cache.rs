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
	sync::Arc,
};

use bestool_canopy::{CanopyClient, names::Entitlement};
use serde_json::Value;
use tokio::sync::OnceCell;
use tracing::debug;

use crate::{checks::fmt_chain, runtime::caddy};

/// One sweep's shared readings. Cheap to build; nothing is asked for until it is
/// wanted.
#[derive(Default)]
pub struct SweepCache {
	/// Caddy's live admin configuration, or `None` where its admin API could not
	/// be read.
	caddy_config: OnceCell<Option<Arc<Value>>>,
	/// The names Caddy is configured to serve, derived from the configuration
	/// above.
	caddy_subjects: OnceCell<Option<Arc<BTreeSet<String>>>>,
	/// The chains the daemon has collected from Canopy, read from disk, keyed by
	/// the name each covers.
	canopy_chains: OnceCell<Result<Arc<BTreeMap<String, String>>, String>>,
	/// Those chains parsed, which is the form the certificate substrate wants.
	/// Held apart from the read above because the two consumers want different
	/// shapes of the same file, and neither should pay for the other's.
	parsed_chains: OnceCell<Arc<Vec<(String, caddy::DiskCert)>>>,
	/// What Canopy last said this machine may do.
	entitlement: OnceCell<Result<Entitlement, String>>,
}

impl SweepCache {
	pub fn new() -> Self {
		Self::default()
	}

	pub async fn caddy_config(&self, http: &reqwest::Client) -> Option<Arc<Value>> {
		self.caddy_config
			.get_or_init(|| async { caddy::fetch_admin_config(http).await.map(Arc::new) })
			.await
			.clone()
	}

	/// The site addresses Caddy is configured to serve.
	///
	/// `None` where the configuration could not be read at all, which is how a
	/// host not running Caddy is passed over — as having no list rather than an
	/// empty one. More than the certificate reading needs this: which names the
	/// host answers on is also what says which names a collection owes a chain
	/// for.
	///
	/// spec: CHK-CCO#which-names-it-grades
	pub async fn caddy_subjects(&self, http: &reqwest::Client) -> Option<Arc<BTreeSet<String>>> {
		self.caddy_subjects
			.get_or_init(|| async {
				let config = self.caddy_config(http).await?;
				Some(Arc::new(caddy::active_subjects(&config)))
			})
			.await
			.clone()
	}

	/// The collected chains, or why they could not be read.
	pub async fn canopy_chains(&self) -> Result<Arc<BTreeMap<String, String>>, String> {
		self.canopy_chains
			.get_or_init(|| async {
				let dir = bestool_canopy::certificates::default_dir();
				bestool_canopy::certificates::load_chains(&dir)
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
			const BODY: &str = r#"{"apps":{"http":{"servers":{"s":{"routes":[{"match":[{"host":["app.example.com"]}]}]}}}}}"#;
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
		let http = reqwest::Client::new();
		assert!(cache.caddy_config(&http).await.is_some());
		let subjects = cache.caddy_subjects(&http).await.unwrap();
		assert!(subjects.contains("app.example.com"));
		assert!(cache.caddy_subjects(&http).await.is_some());
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
}
