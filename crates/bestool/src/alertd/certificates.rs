//! Obtaining TLS certificates from canopy, and serving them to Caddy.
//!
//! Canopy holds the account with the authority and proves control of a name
//! through DNS, so this host needs no DNS credential of its own. What it owns
//! is the key, the collecting, and the serving:
//!
//! - a P-256 key per name, generated here and never sent anywhere
//!   ([`bestool_canopy::certificates`]);
//! - a collection loop on a timer, because proving control through DNS takes
//!   far longer than a client waits;
//! - a certificate endpoint Caddy asks during the handshake, answered entirely
//!   from memory ([`delivery`]).
//!
//! Canopy takes precedence over Caddy's own issuance rather than replacing it. A
//! name this holds a chain for is served from canopy; a name it does not is
//! declined, and Caddy issues for it exactly as it would were this not
//! configured at all. That is what lets a host run this before its DNS
//! credential is withdrawn.
//!
//! spec: TLS
//! spec: TLSD

use std::{
	collections::{BTreeMap, BTreeSet},
	path::PathBuf,
	sync::Arc,
	time::Duration,
};

use bestool_canopy::{
	certificates::{self as certs, KeyPair, KeyStore},
	names::Entitlement,
	schema::{RegisterNameArgs, RegisteredName, RequestCertificateArgs},
};
use futures::future::BoxFuture;
use jiff::Timestamp;
use miette::{IntoDiagnostic as _, Result, WrapErr as _, miette};
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::alertd::{
	BackgroundTask, TaskContext, TaskEndpoint, TaskEndpointResponse, tasks::TaskEndpointHandler,
};

/// A report's whole cause chain, joined.
///
/// A bare `Display` gives only the outermost wrap — "asking canopy what this
/// server may do" — which says what was being done and nothing about what went
/// wrong. What an operator needs is the cause underneath it.
fn why(err: &miette::Report) -> String {
	err.chain()
		.map(ToString::to_string)
		.collect::<Vec<_>>()
		.join(": ")
}

pub mod delivery;
pub mod peer;

/// The name the daemon mounts this task's endpoints under.
pub const TASK_NAME: &str = "canopy-names";

/// How often the task wakes.
///
/// The daemon ticks a task at one fixed interval, so both rhythms this needs
/// come from inside: it wakes at this rate and rate-limits its own steady-state
/// work. That sits better with the watchdog, which counts each tick as activity,
/// than doing its own waiting within a run.
const TICK: Duration = Duration::from_secs(60);

/// How often a pass is made when nothing is waiting on one.
///
/// A canopy-issued chain's lifetime is not known before one arrives, so this
/// suits the shortest profile the authority might use rather than any particular
/// one: a six-day chain renewed at a third of its life still gets two days of
/// passes before it matters.
const STEADY_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// The most names a handshake may leave waiting for a pass.
///
/// `wanted` is a backstop between passes, not a discovery route: what a pass
/// acts on comes from Caddy's configuration, which this only anticipates. A site
/// configured for a wildcard makes the name remote input, because Caddy passes
/// the client's own server name through — so the set is bounded, and past the
/// bound a new name is dropped rather than an old one evicted. Evicting would
/// let a stream of invented names push out the one a real client asked for.
const WANTED_LIMIT: usize = 64;

/// How soon a name whose order is pending is asked about again.
///
/// Sooner than the steady interval, so an order in flight is collected promptly
/// rather than waiting one out; not every tick, so a name that stays pending for
/// an hour is not asked sixty times.
const PENDING_RETRY: Duration = Duration::from_secs(120);

/// What canopy said about one name, the last time it was asked.
#[derive(Clone, Debug, Default)]
pub struct Order {
	/// `pending`, `issued`, `failed`, or `revoked`.
	pub state: String,
	/// Why the last attempt failed, while canopy is still retrying. Surfaced
	/// rather than retried into.
	pub last_error: Option<String>,
	pub not_after: Option<Timestamp>,
	pub revoked: bool,
	pub key_must_be_replaced: bool,
	pub asked_at: Option<Timestamp>,
}

impl Order {
	fn pending(&self) -> bool {
		self.state == "pending"
	}
}

/// A chain this host holds and can serve, with the key it covers.
///
/// Held in memory because the endpoint answering with it sits on the TLS
/// handshake path: it reaches no network, opens no key store and reads no file.
#[derive(Clone)]
pub struct Held {
	pub chain: String,
	pub key_pem: String,
	pub not_after: Option<Timestamp>,
	/// Whether the chain can be served: canopy reports it neither revoked nor
	/// past its expiry.
	pub usable: bool,
	pub revoked: bool,
}

impl std::fmt::Debug for Held {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Held")
			.field("not_after", &self.not_after)
			.field("usable", &self.usable)
			.field("revoked", &self.revoked)
			.finish_non_exhaustive()
	}
}

/// Everything the collection loop and the delivery endpoint share.
pub struct CertificateState {
	/// Where the key store and the collected chains live.
	dir: PathBuf,
	/// Who may fetch a certificate, beyond the superuser.
	permitted: peer::Permitted,

	/// What can be served right now. Read on the handshake path, so nothing
	/// here is loaded lazily.
	held: RwLock<BTreeMap<String, Held>>,
	/// What canopy last said this server may do. `None` until the first ask.
	entitlement: RwLock<Option<Entitlement>>,
	/// What canopy last said about each name asked about.
	orders: RwLock<BTreeMap<String, Order>>,
	/// Names a handshake or a command asked for that nothing is held for, so an
	/// order follows. A backstop between passes rather than a discovery route:
	/// Caddy only asks for names it is configured to serve, which a pass reads
	/// from the same configuration.
	wanted: Mutex<BTreeSet<String>>,

	/// The keys, held across passes so scrypt is not re-run every tick. Loaded
	/// on the first pass and written through on change.
	keys: Mutex<Option<KeyStore>>,
	/// When the last steady-state pass ran.
	last_pass: RwLock<Option<Timestamp>>,
	/// When a pass last stood down, where the last one did.
	///
	/// A handshake records a name whatever this server may do, and a server
	/// standing down holds no chains — so every handshake it declines records
	/// one. Without remembering the stand-down, those names would wake a pass,
	/// and an entitlement request with it, on every tick until the grant
	/// returns.
	stood_down: RwLock<Option<Timestamp>>,
	/// What went wrong on the last pass, where something did.
	last_error: RwLock<Option<String>>,
}

impl CertificateState {
	pub fn new(dir: PathBuf, permitted: peer::Permitted) -> Self {
		Self {
			dir,
			permitted,
			held: RwLock::new(BTreeMap::new()),
			entitlement: RwLock::new(None),
			orders: RwLock::new(BTreeMap::new()),
			wanted: Mutex::new(BTreeSet::new()),
			keys: Mutex::new(None),
			last_pass: RwLock::new(None),
			stood_down: RwLock::new(None),
			last_error: RwLock::new(None),
		}
	}

