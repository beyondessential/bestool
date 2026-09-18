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
		chain
			.usable
			.then(|| (chain.chain.clone(), chain.key_pem.clone()))
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
		if self.wanted.lock().await.insert(name.clone()) {
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
		if steady_due || !self.wanted.lock().await.is_empty() {
			return true;
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

		// A server that may not obtain certificates stops requesting them, and
		// so does one canopy reports as paused. Neither takes a collected chain
		// out of service, so what is held stays served.
		//
		// spec: TLS#when-the-grant-is-absent-or-the-server-is-paused
		if !entitlement.holds_tls_grant() {
			debug!("no TLS grant; not requesting certificates");
			return Ok(());
		}
		if entitlement.fully_paused() {
			debug!("canopy reports this server paused; waiting rather than retrying");
			return Ok(());
		}

		let names = self.target_names(ctx, &entitlement).await;
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
			if let Err(err) = self.collect_one(ctx, &name).await {
				// One name failing must not stop the rest: a stuck order on one
				// site is not a reason to leave every other name uncollected.
				warn!(name, %err, "could not collect a certificate");
				self.orders
					.write()
					.await
					.entry(name.clone())
					.or_default()
					.last_error = Some(format!("{err}"));
			}
			self.wanted.lock().await.remove(&name);
		}
		Ok(())
	}

	/// Whether one name is due to be asked about outside a steady pass.
	async fn name_due(&self, name: &str, now: Timestamp) -> bool {
		if self.wanted.lock().await.contains(name) {
			return true;
		}
		match self.orders.read().await.get(name) {
			None => true,
			Some(order) if order.pending() => order
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
		names.extend(self.wanted.lock().await.iter().cloned());

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

		// A key canopy condemns is replaced before the next request, rather than
		// asked against again. It is never certified again for any name, so
		// replacing it is the only way forward and no operator has to act.
		//
		// spec: TLS#revocation-and-key-replacement
		if answer.key_must_be_replaced {
			info!(name, "canopy condemned this key; generating a replacement");
			self.replace_key(name).await?;
		}

		// A certificate canopy reports as revoked stops being served at once. A
		// replacement is requested under the ordinary schedule: revoking pauses
		// the server, so asking now would only be refused, and the refusal is an
		// operator deciding when this host may have a certificate again.
		if answer.revoked {
			info!(
				name,
				"canopy reports this certificate revoked; taking it out of service"
			);
			self.drop_held(name).await?;
			return Ok(());
		}

		match answer.chain {
			Some(chain) if !chain.trim().is_empty() => {
				self.take_chain(name, chain, not_after, answer.usable)
					.await?;
			}
			_ => debug!(name, state = %answer.state, "no chain yet"),
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
					*self.state.last_error.write().await = Some(format!("{err}"));
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
			endpoint("collect", self.state.clone(), |state, ctx| {
				Box::pin(async move {
					state.pass(&ctx, true).await?;
					*state.last_pass.write().await = Some(Timestamp::now());
					Ok(state.report().await)
				})
			}),
			endpoint("request", self.state.clone(), |state, ctx| {
				Box::pin(async move {
					let name = required(&ctx, "name")?;
					certs::plausible_name(&name)?;
					state.wanted.lock().await.insert(name.to_ascii_lowercase());
					state.pass(&ctx, false).await?;
					Ok(state.report().await)
				})
			}),
			endpoint("dns-register", self.state.clone(), |state, ctx| {
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
					let answer = state.register_name(&ctx, &name, addresses).await?;
					Ok(registered(&answer))
				})
			}),
			endpoint("dns-withdraw", self.state.clone(), |state, ctx| {
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
	let handler: TaskEndpointHandler = Arc::new(move |ctx: TaskContext| {
		let state = state.clone();
		Box::pin(async move {
			match run(state, ctx).await {
				Ok(value) => TaskEndpointResponse::Json(value),
				Err(err) => TaskEndpointResponse::Error {
					status: 400,
					message: format!("{err}"),
				},
			}
		})
	});
	TaskEndpoint { name, handler }
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

	async fn hold(state: &CertificateState, name: &str, usable: bool) {
		state.held.write().await.insert(
			name.to_owned(),
			Held {
				chain: format!("chain for {name}"),
				key_pem: format!("key for {name}"),
				not_after: None,
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

	#[tokio::test]
	async fn a_recorded_name_is_still_subject_to_the_entitlement() {
		// A handshake cannot conjure an order for a name outside the server's
		// reach: the recorded name meets the same test as one read from Caddy.
		let (_dir, state) = state();
		with_entitlement(&state, &["example.com"], true, false).await;
		state.note_wanted("app.elsewhere.test").await;
		state.note_wanted("app.example.com").await;

		let entitlement = state.entitlement.read().await.clone().unwrap();
		let ctx = TaskContext {
			http_client: reqwest::Client::new(),
			canopy_client: None,
			reload: tokio::sync::watch::channel(0u64).1,
			#[cfg(windows)]
			restart: None,
			query: Default::default(),
		};
		let names = state.target_names(&ctx, &entitlement).await;
		assert_eq!(names, vec!["app.example.com".to_string()]);
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
