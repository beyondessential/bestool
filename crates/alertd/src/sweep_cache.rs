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
	time::Duration,
};

use bestool_canopy::{
	CanopyClient,
	names::{Entitlement, Refusal, RefusalKind},
};
use serde_json::Value;
use tokio::sync::OnceCell;
use tracing::debug;

use crate::{
	checks::fmt_chain,
	local_http,
	ownership::{CaddySites, HostApplication, Ownership},
	runtime::caddy,
};

/// Where the running daemon listens, in the order a client tries. The daemon
/// binds every loopback address it can, but on a host with only one family it
/// ends up on just that one. Kept in step with `DAEMON_BASES` in bestool.
const DAEMON_BASES: [&str; 2] = ["http://[::1]:8271", "http://127.0.0.1:8271"];

/// The daemon's open status endpoint for the task that collects chains.
const DAEMON_STATUS_PATH: &str = "/tasks/canopy-names/status";

const DAEMON_TIMEOUT: Duration = Duration::from_secs(3);

/// What the daemon last heard from Canopy about each DNS name it ordered for.
///
/// spec: CHK-CCO#which-dns-names-it-grades
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DaemonStatus {
	/// The DNS names whose last request Canopy refused, lower-cased.
	pub refusals: BTreeMap<String, Refusal>,
}