	pub fn permitted(&self) -> peer::Permitted {
		self.permitted
	}

	/// The chain and key to serve for `name`, or `None` to decline.
	///
	/// Declines for a name outside the domains the group controls, a name with
	/// nothing collected yet, and a chain that is no longer usable because it
	/// has been revoked or has expired. Every one of those hands the handshake
	/// back to Caddy, which issues for the name itself.
	///
	/// spec: TLSD#declining-and-failing
	pub async fn serve(&self, name: &str) -> Option<(String, String)> {
		let name = name.trim_end_matches('.').to_ascii_lowercase();

		// What the endpoint serves turns on the chain in hand rather than on
		// what the server may currently ask for: a withdrawn grant and a pause
		// both stop new requests without taking a collected chain out of
		// service. The domain test is separate — a name outside the group's
		// domains was never this server's to serve.
		if let Some(entitlement) = self.entitlement.read().await.as_ref()
			&& !entitlement.covers(&name)
		{
			return None;
		}

		let held = self.held.read().await;
		let chain = held.get(&name)?;
		// `usable` is what canopy last said, or what the chain's own dates said
		// when it was loaded; neither is re-evaluated as time passes. So the
		// expiry is tested here too: where collection has stopped — canopy
		// unreachable, or a pass standing down — the cached flag would otherwise
		// have the daemon hand Caddy an expired chain indefinitely.
		let live = chain.usable && chain.not_after.is_none_or(|at| at > Timestamp::now());
		live.then(|| (chain.chain.clone(), chain.key_pem.clone()))
	}

	/// Record a name a handshake asked for and nothing was held for, so an order
	/// follows on the next pass.
	///
	/// Subject to the same entitlement test as a name read from Caddy's
	/// configuration, so a handshake cannot conjure an order for a name outside
	/// the server's reach.
	///
	/// spec: TLS#which-names-are-certified
	pub async fn note_wanted(&self, name: &str) {
		let name = name.trim_end_matches('.').to_ascii_lowercase();
		if certs::plausible_name(&name).is_err() {
			return;
		}
		if self.held.read().await.contains_key(&name) {
			return;
		}

		let mut wanted = self.wanted.lock().await;
		if wanted.len() >= WANTED_LIMIT && !wanted.contains(&name) {
			debug!(
				name,
				"not recording a name asked for during a handshake; too many are already waiting"
			);
			return;
		}
		if wanted.insert(name.clone()) {
			debug!(name, "recorded a name asked for during a handshake");
		}
	}

	/// Load what this host already holds, so a restart serves its chains at once
	/// and collects an order already placed rather than placing a new one.
	///
	/// spec: TLS#keys
	async fn load_from_disk(&self) -> Result<()> {
		let mut keys = self.keys.lock().await;
		if keys.is_some() {
			return Ok(());
		}

		let store = certs::load_keys(&self.dir).await?;
		let chains = certs::load_chains(&self.dir).await?;

		let mut held = self.held.write().await;
		for (name, chain) in chains {
			let Some(key_pem) = store.key_pem(&name) else {
				// A chain whose key is gone cannot be served: the key is half of
				// what the endpoint answers with, and canopy keys what it holds
				// by name and key, so this one is asked for afresh.
				debug!(name, "holding a chain with no key; it will be re-ordered");
				continue;
			};
			let not_after =
				certs::chain_not_after(&chain).and_then(|secs| Timestamp::from_second(secs).ok());
			held.insert(
				name,
				Held {
					chain,
					key_pem: key_pem.to_owned(),
					not_after,
					// Nothing canopy has said yet this run, so the chain's own
					// dates are what says whether it can be served. Revocation
					// arrives from canopy and takes it out of service then.
					usable: not_after.is_none_or(|at| at > Timestamp::now()),
					revoked: false,
				},
			);
		}
		info!(names = held.len(), "loaded the chains this host holds");

		*keys = Some(store);
		Ok(())
	}

	/// Whether a pass is due: an order in flight, a name asked for and not held,
	/// or the steady interval having come round.
	async fn pass_due(&self) -> bool {
		let now = Timestamp::now();
		let steady_due = match *self.last_pass.read().await {
			None => true,
			Some(last) => (now - last).get_seconds() >= STEADY_INTERVAL.as_secs() as i64,
		};
		if steady_due {
			return true;
		}

		// A server standing down is answered the same way until the steady
		// interval comes round again, so nothing short of that wakes a pass.
		if let Some(at) = *self.stood_down.read().await
			&& (now - at).get_seconds() < STEADY_INTERVAL.as_secs() as i64
		{
			return false;
		}

		// Each name asked for is tested rather than the set merely being
		// non-empty: a name whose last attempt failed is still asked for, and
		// waking a full pass — which asks canopy what this server may do before
		// it does anything else — on every tick until it succeeds is what the
		// per-name backoff exists to stop.
		let wanted: Vec<String> = self.wanted.lock().await.iter().cloned().collect();
		for name in &wanted {
			if self.name_due(name, now).await {
				return true;
			}
		}

		self.orders.read().await.values().any(|order| {
			order.pending()
				&& order
					.asked_at
					.is_none_or(|at| (now - at).get_seconds() >= PENDING_RETRY.as_secs() as i64)
		})
	}

	/// Ask canopy what this server may do, and keep the answer.
	async fn refresh_entitlement(&self, ctx: &TaskContext) -> Result<Entitlement> {
		let client = ctx
			.canopy_client
			.as_ref()
			.ok_or_else(|| miette!("no canopy client; this host is not enrolled"))?;
		let wire = client
			.names_entitlements()
			.await
			.into_diagnostic()
			.wrap_err("asking canopy what this server may do")?;
		let entitlement = Entitlement::from_wire(&wire);
		*self.entitlement.write().await = Some(entitlement.clone());
		Ok(entitlement)
	}

	/// The entitlement as last answered, asking canopy where none has been.
	async fn entitlement(&self, ctx: &TaskContext) -> Result<Entitlement> {
		if let Some(held) = self.entitlement.read().await.clone() {
			return Ok(held);
		}
		self.refresh_entitlement(ctx).await
	}

	/// One collection pass: what may be certified, asked about and collected.
	///
	/// spec: TLS#requesting-and-collecting
	async fn pass(&self, ctx: &TaskContext, steady: bool) -> Result<()> {
		self.load_from_disk().await?;
		let entitlement = self.refresh_entitlement(ctx).await?;

		if let Some(reason) = stands_down(&entitlement) {
			debug!(reason, "not requesting certificates");
			self.stand_down().await;
			return Ok(());
		}
		*self.stood_down.write().await = None;

		let names = self.target_names(ctx, &entitlement).await;

		self.prune_orders(&names).await;

		if names.is_empty() {
			debug!("no name to certify on this host");
			return Ok(());
		}

		let now = Timestamp::now();
		for name in names {
			let due = steady || self.name_due(&name, now).await;
			if !due {
				continue;
			}
			match self.collect_one(ctx, &name).await {
				Ok(()) => {
					self.wanted.lock().await.remove(&name);
				}
				Err(err) => {
					// One name failing must not stop the rest: a stuck order on
					// one site is not a reason to leave every other name
					// uncollected.
					warn!(name, %err, "could not collect a certificate");
					let mut orders = self.orders.write().await;
					let order = orders.entry(name.clone()).or_default();
					order.last_error = Some(why(&err));
					// Stamped although canopy recorded nothing, so a failed
					// attempt backs off exactly as a pending order does. The
					// name stays asked-for: dropping it on a transient failure
					// would leave a name Caddy is serving waiting out the steady
					// interval for its first chain.
					order.asked_at = Some(now);
				}
			}
		}
		Ok(())
	}

	/// Record a pass that stood down.
	///
	/// Nothing will be ordered for while this stands, and the names a handshake
	/// left behind are the reason a pass was due at all — so they go, and the
	/// stand-down is remembered so the ones arriving next do not wake another.
	/// The grant and the pause are canopy's to lift; asking again before the
	/// steady interval only earns the same refusal.
	///
	/// spec: TLS#when-the-grant-is-absent-or-the-server-is-paused
	async fn stand_down(&self) {
		self.wanted.lock().await.clear();
		*self.stood_down.write().await = Some(Timestamp::now());
	}

	/// Drop what canopy said about names no longer in play.
	///
	/// A name reaches the target list from Caddy's configuration or from a
	/// handshake, and one that has left both is not going to be asked about
	/// again. Without this a host serving a wildcard keeps an entry for every
	/// name any client ever offered.
	async fn prune_orders(&self, targets: &[String]) {
		let held = self.held.read().await;
		let targets: BTreeSet<&String> = targets.iter().collect();
		self.orders
			.write()
			.await
			.retain(|name, _| targets.contains(name) || held.contains_key(name));
	}

	/// Whether one name is due to be asked about outside a steady pass.
	///
	/// A name nothing has been attempted for is due at once, which is what makes
	/// a handshake's ask prompt. Once an attempt has been made — whether canopy
	/// recorded an order or the attempt failed outright — the retry backoff
	/// governs, so neither a pending order nor a failing one is asked about every
	/// tick.
	async fn name_due(&self, name: &str, now: Timestamp) -> bool {
		let wanted = self.wanted.lock().await.contains(name);
		match self.orders.read().await.get(name) {
			None => true,
			Some(order) if wanted || order.pending() => order
				.asked_at
				.is_none_or(|at| (now - at).get_seconds() >= PENDING_RETRY.as_secs() as i64),
			Some(_) => false,
		}
	}

	/// The names to certify: a site address Caddy serves, or one a handshake
	/// asked for, that the entitlement covers.
	///
	/// A name meeting one test and not the other is left to Caddy's own
	/// issuance.
	///
	/// spec: TLS#which-names-are-certified
	async fn target_names(&self, ctx: &TaskContext, entitlement: &Entitlement) -> Vec<String> {
		let mut names: BTreeSet<String> = delivery::caddy_subjects(&ctx.http_client)
			.await
			.unwrap_or_else(|err| {
				debug!(%err, "could not read Caddy's active subjects");
				BTreeSet::new()
			});
		// A name is recorded during a handshake before any entitlement has been
		// asked for, so the test is applied here rather than there — and a name
		// this entitlement will never order for is dropped from the record as
		// well as from the list. Left there it would keep `pass_due` true on
		// every tick, which spends a full entitlement request a minute on a name
		// that can never be ordered for.
		{
			let mut wanted = self.wanted.lock().await;
			wanted.retain(|name| entitlement.may_certify(name));
			names.extend(wanted.iter().cloned());
		}

		names
			.into_iter()
			.filter(|name| certs::plausible_name(name).is_ok() && entitlement.may_certify(name))
			.collect()
	}

	/// Ask canopy for one name, and take what comes back.
	///
	/// Request and collect are the same call and it is safe to repeat: a name
	/// and key canopy already holds a certificate for is answered from what it
	/// holds rather than ordered again.
	async fn collect_one(&self, ctx: &TaskContext, name: &str) -> Result<()> {
		let client = ctx
			.canopy_client
			.as_ref()
			.ok_or_else(|| miette!("no canopy client; this host is not enrolled"))?;

		let key = self.key_for(name).await?;
		let csr = certs::signing_request(name, &key)?;
		let answer = client
			.certificates_request(
				&RequestCertificateArgs::builder()
					.csr(csr)
					.name(name.to_owned())
					.build(),
			)
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("asking canopy to certify {name}"))?;

		let not_after = answer.not_after.as_deref().and_then(certs::parse_not_after);
		let order = Order {
			state: answer.state.clone(),
			last_error: answer.last_error.clone(),
			not_after,
			revoked: answer.revoked,
			key_must_be_replaced: answer.key_must_be_replaced,
			asked_at: Some(Timestamp::now()),
		};
		if let Some(ref err) = order.last_error {
			warn!(name, error = %err, state = %order.state, "canopy reports this order failing");
		}
		self.orders
			.write()
			.await
			.insert(name.to_owned(), order.clone());