impl DaemonStatus {
	/// Read the refusals out of the status endpoint's answer.
	///
	/// A row with no `refusal` is an order that is fine, or failing for a reason
	/// that is not a refusal, and says nothing here.
	pub fn from_json(answer: &Value) -> Self {
		let refusals = answer["orders"]
			.as_array()
			.into_iter()
			.flatten()
			.filter_map(|row| {
				let name = row["name"].as_str()?.trim().to_ascii_lowercase();
				let kind = match row["refusal"].as_str()? {
					"undeclared" => RefusalKind::Undeclared,
					"denied" => RefusalKind::Denied,
					_ => RefusalKind::Other,
				};
				let reason = row["reason"]
					.as_str()
					.or_else(|| row["lastError"].as_str())
					.unwrap_or("canopy gave no reason")
					.to_owned();
				Some((name, Refusal { kind, reason }))
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
	/// Where to look for the daemon; the loopback addresses it listens on when
	/// empty.
	daemon_bases: Vec<String>,
	/// The applications on this machine that Caddy can be fronting, which is what
	/// a DNS name's site is attributed against.
	host_applications: Vec<HostApplication>,
	/// Caddy's live admin configuration, or `None` where its admin API could not
	/// be read.
	caddy_config: OnceCell<Option<Arc<Value>>>,
	/// The sites in that configuration, parsed once.
	caddy_sites: OnceCell<Option<Arc<CaddySites>>>,
	/// Which application each DNS name belongs to.
	ownership: OnceCell<Option<Arc<Ownership>>>,
	/// What the daemon last heard from Canopy about each DNS name, or why the
	/// daemon could not be asked.
	daemon_status: OnceCell<Result<Arc<DaemonStatus>, String>>,
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

	/// Ask the daemon at these base URLs instead of the loopback addresses it
	/// listens on.
	#[cfg(test)]
	fn with_daemon_bases(mut self, bases: Vec<String>) -> Self {
		self.daemon_bases = bases;
		self
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
		Some(Arc::new(self.caddy_sites().await?.hooked_addresses()))
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

	/// What the running daemon last heard from Canopy about each DNS name, or why
	/// it could not be asked.
	///
	/// spec: CHK-CCO#which-dns-names-it-grades
	pub async fn daemon_status(&self) -> Result<Arc<DaemonStatus>, String> {
		self.daemon_status
			.get_or_init(|| async { self.fetch_daemon_status().await.map(Arc::new) })
			.await
			.clone()
	}

	async fn fetch_daemon_status(&self) -> Result<DaemonStatus, String> {
		let defaults = DAEMON_BASES.map(String::from);
		let bases = if self.daemon_bases.is_empty() {
			&defaults[..]
		} else {
			&self.daemon_bases[..]
		};

		let mut last_err = String::new();
		for base in bases {
			let response = match local_http::client()
				.get(format!("{base}{DAEMON_STATUS_PATH}"))
				.timeout(DAEMON_TIMEOUT)
				.send()
				.await
			{
				Ok(response) => response,
				Err(err) => {
					last_err = fmt_chain(&err);
					continue;
				}
			};
			if !response.status().is_success() {
				return Err(format!("the daemon answered {}", response.status()));
			}
			return response
				.json::<Value>()
				.await
				.map(|answer| DaemonStatus::from_json(&answer))
				.map_err(|err| format!("the daemon's answer did not parse: {}", fmt_chain(&err)));
		}
		Err(format!("no daemon answered: {last_err}"))
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
		assert!(cache.caddy_subjects().await.is_some());
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

	/// An HTTP server that answers every request with `body`, counting them.
	async fn stub(status: &'static str, body: &'static str) -> (String, Arc<AtomicUsize>) {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let base = format!("http://{}", listener.local_addr().unwrap());
		let hits = Arc::new(AtomicUsize::new(0));
		let counted = hits.clone();
		tokio::spawn(async move {
			while let Ok((mut stream, _)) = listener.accept().await {
				counted.fetch_add(1, Ordering::SeqCst);
				let mut scratch = [0u8; 1024];
				let _ = stream.read(&mut scratch).await;
				let _ = stream
					.write_all(
						format!(
							"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
							body.len()
						)
						.as_bytes(),
					)
					.await;
				let _ = stream.shutdown().await;
			}
		});
		(base, hits)
	}

	async fn dead_base() -> String {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		format!("http://{}", listener.local_addr().unwrap())
	}

	const STATUS: &str = r#"{"orders":[
		{"name":"Wait.example.com","state":"refused","refusal":"undeclared","reason":"needs declaring","lastError":"403"},
		{"name":"no.example.com","state":"refused","refusal":"denied","reason":null,"lastError":"denied by an operator"},
		{"name":"mismatch.example.com","state":"refused","refusal":"other","reason":"this machine hosts tamanu-central"},
		{"name":"fine.example.com","state":"issued","refusal":null,"reason":null,"lastError":null}
	]}"#;

	/// The daemon is asked once however many applications' runs want the
	/// answer, and what it says is read as typed refusals.
	#[tokio::test]
	async fn the_daemons_status_is_fetched_once_for_the_sweep() {
		let (base, hits) = stub("200 OK", STATUS).await;
		let cache = SweepCache::new().with_daemon_bases(vec![base]);

		let first = cache.daemon_status().await.unwrap();
		let second = cache.daemon_status().await.unwrap();
		assert!(Arc::ptr_eq(&first, &second));
		assert_eq!(hits.load(Ordering::SeqCst), 1);

		let waiting = first.refusal("wait.example.com").unwrap();
		assert_eq!(waiting.kind, RefusalKind::Undeclared);
		assert_eq!(waiting.reason, "needs declaring");
		assert_eq!(
			first.refusal("NO.example.com").unwrap().reason,
			"denied by an operator"
		);
		assert_eq!(
			first.refusal("mismatch.example.com").unwrap().kind,
			RefusalKind::Other
		);
		assert!(first.refusal("fine.example.com").is_none());
	}

	/// A daemon that cannot be reached is an answer the checks branch on, and it
	/// is not asked again within the sweep.
	#[tokio::test]
	async fn an_unreachable_daemon_is_an_error_value() {
		let cache = SweepCache::new().with_daemon_bases(vec![dead_base().await]);
		let err = cache.daemon_status().await.unwrap_err();
		assert!(err.contains("no daemon answered"), "{err}");
		assert_eq!(cache.daemon_status().await.unwrap_err(), err);
	}

	#[tokio::test]
	async fn a_daemon_answering_with_an_error_is_an_error_value() {
		let (base, _) = stub("500 Internal Server Error", "{}").await;
		let cache = SweepCache::new().with_daemon_bases(vec![base]);
		let err = cache.daemon_status().await.unwrap_err();
		assert!(err.contains("500"), "{err}");
	}

	/// The first base that connects answers, so a host whose daemon is on only
	/// one address family is still reached.
	#[tokio::test]
	async fn the_next_base_is_tried_when_one_does_not_connect() {
		let (live, hits) = stub("200 OK", STATUS).await;
		let cache = SweepCache::new().with_daemon_bases(vec![dead_base().await, live]);
		assert_eq!(cache.daemon_status().await.unwrap().refusals.len(), 3);
		assert_eq!(hits.load(Ordering::SeqCst), 1);
	}
}