		match disposition(&order, answer.chain.as_deref()) {
			// A key canopy condemns is replaced before the next request, rather
			// than asked against again. It is never certified again for any
			// name, so replacing it is the only way forward and no operator has
			// to act.
			//
			// spec: TLS#revocation-and-key-replacement
			Disposition::ReplaceKey => {
				info!(name, "canopy condemned this key; generating a replacement");
				self.replace_key(name).await?;
			}
			// A certificate canopy reports as revoked stops being served at
			// once. A replacement is requested under the ordinary schedule:
			// revoking pauses the server, so asking now would only be refused,
			// and the refusal is an operator deciding when this host may have a
			// certificate again.
			Disposition::DropHeld => {
				info!(
					name,
					"canopy reports this certificate revoked; taking it out of service"
				);
				self.drop_held(name).await?;
			}
			Disposition::TakeChain => {
				let chain = answer.chain.unwrap_or_default();
				self.take_chain(name, chain, not_after, answer.usable)
					.await?;
			}
			Disposition::Wait => debug!(name, state = %answer.state, "no chain yet"),
		}
		Ok(())
	}

	/// The key for `name`, generating and storing one where there is none.
	async fn key_for(&self, name: &str) -> Result<KeyPair> {
		let mut keys = self.keys.lock().await;
		let store = keys.get_or_insert_with(KeyStore::default);
		if let Some(key) = store.key(name) {
			return key;
		}

		let key = certs::generate_key()?;
		store.replace(name, &key);
		certs::store_keys(&self.dir, store).await?;
		info!(name, "generated a key for this name");
		Ok(key)
	}

	/// Replace the key held for `name`, and drop what it covered.
	async fn replace_key(&self, name: &str) -> Result<()> {
		let key = certs::generate_key()?;
		{
			let mut keys = self.keys.lock().await;
			let store = keys.get_or_insert_with(KeyStore::default);
			store.replace(name, &key);
			certs::store_keys(&self.dir, store).await?;
		}
		// The chain covered the condemned key, so it goes with it.
		self.drop_held(name).await
	}

	/// Take a collected chain into service, and keep it across a restart.
	async fn take_chain(
		&self,
		name: &str,
		chain: String,
		not_after: Option<Timestamp>,
		usable: bool,
	) -> Result<()> {
		let key_pem = {
			let keys = self.keys.lock().await;
			keys.as_ref()
				.and_then(|store| store.key_pem(name).map(ToOwned::to_owned))
				.ok_or_else(|| miette!("collected a chain for {name} with no key to serve it"))?
		};

		let replacing = self
			.held
			.read()
			.await
			.get(name)
			.is_some_and(|held| held.chain != chain);
		certs::store_chain(&self.dir, name, &chain).await?;
		self.held.write().await.insert(
			name.to_owned(),
			Held {
				chain,
				key_pem,
				not_after,
				usable,
				revoked: false,
			},
		);
		if replacing {
			info!(name, "a renewed chain replaced the one held");
		} else {
			info!(name, "collected a chain");
		}
		Ok(())
	}

	/// Take a name's chain out of service and off the disk.
	async fn drop_held(&self, name: &str) -> Result<()> {
		self.held.write().await.remove(name);
		certs::forget_chain(&self.dir, name).await?;
		Ok(())
	}

	/// What this task holds, for the status endpoint and the CLI.
	async fn report(&self) -> Value {
		let entitlement = self.entitlement.read().await.clone();
		let held = self.held.read().await;
		let orders = self.orders.read().await;

		let names: Vec<Value> = held
			.iter()
			.map(|(name, chain)| {
				json!({
					"name": name,
					"notAfter": chain.not_after.map(|at| at.to_string()),
					"usable": chain.usable,
					"revoked": chain.revoked,
				})
			})
			.collect();

		let orders: Vec<Value> = orders
			.iter()
			.map(|(name, order)| {
				json!({
					"name": name,
					"state": order.state,
					"lastError": order.last_error,
					"notAfter": order.not_after.map(|at| at.to_string()),
					"revoked": order.revoked,
					"keyMustBeReplaced": order.key_must_be_replaced,
					"askedAt": order.asked_at.map(|at| at.to_string()),
				})
			})
			.collect();

		json!({
			"entitled": entitlement.as_ref().is_some_and(Entitlement::holds_tls_grant),
			"dnsEntitled": entitlement.as_ref().is_some_and(Entitlement::holds_dns_grant),
			"paused": entitlement.as_ref().is_some_and(Entitlement::fully_paused),
			"domains": entitlement.as_ref().map(Entitlement::domains).unwrap_or_default(),
			"registeredNames": entitlement
				.as_ref()
				.map(Entitlement::registered_names)
				.unwrap_or_default(),
			"canopyHolds": entitlement.as_ref().map(|e| e
				.certificates()
				.iter()
				.map(|held| json!({
					"name": held.name,
					"keyFingerprint": held.key_fingerprint,
					"notAfter": held.not_after,
					"usable": held.usable,
					"revoked": held.revoked,
					"keyMustBeReplaced": held.key_must_be_replaced,
				}))
				.collect::<Vec<_>>())
				.unwrap_or_default(),
			"held": names,
			"orders": orders,
			"wanted": self.wanted.lock().await.iter().cloned().collect::<Vec<_>>(),
			"lastPass": self.last_pass.read().await.map(|at| at.to_string()),
			"lastError": self.last_error.read().await.clone(),
		})
	}

	/// Publish the addresses a name resolves to, or withdraw it with none.
	///
	/// spec: NAM#registering-addresses-for-a-name
	async fn register_name(
		&self,
		ctx: &TaskContext,
		name: &str,
		addresses: Vec<String>,
	) -> Result<RegisteredName> {
		certs::plausible_name(name)?;
		// The daemon is the component holding the DNS grant, so it is where an
		// address is tested rather than at whichever client happened to call:
		// canopy publishes what it is given, and the CLI is not the only way
		// here. Parsed and re-rendered, so what is published is one canonical
		// spelling of the address rather than whatever form arrived.
		let addresses = addresses
			.iter()
			.map(|address| {
				address
					.parse::<std::net::IpAddr>()
					.map(|parsed| parsed.to_string())
					.map_err(|_| miette!("{address:?} is not an IP address"))
			})
			.collect::<Result<Vec<String>>>()?;

		let client = ctx
			.canopy_client
			.as_ref()
			.ok_or_else(|| miette!("no canopy client; this host is not enrolled"))?;

		let entitlement = self.entitlement(ctx).await?;
		if !entitlement.holds_dns_grant() {
			return Err(miette!("this server may not manage its own DNS records"));
		}
		if !entitlement.covers(name) {
			return Err(miette!(
				"{name} is not within a domain this server's group controls"
			));
		}

		let answer = client
			.names_register(
				&RegisterNameArgs::builder()
					.addresses(addresses)
					.name(name.to_owned())
					.build(),
			)
			.await
			.into_diagnostic()
			.wrap_err_with(|| format!("registering {name} with canopy"))?;

		// The registration changed what canopy holds for this server, so the
		// cached answer is stale.
		let _ = self.refresh_entitlement(ctx).await;
		Ok(answer)
	}
}

/// What one answer from canopy means for what this host holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposition {
	/// Replace the key, and take nothing from this answer.
	ReplaceKey,
	/// Take the certificate held for this name out of service.
	DropHeld,
	/// Take the chain this answer carries into service.
	TakeChain,
	/// Nothing to act on yet.
	Wait,
}

/// Read an answer for what to do with it.
///
/// The order is the point. A condemned key comes first and takes nothing else
/// from the answer: any chain alongside it covers the key being replaced, so
/// installing the two together would hand Caddy a chain and a key that do not
/// match — which fails the handshake outright, where declining would have let
/// Caddy issue for the name itself. The next pass requests against the new key.
///
/// spec: TLS#revocation-and-key-replacement
fn disposition(order: &Order, chain: Option<&str>) -> Disposition {
	if order.key_must_be_replaced {
		Disposition::ReplaceKey
	} else if order.revoked {
		Disposition::DropHeld
	} else if chain.is_some_and(|chain| !chain.trim().is_empty()) {
		Disposition::TakeChain
	} else {
		Disposition::Wait
	}
}

/// Why this server makes no requests, where it makes none.
///
/// A server that may not obtain certificates stops requesting them, and so does
/// one canopy reports as paused. Neither takes a collected chain out of service,
/// so what is held stays served either way: withdrawing a grant from a host
/// under suspicion stops it obtaining anything new without also dropping every
/// name it currently answers on.
///
/// A pause is canopy's to lift and no length of pause is escalated from here, so
/// a paused server waits rather than retrying against the refusal.
///
/// spec: TLS#when-the-grant-is-absent-or-the-server-is-paused
fn stands_down(entitlement: &Entitlement) -> Option<&'static str> {
	if !entitlement.holds_tls_grant() {
		Some("this server holds no TLS grant")
	} else if entitlement.fully_paused() {
		Some("canopy reports this server paused")
	} else {
		None
	}
}

/// The background task: the collection loop, and the endpoints the CLI reaches.
pub struct CanopyNames {
	state: Arc<CertificateState>,
}

impl CanopyNames {
	pub fn new(state: Arc<CertificateState>) -> Self {
		Self { state }
	}
}

impl BackgroundTask for CanopyNames {
	fn name(&self) -> &'static str {
		TASK_NAME
	}

	fn interval(&self) -> Duration {
		TICK
	}

	fn run<'a>(&'a self, ctx: &'a TaskContext) -> BoxFuture<'a, Result<()>> {
		Box::pin(async move {
			// Chains are loaded whatever else happens, so a restart serves what
			// it held even where canopy cannot be reached at all.
			if let Err(err) = self.state.load_from_disk().await {
				warn!(%err, "could not load the chains this host holds");
			}

			if !self.state.pass_due().await {
				return Ok(());
			}

			let steady = match *self.state.last_pass.read().await {
				None => true,
				Some(last) => {
					(Timestamp::now() - last).get_seconds() >= STEADY_INTERVAL.as_secs() as i64
				}
			};

			match self.state.pass(ctx, steady).await {
				Ok(()) => {
					*self.state.last_error.write().await = None;
				}
				Err(err) => {
					warn!(%err, "certificate collection pass failed");
					*self.state.last_error.write().await = Some(why(&err));
				}
			}
			if steady {
				*self.state.last_pass.write().await = Some(Timestamp::now());
			}
			Ok(())
		})
	}

	fn http_endpoints(&self) -> Vec<TaskEndpoint> {
		vec![
			endpoint("status", self.state.clone(), |state, _ctx| {
				Box::pin(async move { Ok(state.report().await) })
			}),
			guarded_endpoint("collect", self.state.clone(), |state, ctx| {
				Box::pin(async move {
					state.pass(&ctx, true).await?;
					*state.last_pass.write().await = Some(Timestamp::now());
					Ok(state.report().await)
				})
			}),
			guarded_endpoint("request", self.state.clone(), |state, ctx| {
				Box::pin(async move {
					let name = required(&ctx, "name")?;
					certs::plausible_name(&name)?;
					let name = name.to_ascii_lowercase();
					// A name outside the group's domains is refused rather than
					// recorded: a pass would drop it, and the operator needs to
					// be told that rather than handed a report that looks like
					// the ask was taken.
					if !state.entitlement(&ctx).await?.covers(&name) {
						return Err(miette!(
							"{name} is not within a domain this server's group controls"
						));
					}
					state.wanted.lock().await.insert(name);
					state.pass(&ctx, false).await?;
					Ok(state.report().await)
				})
			}),
			guarded_endpoint("dns-register", self.state.clone(), |state, ctx| {
				Box::pin(async move {
					let name = required(&ctx, "name")?;
					let addresses = ctx
						.query
						.get("addresses")
						.map(|list| {
							list.split(',')
								.map(str::trim)
								.filter(|a| !a.is_empty())
								.map(ToOwned::to_owned)
								.collect::<Vec<_>>()
						})
						.unwrap_or_default();
					// An empty list is what withdraws a name, so registering
					// with none would take the records down — and would look
					// like a publish that had worked. Withdrawing is its own
					// endpoint, asked for deliberately.
					if addresses.is_empty() {
						return Err(miette!(
							"registering {name} needs at least one address; use dns-withdraw to take its records down"
						));
					}
					let answer = state.register_name(&ctx, &name, addresses).await?;
					Ok(registered(&answer))
				})
			}),
			guarded_endpoint("dns-withdraw", self.state.clone(), |state, ctx| {
				Box::pin(async move {
					let name = required(&ctx, "name")?;
					// An empty address list is what withdraws a name: the
					// records come down and the name is freed.
					let answer = state.register_name(&ctx, &name, Vec::new()).await?;
					Ok(registered(&answer))
				})
			}),
		]
	}
}

fn registered(answer: &RegisteredName) -> Value {
	json!({
		"name": answer.name,
		"addresses": answer.addresses,
		"publishedAddresses": answer.published_addresses,
		"published": answer.published,
		"lastError": answer.last_error,
	})
}

fn required(ctx: &TaskContext, key: &str) -> Result<String> {
	ctx.query
		.get(key)
		.filter(|value| !value.is_empty())
		.cloned()
		.ok_or_else(|| miette!("this endpoint needs a `{key}` query parameter"))
}

/// Wire one endpoint, turning a failure into a 400 with its message.
///
/// A command reaching this is an operator asking for something; what went wrong
/// is what they need back, not a bare status.
fn endpoint(
	name: &'static str,
	state: Arc<CertificateState>,
	run: fn(Arc<CertificateState>, TaskContext) -> BoxFuture<'static, Result<Value>>,
) -> TaskEndpoint {
	TaskEndpoint::open(name, wrap(state, run))
}

/// [`endpoint`], for one that changes state: the superuser only, over POST.
///
/// Publishing or withdrawing a name changes what the world resolves for a
/// production deployment, and a request costs the authority an order — neither
/// is something any process that can reach loopback may ask for.
///
/// spec: NAM#commands
fn guarded_endpoint(
	name: &'static str,
	state: Arc<CertificateState>,
	run: fn(Arc<CertificateState>, TaskContext) -> BoxFuture<'static, Result<Value>>,
) -> TaskEndpoint {
	TaskEndpoint::guarded(name, wrap(state, run))
}

fn wrap(
	state: Arc<CertificateState>,
	run: fn(Arc<CertificateState>, TaskContext) -> BoxFuture<'static, Result<Value>>,
) -> TaskEndpointHandler {
	Arc::new(move |ctx: TaskContext| {
		let state = state.clone();
		Box::pin(async move {
			match run(state, ctx).await {
				Ok(value) => TaskEndpointResponse::Json(value),
				Err(err) => TaskEndpointResponse::Error {
					status: 400,
					message: why(&err),
				},
			}
		})
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	fn state() -> (tempfile::TempDir, CertificateState) {
		let dir = tempfile::tempdir().unwrap();
		let state = CertificateState::new(dir.path().to_path_buf(), peer::Permitted::default());
		(dir, state)
	}

	async fn with_entitlement(state: &CertificateState, domains: &[&str], tls: bool, paused: bool) {
		let wire = bestool_canopy::schema::Entitlements::builder()
			.applications(vec![])
			.certificates(vec![])
			.domains(domains.iter().map(|d| (*d).to_string()).collect::<Vec<_>>())
			.may_manage_dns(false)
			.may_manage_tls(tls)
			.paused(paused)
			.registered_names(vec![])
			.build();
		*state.entitlement.write().await = Some(Entitlement::from_wire(&wire));
	}

	/// A context with nothing behind it: no canopy, and an HTTP client whose
	/// caddy admin API is not running, so the subjects it reads are empty.
	fn detached_ctx() -> TaskContext {
		TaskContext {
			http_client: reqwest::Client::new(),
			canopy_client: None,
			reload: tokio::sync::watch::channel(0u64).1,
			#[cfg(windows)]
			restart: None,
			query: Default::default(),
		}
	}

	async fn hold(state: &CertificateState, name: &str, usable: bool) {
		hold_until(state, name, usable, None).await;
	}

	async fn hold_until(
		state: &CertificateState,
		name: &str,
		usable: bool,
		not_after: Option<Timestamp>,
	) {
		state.held.write().await.insert(
			name.to_owned(),
			Held {
				chain: format!("chain for {name}"),
				key_pem: format!("key for {name}"),
				not_after,
				usable,
				revoked: !usable,
			},
		);
	}

	#[tokio::test]
	async fn a_held_usable_chain_is_served_with_its_key() {
		let (_dir, state) = state();
		with_entitlement(&state, &["example.com"], true, false).await;
		hold(&state, "app.example.com", true).await;

		let (chain, key) = state.serve("app.example.com").await.unwrap();
		assert_eq!(chain, "chain for app.example.com");
		assert_eq!(key, "key for app.example.com");
		// The name as the client spelled it, trailing dot and all.
		assert!(state.serve("APP.example.com.").await.is_some());
	}

	#[tokio::test]
	async fn every_case_with_no_chain_to_offer_declines() {
		// A decline hands the handshake back to Caddy; an error would take down
		// a name Caddy would otherwise have covered. So each of these is a
		// decline, not a failure.
		let (_dir, state) = state();
		with_entitlement(&state, &["example.com"], true, false).await;

		// Nothing collected yet.
		assert!(state.serve("app.example.com").await.is_none());

		// Out of the domains the group controls.
		hold(&state, "app.elsewhere.test", true).await;
		assert!(state.serve("app.elsewhere.test").await.is_none());

		// Collected, then revoked or expired.
		hold(&state, "old.example.com", false).await;
		assert!(state.serve("old.example.com").await.is_none());
	}

	#[tokio::test]
	async fn a_held_chain_keeps_being_served_when_the_grant_goes_and_while_paused() {
		// Neither a withdrawn grant nor a pause is a revocation, and revocation
		// is what takes a chain out of service — so containment stops this host
		// obtaining anything new without dropping every name it answers on.
		let (_dir, state) = state();
		hold(&state, "app.example.com", true).await;

		with_entitlement(&state, &["example.com"], false, false).await;
		assert!(state.serve("app.example.com").await.is_some());

		with_entitlement(&state, &["example.com"], true, true).await;
		assert!(state.serve("app.example.com").await.is_some());
	}

	#[tokio::test]
	async fn a_handshake_for_a_name_with_nothing_held_records_it() {
		let (_dir, state) = state();
		state.note_wanted("app.example.com").await;
		assert!(state.wanted.lock().await.contains("app.example.com"));

		// A name already held needs no order.
		hold(&state, "other.example.com", true).await;
		state.note_wanted("other.example.com").await;
		assert!(!state.wanted.lock().await.contains("other.example.com"));

		// Something that could not be a name is not recorded at all.
		state.note_wanted("not a name/../etc").await;
		assert_eq!(state.wanted.lock().await.len(), 1);
	}

	/// `usable` is what canopy last said, and nothing re-evaluates it as time
	/// passes. A chain that has since expired is a decline — which hands the
	/// name back to Caddy's own issuance — rather than an expired certificate
	/// served forever because collection stopped.
	///
	/// spec: TLSD#declining-and-failing
	#[tokio::test]
	async fn a_chain_that_has_expired_since_it_was_collected_is_declined() {
		let (_dir, state) = state();
		with_entitlement(&state, &["example.com"], true, false).await;

		let hour = jiff::SignedDuration::from_hours(1);
		hold_until(
			&state,
			"app.example.com",
			true,
			Some(Timestamp::now() + hour),
		)
		.await;
		assert!(state.serve("app.example.com").await.is_some());

		hold_until(
			&state,
			"app.example.com",
			true,
			Some(Timestamp::now() - hour),
		)
		.await;
		assert!(state.serve("app.example.com").await.is_none());
	}

	/// A name recorded during a handshake that the entitlement turns out not to
	/// cover leaves the record as well as the target list: kept, it would make a
	/// pass due on every tick and spend an entitlement request each time on a
	/// name that can never be ordered for.
	#[tokio::test]
	async fn a_name_the_entitlement_does_not_cover_is_dropped_from_the_record() {
		let (_dir, state) = state();
		with_entitlement(&state, &["example.com"], true, false).await;
		state.note_wanted("app.elsewhere.test").await;
		assert!(state.pass_due().await);

		let entitlement = state.entitlement.read().await.clone().unwrap();
		assert!(
			state
				.target_names(&detached_ctx(), &entitlement)
				.await
				.is_empty()
		);

		// Nothing is waiting on a pass any more, so the steady interval governs
		// again rather than every tick making one due.
		assert!(state.wanted.lock().await.is_empty());
		*state.last_pass.write().await = Some(Timestamp::now());
		assert!(!state.pass_due().await);
	}

	#[tokio::test]
	async fn a_recorded_name_is_still_subject_to_the_entitlement() {
		// A handshake cannot conjure an order for a name outside the server's
		// reach: the recorded name meets the same test as one read from Caddy.
		let (_dir, state) = state();
		with_entitlement(&state, &["example.com"], true, false).await;
		state.note_wanted("app.elsewhere.test").await;
		state.note_wanted("app.example.com").await;

		let entitlement = state.entitlement.read().await.clone().unwrap();
		let names = state.target_names(&detached_ctx(), &entitlement).await;
		assert_eq!(names, vec!["app.example.com".to_string()]);
	}

	/// A condemned key takes precedence over everything else the answer carries.
	///
	/// Canopy can answer with a chain and `key_must_be_replaced` together: the
	/// chain covers the key being condemned. Taking it would pair the old chain
	/// with the new key, and the endpoint hands Caddy both — a mismatch fails
	/// the handshake outright, where declining would have let Caddy issue for
	/// the name itself.
	///
	/// spec: TLS#revocation-and-key-replacement
	#[test]
	fn a_condemned_key_is_replaced_and_nothing_else_in_the_answer_is_taken() {
		let condemned = Order {
			state: "issued".into(),
			key_must_be_replaced: true,
			..Order::default()
		};
		assert_eq!(
			disposition(&condemned, Some("-----BEGIN CERTIFICATE-----")),
			Disposition::ReplaceKey
		);
		// Even alongside a revocation, which would otherwise be the action.
		let both = Order {
			revoked: true,
			..condemned.clone()
		};
		assert_eq!(
			disposition(&both, Some("-----BEGIN CERTIFICATE-----")),
			Disposition::ReplaceKey
		);
	}

	/// The rest of the precedence: a revocation drops what is held whatever
	/// chain came with it, a chain is taken, and an empty one waits.
	#[test]
	fn a_revocation_drops_what_is_held_and_a_chain_is_otherwise_taken() {
		let revoked = Order {
			revoked: true,
			..Order::default()
		};
		assert_eq!(disposition(&revoked, Some("chain")), Disposition::DropHeld);

		let issued = Order {
			state: "issued".into(),
			..Order::default()
		};
		assert_eq!(disposition(&issued, Some("chain")), Disposition::TakeChain);

		let pending = Order {
			state: "pending".into(),
			..Order::default()
		};
		assert_eq!(disposition(&pending, None), Disposition::Wait);
		assert_eq!(disposition(&pending, Some("   ")), Disposition::Wait);
	}

	/// A name whose attempt failed outright stays asked for, so it is retried
	/// rather than waiting out the steady interval — but it backs off like a
	/// pending order, rather than waking a full pass on every tick.
	#[tokio::test]
	async fn a_failed_attempt_is_retried_on_the_backoff_rather_than_every_tick() {
		let (_dir, state) = state();
		with_entitlement(&state, &["example.com"], true, false).await;
		state.note_wanted("app.example.com").await;
		*state.last_pass.write().await = Some(Timestamp::now());

		// Nothing attempted yet, so the name asked for makes a pass due.
		let now = Timestamp::now();
		assert!(state.pass_due().await);
		assert!(state.name_due("app.example.com", now).await);

		// A pass that failed on it: the failure is recorded and stamped, and the
		// name is still asked for.
		{
			let mut orders = state.orders.write().await;
			let order = orders.entry("app.example.com".to_string()).or_default();
			order.last_error = Some("canopy unreachable".into());
			order.asked_at = Some(now);
		}
		assert!(state.wanted.lock().await.contains("app.example.com"));
		assert!(!state.name_due("app.example.com", now).await);
		assert!(!state.pass_due().await);

		// Once the backoff has elapsed it is due again.
		let later = now + jiff::SignedDuration::from_secs(PENDING_RETRY.as_secs() as i64 + 1);
		assert!(state.name_due("app.example.com", later).await);
	}

	/// An empty address list is what withdraws a name, so registering with none
	/// would take a production name's records down while reading as a publish
	/// that had worked. Withdrawing is its own endpoint, asked for deliberately.
	///
	/// spec: NAM#registering-addresses-for-a-name
	#[tokio::test]
	async fn registering_with_no_address_is_refused_rather_than_withdrawing() {
		let (_dir, state) = state();
		with_entitlement(&state, &["example.com"], true, false).await;
		let task = CanopyNames::new(Arc::new(state));
		let endpoints = task.http_endpoints();
		let register = endpoints
			.iter()
			.find(|endpoint| endpoint.name == "dns-register")
			.expect("the task exposes dns-register");

		for query in [vec![], vec![("addresses", "")], vec![("addresses", " , ")]] {
			let mut ctx = detached_ctx();
			ctx.query.insert("name".into(), "app.example.com".into());
			for (key, value) in &query {
				ctx.query.insert((*key).into(), (*value).into());
			}
			match (register.handler)(ctx).await {
				TaskEndpointResponse::Error { status, message } => {
					assert_eq!(status, 400);
					assert!(
						message.contains("dns-withdraw"),
						"the refusal must point at the endpoint that does withdraw: {message}"
					);
				}
				_ => panic!("expected a refusal for {query:?}"),
			}
		}
	}

	/// A handshake records a name whatever this server may do, and a server
	/// standing down holds no chains — so every handshake it declines records
	/// one. Those names must not wake a pass, or a paused server spends an
	/// entitlement request every tick on an answer that cannot change.
	///
	/// spec: TLS#when-the-grant-is-absent-or-the-server-is-paused
	#[tokio::test]
	async fn a_server_standing_down_waits_rather_than_asking_every_tick() {
		let (_dir, state) = state();
		// The grant is gone, which is what a pass will find.
		with_entitlement(&state, &["example.com"], false, false).await;
		*state.last_pass.write().await = Some(Timestamp::now());

		state.note_wanted("app.example.com").await;
		assert!(state.pass_due().await, "a name asked for makes a pass due");

		// What a pass finds, and so which branch it takes.
		let entitlement = state.entitlement.read().await.clone().unwrap();
		assert_eq!(
			stands_down(&entitlement),
			Some("this server holds no TLS grant")
		);

		// It drops what it will never order for, and remembers having done so.
		state.stand_down().await;
		assert!(state.wanted.lock().await.is_empty());
		assert!(!state.pass_due().await);

		// And a handshake arriving after it does not wake another.
		state.note_wanted("other.example.com").await;
		assert!(
			!state.pass_due().await,
			"a name recorded under a stand-down must not wake a pass"
		);
	}

	/// Caddy passes the client's own server name through for a wildcard site, so
	/// the name reaching `note_wanted` is remote input. The set it feeds is
	/// bounded, and past the bound a new name is dropped rather than an old one
	/// evicted — evicting would let invented names push out a real client's.
	#[tokio::test]
	async fn a_stream_of_invented_names_cannot_grow_the_set_without_bound() {
		let (_dir, state) = state();
		state.note_wanted("real.example.com").await;
		for n in 0..WANTED_LIMIT * 2 {
			state
				.note_wanted(&format!("invented-{n}.example.com"))
				.await;
		}

		let wanted = state.wanted.lock().await;
		assert_eq!(wanted.len(), WANTED_LIMIT);
		assert!(
			wanted.contains("real.example.com"),
			"the name a real client asked for must not be pushed out"
		);
	}

	/// An order is kept for a name still in play and dropped for one that has
	/// left both Caddy's configuration and the set a handshake feeds, so a host
	/// serving a wildcard does not keep an entry per name ever offered.
	#[tokio::test]
	async fn what_canopy_said_about_a_name_no_longer_in_play_is_dropped() {
		let (_dir, state) = state();
		with_entitlement(&state, &["example.com"], true, false).await;
		let entitlement = state.entitlement.read().await.clone().unwrap();
		hold(&state, "held.example.com", true).await;

		for name in ["gone.example.com", "held.example.com", "app.example.com"] {
			state.orders.write().await.insert(
				name.to_string(),
				Order {
					state: "issued".into(),
					..Order::default()
				},
			);
		}
		// Caddy's admin API is not running under the test, so the set a handshake
		// feeds is the whole of the target list.
		state.note_wanted("app.example.com").await;
		let targets = state.target_names(&detached_ctx(), &entitlement).await;
		assert_eq!(targets, vec!["app.example.com".to_string()]);
		state.prune_orders(&targets).await;

		let orders = state.orders.read().await;
		assert!(orders.contains_key("app.example.com"), "a target is kept");
		assert!(
			orders.contains_key("held.example.com"),
			"a name still holding a chain is kept"
		);
		assert!(
			!orders.contains_key("gone.example.com"),
			"a name in neither is dropped"
		);
	}

	/// Canopy publishes what it is handed, so an address that is not one is
	/// refused here rather than at whichever client happened to call: the daemon
	/// is the component holding the DNS grant.
	///
	/// spec: NAM#registering-addresses-for-a-name
	#[tokio::test]
	async fn an_address_that_is_not_an_ip_is_refused_before_canopy_sees_it() {
		let (_dir, state) = state();
		with_entitlement(&state, &["example.com"], true, false).await;

		let err = state
			.register_name(
				&detached_ctx(),
				"app.example.com",
				vec!["203.0.113.5".into(), "not-an-address".into()],
			)
			.await
			.unwrap_err();
		assert!(
			why(&err).contains("not-an-address"),
			"the offending value must be named: {}",
			why(&err)
		);

		// Well-formed addresses get past the parse and fail for the reason they
		// should: this host has no DNS grant and no canopy client.
		let err = state
			.register_name(&detached_ctx(), "app.example.com", vec!["::1".into()])
			.await
			.unwrap_err();
		assert!(!why(&err).contains("is not an IP address"), "{}", why(&err));
	}

	/// A grant withdrawn under an incident, and a pause, both stop new requests
	/// — and neither is a revocation, so what is already held stays served.
	#[tokio::test]
	async fn a_withdrawn_grant_and_a_pause_both_stop_requesting_without_dropping_what_is_held() {
		let (_dir, state) = state();
		hold(&state, "app.example.com", true).await;

		with_entitlement(&state, &["example.com"], false, false).await;
		let withdrawn = state.entitlement.read().await.clone().unwrap();
		assert_eq!(
			stands_down(&withdrawn),
			Some("this server holds no TLS grant")
		);
		assert!(state.serve("app.example.com").await.is_some());

		with_entitlement(&state, &["example.com"], true, true).await;
		let paused = state.entitlement.read().await.clone().unwrap();
		assert_eq!(
			stands_down(&paused),
			Some("canopy reports this server paused")
		);
		assert!(state.serve("app.example.com").await.is_some());

		with_entitlement(&state, &["example.com"], true, false).await;
		let entitled = state.entitlement.read().await.clone().unwrap();
		assert_eq!(stands_down(&entitled), None);
	}

	/// A name has to meet both tests: Caddy serving it, and the entitlement
	/// covering it. One without the other is left to Caddy's own issuance.
	#[tokio::test]
	async fn a_name_meeting_one_test_and_not_the_other_is_not_ordered_for() {
		let (_dir, state) = state();
		with_entitlement(&state, &["example.com"], true, false).await;
		let entitlement = state.entitlement.read().await.clone().unwrap();

		// Caddy's admin API is not running under this test, so the subjects it
		// would contribute are empty: a name the entitlement covers that Caddy
		// does not serve produces no order.
		assert!(
			state
				.target_names(&detached_ctx(), &entitlement)
				.await
				.is_empty()
		);

		// And a subject the entitlement does not cover is dropped even when it
		// reaches the task by the one route a test can drive.
		state.note_wanted("app.elsewhere.test").await;
		assert!(
			state
				.target_names(&detached_ctx(), &entitlement)
				.await
				.is_empty()
		);
	}

	#[tokio::test]
	async fn a_pending_order_is_retried_sooner_than_the_steady_pass_and_stops_once_it_resolves() {
		let (_dir, state) = state();
		*state.last_pass.write().await = Some(Timestamp::now());

		state.orders.write().await.insert(
			"app.example.com".into(),
			Order {
				state: "pending".into(),
				asked_at: Some(Timestamp::now()),
				..Order::default()
			},
		);
		assert!(!state.pass_due().await, "just asked; not due yet");

		let stale = Timestamp::now() - std::time::Duration::from_secs(300);
		state
			.orders
			.write()
			.await
			.get_mut("app.example.com")
			.unwrap()
			.asked_at = Some(stale);
		assert!(state.pass_due().await, "a pending order is retried sooner");

		state
			.orders
			.write()
			.await
			.get_mut("app.example.com")
			.unwrap()
			.state = "issued".into();
		assert!(
			!state.pass_due().await,
			"an order that resolved stops being retried"
		);
	}

	#[tokio::test]
	async fn a_name_asked_for_makes_a_pass_due() {
		let (_dir, state) = state();
		*state.last_pass.write().await = Some(Timestamp::now());
		assert!(!state.pass_due().await);
		state.note_wanted("app.example.com").await;
		assert!(state.pass_due().await);
	}

	#[tokio::test]
	async fn a_restart_serves_what_it_held_and_keeps_the_key_that_covers_it() {
		// Canopy keys what it holds by name and key, so a key that did not
		// survive would turn a collection into a fresh order.
		let dir = tempfile::tempdir().unwrap();
		let key = certs::generate_key().unwrap();
		let mut store = KeyStore::default();
		store.replace("app.example.com", &key);
		certs::store_keys(dir.path(), &store).await.unwrap();
		certs::store_chain(
			dir.path(),
			"app.example.com",
			"-----BEGIN CERTIFICATE-----\n",
		)
		.await
		.unwrap();

		let state = CertificateState::new(dir.path().to_path_buf(), peer::Permitted::default());
		state.load_from_disk().await.unwrap();

		let (chain, served_key) = state.serve("app.example.com").await.unwrap();
		assert!(chain.contains("BEGIN CERTIFICATE"));
		assert_eq!(
			certs::key_fingerprint(&KeyPair::from_pem(&served_key).unwrap()),
			certs::key_fingerprint(&key)
		);
	}

	#[tokio::test]
	async fn a_chain_whose_key_is_gone_is_not_served() {
		let dir = tempfile::tempdir().unwrap();
		certs::store_chain(
			dir.path(),
			"app.example.com",
			"-----BEGIN CERTIFICATE-----\n",
		)
		.await
		.unwrap();

		let state = CertificateState::new(dir.path().to_path_buf(), peer::Permitted::default());
		state.load_from_disk().await.unwrap();
		assert!(state.serve("app.example.com").await.is_none());
	}
}
