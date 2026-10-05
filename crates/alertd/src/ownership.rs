//! Which application a DNS name belongs to.
//!
//! Canopy resolves a request from the DNS name and the application type it
//! carries, so this side has to say which application each DNS name is for. The
//! answer is one function, shared by the daemon that requests certificates and
//! by the checks that grade them, so a request and the report on it never
//! disagree:
//!
//! 1. the application the DNS name's Caddy site is attributed to;
//! 2. otherwise the application whose declared names list it;
//! 3. otherwise none, and the DNS name is left alone.
//!
//! A site is attributed from Caddy's live admin configuration: first by whether
//! one of its addresses is an application's canonical URL host, then by the
//! upstreams it proxies to.
//!
//! spec: NAM#which-application-a-dns-name-belongs-to

use std::collections::{BTreeMap, BTreeSet};

use bestool_canopy::names::Entitlement;
use bestool_tamanu::{ApiServerKind, config::TamanuConfig};
use serde_json::Value;
use url::Url;

use crate::subject::ApplicationKind;

/// An application on this machine, and how Caddy reaches it.
///
/// spec: NAM#which-application-a-dns-name-belongs-to
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostApplication {
	/// The application type as canopy names it, which is what a request carries.
	pub type_slug: String,
	/// The hosts of its configured canonical URLs.
	pub canonical_hosts: BTreeSet<String>,
	/// The service names its deployment gives its own upstreams.
	pub service_names: BTreeSet<String>,
	/// Service names it shares with other applications of the same product, so
	/// that proxying to one identifies the product but not which application.
	pub shared_names: BTreeSet<String>,
	/// The local ports it is configured to listen on.
	pub local_ports: BTreeSet<u16>,
}

impl HostApplication {
	/// A Tamanu deployment.
	///
	/// Its API service name carries the role, while the frontend and patient
	/// portal services belong to Tamanu without saying which one.
	pub fn tamanu(kind: ApiServerKind, config: &TamanuConfig) -> Self {
		let role = match kind {
			ApiServerKind::Central => "central",
			ApiServerKind::Facility => "facility",
		};
		let canonical_hosts = [
			config.canonical_host_name.as_ref(),
			config.canonical_url.as_ref(),
		]
		.into_iter()
		.flatten()
		.filter_map(url_host)
		.collect();

		Self {
			type_slug: ApplicationKind::from(kind).type_slug().to_owned(),
			canonical_hosts,
			service_names: [format!("api.{role}.tamanu.internal")].into(),
			shared_names: [
				"frontend.tamanu.internal".to_owned(),
				"patientportal.tamanu.internal".to_owned(),
			]
			.into(),
			local_ports: [config.port()].into(),
		}
	}

	/// The mSupply deployment.
	pub fn msupply() -> Self {
		Self {
			type_slug: crate::msupply::TYPE_SLUG.to_owned(),
			canonical_hosts: BTreeSet::new(),
			service_names: ["api.msupply.internal".to_owned()].into(),
			shared_names: BTreeSet::new(),
			local_ports: [8000].into(),
		}
	}

	fn owns(&self, upstream: &Upstream) -> bool {
		match upstream {
			Upstream::Name(name) => {
				self.service_names.contains(name) || self.shared_names.contains(name)
			}
			Upstream::Local(port) => self.local_ports.contains(port),
		}
	}
}

/// The applications on this machine that Caddy can be fronting: the Tamanu
/// deployment its install says is here, and mSupply where it is installed.
pub async fn discover_host_applications() -> Vec<HostApplication> {
	let mut out = Vec::new();

	match bestool_tamanu::try_find_tamanu(None).await {
		Ok(Some((_, root))) => match bestool_tamanu::config::load_config(&root, None) {
			Ok(config) => {
				let kind = bestool_tamanu::detect_kind(&config, None).await;
				out.push(HostApplication::tamanu(kind, &config));
			}
			Err(err) => tracing::debug!(%err, "could not read the Tamanu configuration"),
		},
		Ok(None) => {}
		Err(err) => tracing::debug!(%err, "could not look for a Tamanu install"),
	}

	if crate::msupply::installed() {
		out.push(HostApplication::msupply());
	}
	out
}

fn url_host(url: &Url) -> Option<String> {
	url.host_str().map(normalise)
}

fn normalise(name: &str) -> String {
	name.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Where a reverse proxy sends traffic, as far as attribution cares.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Upstream {
	/// A service or host name.
	Name(String),
	/// A port on this machine.
	Local(u16),
}

impl Upstream {
	fn from_dial(dial: &str) -> Option<Self> {
		let dial = dial.trim();
		let dial = dial.split_once("://").map_or(dial, |(_, rest)| rest);
		let dial = dial.split('/').next().unwrap_or(dial);
		if dial.is_empty() || dial.starts_with("unix") || dial.contains('{') {
			return None;
		}

		let (host, port) = match dial.strip_prefix('[') {
			Some(rest) => {
				let (host, after) = rest.split_once(']')?;
				(host, after.strip_prefix(':'))
			}
			None => match dial.rsplit_once(':') {
				Some((host, port)) if !host.contains(':') => (host, Some(port)),
				_ => (dial, None),
			},
		};
		let host = normalise(host);
		let port = port.and_then(|p| p.parse::<u16>().ok());

		match (is_loopback(&host), port) {
			(true, Some(port)) => Some(Self::Local(port)),
			_ => Some(Self::Name(host)),
		}
	}
}

fn is_loopback(host: &str) -> bool {
	matches!(host, "localhost" | "127.0.0.1" | "::1")
}

/// One site Caddy serves: the addresses it answers on and where it proxies.
#[derive(Clone, Debug, Default)]
struct Site {
	hosts: BTreeSet<String>,
	upstreams: BTreeSet<Upstream>,
	/// The upstreams reached through a route matching `/api/*`.
	api_upstreams: BTreeSet<Upstream>,
}

/// What a TLS automation policy says to ask the daemon for.
#[derive(Clone, Debug)]
struct HookedPolicy {
	/// Empty for a policy that applies to every name.
	subjects: Vec<String>,
}

/// The sites in Caddy's live configuration.
///
/// spec: TLS#which-dns-names-are-certified
#[derive(Clone, Debug, Default)]
pub struct CaddySites {
	sites: Vec<Site>,
	hooked: Vec<HookedPolicy>,
}

impl CaddySites {
	/// Read the sites and the certificate hooks out of Caddy's admin
	/// configuration.
	pub fn from_config(config: &Value) -> Self {
		let mut sites = Vec::new();
		if let Some(servers) = config["apps"]["http"]["servers"].as_object() {
			for server in servers.values() {
				for route in server["routes"].as_array().into_iter().flatten() {
					let mut site = Site::default();
					collect_site(route, false, &mut site);
					if !site.hosts.is_empty() {
						sites.push(site);
					}
				}
			}
		}

		let hooked = config["apps"]["tls"]["automation"]["policies"]
			.as_array()
			.into_iter()
			.flatten()
			.filter(|policy| names_the_daemon(&policy["get_certificate"]))
			.map(|policy| HookedPolicy {
				subjects: strings(&policy["subjects"]),
			})
			.collect();

		Self { sites, hooked }
	}

	/// Every address a site is configured to serve.
	pub fn addresses(&self) -> BTreeSet<String> {
		self.sites
			.iter()
			.flat_map(|site| site.hosts.iter().cloned())
			.collect()
	}

	/// The addresses whose TLS automation policy names the daemon's certificate
	/// endpoint as a certificate source.
	///
	/// A site that doesn't is never served from canopy, so nothing is ordered
	/// for it of this side's own accord.
	///
	/// spec: TLS#which-dns-names-are-certified
	pub fn hooked_addresses(&self) -> BTreeSet<String> {
		self.addresses()
			.into_iter()
			.filter(|address| self.is_hooked(address))
			.collect()
	}

	/// Whether Caddy is configured to serve `name` on some site, hooked or not.
	pub fn serves(&self, name: &str) -> bool {
		let name = normalise(name);
		self.sites
			.iter()
			.any(|site| site.hosts.iter().any(|host| host_covers(host, &name)))
	}

	fn is_hooked(&self, address: &str) -> bool {
		self.hooked.iter().any(|policy| {
			policy.subjects.is_empty()
				|| policy
					.subjects
					.iter()
					.any(|subject| host_covers(&normalise(subject), address))
		})
	}

	/// The application each address is attributed to.
	///
	/// An address left out is unattributed: its site proxies to nothing
	/// recognised, to several applications, or to none; or it is served by
	/// sites attributed to different applications.
	pub fn attribute(&self, applications: &[HostApplication]) -> BTreeMap<String, String> {
		let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
		for site in &self.sites {
			let Some(owner) = attribute_site(site, applications) else {
				continue;
			};
			for host in &site.hosts {
				seen.entry(host.clone())
					.or_default()
					.insert(owner.to_owned());
			}
		}
		seen.into_iter()
			.filter_map(|(host, owners)| {
				let mut owners = owners.into_iter();
				let owner = owners.next()?;
				owners.next().is_none().then_some((host, owner))
			})
			.collect()
	}
}

/// Whether a configured address (possibly a wildcard) answers for `name`.
///
/// A wildcard stands for exactly one label.
pub fn host_covers(pattern: &str, name: &str) -> bool {
	if pattern == name {
		return true;
	}
	match pattern.strip_prefix("*.") {
		Some(suffix) => name
			.split_once('.')
			.is_some_and(|(label, rest)| !label.is_empty() && rest == suffix),
		None => false,
	}
}

fn strings(value: &Value) -> Vec<String> {
	value
		.as_array()
		.into_iter()
		.flatten()
		.filter_map(Value::as_str)
		.map(normalise)
		.collect()
}

/// Whether a policy's `get_certificate` managers include one asking the
/// daemon's `/certificate` endpoint over HTTP.
fn names_the_daemon(managers: &Value) -> bool {
	managers.as_array().into_iter().flatten().any(|manager| {
		manager["via"].as_str() == Some("http")
			&& manager["url"].as_str().is_some_and(|url| {
				Url::parse(url).is_ok_and(|url| {
					url.path() == "/certificate"
						&& url
							.host_str()
							.is_some_and(|host| is_loopback(host.trim_matches(['[', ']'])))
				})
			})
	})
}

fn collect_site(route: &Value, in_api_route: bool, site: &mut Site) {
	let mut api = in_api_route;
	for matcher in route["match"].as_array().into_iter().flatten() {
		site.hosts.extend(strings(&matcher["host"]));
		if matcher["path"]
			.as_array()
			.into_iter()
			.flatten()
			.filter_map(Value::as_str)
			.any(|path| path.starts_with("/api"))
		{
			api = true;
		}
	}

	for handler in route["handle"].as_array().into_iter().flatten() {
		match handler["handler"].as_str() {
			Some("reverse_proxy") => {
				for upstream in proxy_upstreams(handler) {
					if api {
						site.api_upstreams.insert(upstream.clone());
					}
					site.upstreams.insert(upstream);
				}
			}
			_ => {
				for inner in handler["routes"].as_array().into_iter().flatten() {
					collect_site(inner, api, site);
				}
			}
		}
	}
}

fn proxy_upstreams(handler: &Value) -> Vec<Upstream> {
	let mut out: Vec<Upstream> = handler["upstreams"]
		.as_array()
		.into_iter()
		.flatten()
		.filter_map(|upstream| Upstream::from_dial(upstream["dial"].as_str()?))
		.collect();

	let dynamic = &handler["dynamic_upstreams"];
	if let Some(name) = dynamic["name"].as_str().filter(|name| !name.contains('{')) {
		let port = match &dynamic["port"] {
			Value::String(port) => port.parse::<u16>().ok(),
			Value::Number(port) => port.as_u64().and_then(|port| u16::try_from(port).ok()),
			_ => None,
		};
		let name = normalise(name);
		out.push(match (is_loopback(&name), port) {
			(true, Some(port)) => Upstream::Local(port),
			_ => Upstream::Name(name),
		});
	}
	out
}

/// The applications that own every upstream in `upstreams`.
///
/// Empty when there are no upstreams: "every upstream is theirs" is vacuously
/// true of a site that proxies nowhere, and that is not attribution.
fn owners_of_all<'a>(
	upstreams: &BTreeSet<Upstream>,
	applications: &'a [HostApplication],
) -> Vec<&'a HostApplication> {
	if upstreams.is_empty() {
		return Vec::new();
	}
	applications
		.iter()
		.filter(|app| upstreams.iter().all(|upstream| app.owns(upstream)))
		.collect()
}

fn attribute_site<'a>(site: &Site, applications: &'a [HostApplication]) -> Option<&'a str> {
	let by_canonical: Vec<&HostApplication> = applications
		.iter()
		.filter(|app| {
			site.hosts
				.iter()
				.any(|host| app.canonical_hosts.contains(host))
		})
		.collect();
	match by_canonical.as_slice() {
		[only] => return Some(&only.type_slug),
		[] => {}
		_ => return None,
	}

	let by_upstream = owners_of_all(&site.upstreams, applications);
	match by_upstream.as_slice() {
		[only] => Some(&only.type_slug),
		[] => None,
		// A frontend shared between applications identifies none of them, so the
		// API route's own upstream decides.
		several => match owners_of_all(&site.api_upstreams, applications).as_slice() {
			[only] if several.iter().any(|app| app.type_slug == only.type_slug) => {
				Some(&only.type_slug)
			}
			_ => None,
		},
	}
}

/// Which application each DNS name belongs to, as of one reading of Caddy's
/// configuration and canopy's answer.
///
/// spec: NAM#which-application-a-dns-name-belongs-to
#[derive(Clone, Debug, Default)]
pub struct Ownership {
	attributed: BTreeMap<String, String>,
	declared: BTreeMap<String, String>,
}

impl Ownership {
	pub fn resolve(
		sites: &CaddySites,
		applications: &[HostApplication],
		entitlement: &Entitlement,
	) -> Self {
		let attributed = sites.attribute(applications);

		let mut declared: BTreeMap<String, String> = BTreeMap::new();
		let mut ambiguous: BTreeSet<String> = BTreeSet::new();
		for entry in &entitlement.applications {
			// A machine canopy answers for in its top-level fields hosts one
			// application and the entry carries no type, so the application on the
			// host is the one declaring.
			let type_slug = match (&entry.type_slug, applications) {
				(Some(type_slug), _) => type_slug.clone(),
				(None, [only]) => only.type_slug.clone(),
				(None, _) => continue,
			};
			for name in &entry.registered_names {
				let name = normalise(name);
				match declared.get(&name) {
					Some(existing) if *existing != type_slug => {
						ambiguous.insert(name);
					}
					_ => {
						declared.insert(name, type_slug.clone());
					}
				}
			}
		}
		for name in ambiguous {
			declared.remove(&name);
		}

		Self {
			attributed,
			declared,
		}
	}

	/// The type of the application `dns_name` belongs to, or `None` where it
	/// belongs to none.
	pub fn owner(&self, dns_name: &str) -> Option<&str> {
		let name = normalise(dns_name);
		fn lookup<'a>(map: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
			map.get(name)
				.or_else(|| {
					map.iter()
						.find(|(pattern, _)| host_covers(pattern, name))
						.map(|(_, owner)| owner)
				})
				.map(String::as_str)
		}
		lookup(&self.attributed, &name).or_else(|| lookup(&self.declared, &name))
	}

	/// The types of the applications a certificate for `cert_name` serves.
	///
	/// A certificate for a wildcard serves every known DNS name the wildcard
	/// covers, so it belongs to each application owning one of them, as well as
	/// to the owner of the wildcard itself.
	///
	/// spec: CHK-CCT
	pub fn owners_served_by(&self, cert_name: &str) -> BTreeSet<&str> {
		let cert_name = normalise(cert_name);
		let mut owners: BTreeSet<&str> = self.owner(&cert_name).into_iter().collect();
		if cert_name.starts_with("*.") {
			for name in self.attributed.keys().chain(self.declared.keys()) {
				if host_covers(&cert_name, name)
					&& let Some(owner) = self.owner(name)
				{
					owners.insert(owner);
				}
			}
		}
		owners
	}
}

#[cfg(test)]
mod tests {
	use bestool_canopy::schema::{ApplicationEntitlements, Entitlements};
	use serde_json::json;

	use super::*;

	fn tamanu(role: ApiServerKind, canonical: Option<&str>, port: Option<u16>) -> HostApplication {
		let mut config = TamanuConfig::from_database(
			bestool_tamanu::config::Database::from_url("postgresql://u:p@localhost/db").unwrap(),
		);
		config.canonical_url = canonical.map(|url| url.parse().unwrap());
		config.port = port;
		HostApplication::tamanu(role, &config)
	}

	fn central() -> HostApplication {
		tamanu(
			ApiServerKind::Central,
			Some("https://central.example.com"),
			None,
		)
	}

	fn facility() -> HostApplication {
		tamanu(ApiServerKind::Facility, None, None)
	}

	fn proxy_dynamic(name: &str) -> Value {
		json!({"handler": "reverse_proxy", "dynamic_upstreams": {"source": "a", "name": name, "port": "3000"}})
	}

	fn proxy_dial(dial: &str) -> Value {
		json!({"handler": "reverse_proxy", "upstreams": [{"dial": dial}]})
	}

	fn site(hosts: &[&str], handlers: Vec<Value>) -> Value {
		json!({
			"match": [{"host": hosts}],
			"handle": [{"handler": "subroute", "routes": [{"handle": handlers}]}],
		})
	}

	fn api_route(upstream: Value) -> Value {
		json!({"match": [{"path": ["/api/*"]}], "handle": [upstream]})
	}

	fn caddy(routes: Vec<Value>, hook_subjects: Option<&[&str]>) -> Value {
		let policies = match hook_subjects {
			Some(subjects) => json!([{
				"subjects": subjects,
				"get_certificate": [{"via": "http", "url": "http://127.0.0.1:8271/certificate"}],
			}]),
			None => json!([]),
		};
		json!({"apps": {
			"http": {"servers": {"srv0": {"routes": routes}}},
			"tls": {"automation": {"policies": policies}},
		}})
	}

	fn attributed(config: Value, apps: &[HostApplication]) -> BTreeMap<String, String> {
		CaddySites::from_config(&config).attribute(apps)
	}

	#[test]
	fn a_site_on_a_canonical_url_host_is_that_tamanus_even_with_another_upstream() {
		let config = caddy(
			vec![site(
				&["central.example.com"],
				vec![proxy_dynamic("api.msupply.internal")],
			)],
			None,
		);
		let owners = attributed(config, &[central(), HostApplication::msupply()]);
		assert_eq!(owners["central.example.com"], "tamanu-central");
	}

	#[test]
	fn upstreams_attribute_a_site() {
		let config = caddy(
			vec![
				site(
					&["fac.example.com"],
					vec![
						proxy_dynamic("api.facility.tamanu.internal"),
						proxy_dynamic("frontend.tamanu.internal"),
					],
				),
				site(
					&["supply.example.com"],
					vec![proxy_dynamic("api.msupply.internal")],
				),
			],
			None,
		);
		let owners = attributed(config, &[facility(), HostApplication::msupply()]);
		assert_eq!(owners["fac.example.com"], "tamanu-facility");
		assert_eq!(owners["supply.example.com"], "msupply");
	}

	#[test]
	fn a_local_port_attributes_a_windows_style_site() {
		let config = caddy(
			vec![site(
				&["fac.example.com"],
				vec![proxy_dial("localhost:3100")],
			)],
			None,
		);
		let app = tamanu(ApiServerKind::Facility, None, Some(3100));
		assert_eq!(
			attributed(config, &[app])["fac.example.com"],
			"tamanu-facility"
		);
	}

	#[test]
	fn a_shared_frontend_is_decided_by_the_api_route() {
		let both = [central(), facility()];
		let frontend = json!({"handle": [proxy_dynamic("frontend.tamanu.internal")]});

		let config = caddy(
			vec![json!({
				"match": [{"host": ["a.example.com"]}],
				"handle": [{"handler": "subroute", "routes": [
					api_route(proxy_dynamic("api.facility.tamanu.internal")),
					frontend.clone(),
				]}],
			})],
			None,
		);
		assert_eq!(
			attributed(config, &both)["a.example.com"],
			"tamanu-facility"
		);

		// Nothing says which Tamanu it is.
		let config = caddy(
			vec![site(
				&["b.example.com"],
				vec![proxy_dynamic("frontend.tamanu.internal")],
			)],
			None,
		);
		assert!(attributed(config, &both).is_empty());

		// With one Tamanu on the host the frontend is that Tamanu's.
		let config = caddy(
			vec![site(
				&["b.example.com"],
				vec![proxy_dynamic("frontend.tamanu.internal")],
			)],
			None,
		);
		assert_eq!(
			attributed(config, &[facility()])["b.example.com"],
			"tamanu-facility"
		);
	}

	#[test]
	fn a_site_with_no_upstream_is_unattributed_not_everyones() {
		let redirect = json!({
			"match": [{"host": ["old.example.com"]}],
			"handle": [{"handler": "static_response", "status_code": 301}],
		});
		let config = caddy(vec![redirect], None);
		assert!(attributed(config, &[central(), HostApplication::msupply()]).is_empty());
	}

	#[test]
	fn mixed_and_unrecognised_upstreams_are_unattributed() {
		let apps = [central(), HostApplication::msupply()];
		let mixed = caddy(
			vec![site(
				&["mix.example.com"],
				vec![
					proxy_dynamic("api.central.tamanu.internal"),
					proxy_dynamic("api.msupply.internal"),
				],
			)],
			None,
		);
		assert!(attributed(mixed, &apps).is_empty());

		let unknown = caddy(
			vec![site(&["x.example.com"], vec![proxy_dial("10.0.0.9:9000")])],
			None,
		);
		assert!(attributed(unknown, &apps).is_empty());
	}

	#[test]
	fn a_dns_name_served_by_sites_of_different_applications_is_unattributed() {
		let config = caddy(
			vec![
				site(
					&["shared.example.com"],
					vec![proxy_dynamic("api.central.tamanu.internal")],
				),
				site(
					&["shared.example.com"],
					vec![proxy_dynamic("api.msupply.internal")],
				),
			],
			None,
		);
		let owners = attributed(config, &[central(), HostApplication::msupply()]);
		assert!(!owners.contains_key("shared.example.com"));
	}

	#[test]
	fn only_hooked_addresses_are_certified_unless_a_policy_covers_everything() {
		let routes = vec![
			site(&["hooked.example.com"], vec![]),
			site(&["plain.example.com"], vec![]),
		];
		let sites = CaddySites::from_config(&caddy(routes.clone(), Some(&["hooked.example.com"])));
		assert_eq!(sites.addresses().len(), 2);
		assert_eq!(
			sites.hooked_addresses(),
			["hooked.example.com".to_owned()].into()
		);

		let none = CaddySites::from_config(&caddy(routes.clone(), None));
		assert!(none.hooked_addresses().is_empty());

		let wildcard = CaddySites::from_config(&caddy(routes, Some(&["*.example.com"])));
		assert_eq!(wildcard.hooked_addresses().len(), 2);
	}

	#[test]
	fn a_manager_that_is_not_the_daemons_endpoint_is_not_a_hook() {
		for manager in [
			json!({"via": "http", "url": "http://example.com/certificate"}),
			json!({"via": "http", "url": "http://127.0.0.1:8271/other"}),
			json!({"via": "tailscale"}),
		] {
			let config = json!({"apps": {
				"http": {"servers": {"s": {"routes": [site(&["a.example.com"], vec![])]}}},
				"tls": {"automation": {"policies": [{"subjects": ["a.example.com"], "get_certificate": [manager]}]}},
			}});
			assert!(
				CaddySites::from_config(&config)
					.hooked_addresses()
					.is_empty()
			);
		}
	}

	#[test]
	fn caddy_serving_a_name_does_not_depend_on_the_hook() {
		let sites = CaddySites::from_config(&caddy(
			vec![site(&["plain.example.com", "*.wild.example.com"], vec![])],
			None,
		));
		assert!(sites.serves("plain.example.com"));
		assert!(sites.serves("x.wild.example.com"));
		assert!(!sites.serves("x.y.wild.example.com"));
		assert!(!sites.serves("other.example.com"));
	}

	fn entitlement(declared: &[(&str, &[&str])]) -> Entitlement {
		let applications: Vec<_> = declared
			.iter()
			.map(|(type_slug, names)| {
				ApplicationEntitlements::builder()
					.certificates(vec![])
					.domains(vec!["example.com".to_string()])
					.may_manage_dns(false)
					.may_manage_tls(true)
					.paused(false)
					.registered_names(names.iter().map(|n| (*n).to_string()).collect::<Vec<_>>())
					.type_((*type_slug).parse().unwrap())
					.build()
			})
			.collect();
		Entitlement::from_wire(
			&Entitlements::builder()
				.applications(applications)
				.certificates(vec![])
				.domains(vec![])
				.may_manage_dns(false)
				.may_manage_tls(false)
				.paused(false)
				.registered_names(vec![])
				.build(),
		)
	}

	#[test]
	fn a_site_attribution_beats_a_declaration_and_a_declaration_fills_the_gaps() {
		let config = caddy(
			vec![site(
				&["tam.example.com"],
				vec![proxy_dynamic("api.central.tamanu.internal")],
			)],
			None,
		);
		let sites = CaddySites::from_config(&config);
		let apps = [central(), HostApplication::msupply()];
		// Canopy has the Tamanu's name declared under mSupply, and a name no site
		// attributes declared under mSupply too.
		let ownership = Ownership::resolve(
			&sites,
			&apps,
			&entitlement(&[("msupply", &["tam.example.com", "elsewhere.example.com"])]),
		);
		assert_eq!(ownership.owner("tam.example.com"), Some("tamanu-central"));
		assert_eq!(ownership.owner("elsewhere.example.com"), Some("msupply"));
		assert_eq!(ownership.owner("nobody.example.com"), None);
	}

	#[test]
	fn a_name_declared_by_two_applications_belongs_to_neither() {
		let ownership = Ownership::resolve(
			&CaddySites::default(),
			&[],
			&entitlement(&[
				("tamanu-central", &["dup.example.com", "solo.example.com"]),
				("msupply", &["dup.example.com"]),
			]),
		);
		assert_eq!(ownership.owner("dup.example.com"), None);
		assert_eq!(ownership.owner("solo.example.com"), Some("tamanu-central"));
	}

	#[test]
	fn a_flat_answer_is_declared_by_the_one_application_on_the_host() {
		let flat = Entitlement::from_wire(
			&Entitlements::builder()
				.applications(vec![])
				.certificates(vec![])
				.domains(vec!["example.com".to_string()])
				.may_manage_dns(false)
				.may_manage_tls(true)
				.paused(false)
				.registered_names(vec!["decl.example.com".to_string()])
				.build(),
		);
		let one = Ownership::resolve(&CaddySites::default(), &[central()], &flat);
		assert_eq!(one.owner("decl.example.com"), Some("tamanu-central"));

		let two = Ownership::resolve(
			&CaddySites::default(),
			&[central(), HostApplication::msupply()],
			&flat,
		);
		assert_eq!(two.owner("decl.example.com"), None);
	}

	#[test]
	fn a_wildcard_site_owns_the_names_beneath_it() {
		let config = caddy(
			vec![site(
				&["*.supply.example.com"],
				vec![proxy_dynamic("api.msupply.internal")],
			)],
			None,
		);
		let ownership = Ownership::resolve(
			&CaddySites::from_config(&config),
			&[HostApplication::msupply()],
			&Entitlement::default(),
		);
		assert_eq!(ownership.owner("a.supply.example.com"), Some("msupply"));
		assert_eq!(ownership.owner("A.Supply.Example.Com."), Some("msupply"));
	}

	#[test]
	fn dials_are_read_for_hosts_and_ports() {
		assert_eq!(
			Upstream::from_dial("localhost:3000"),
			Some(Upstream::Local(3000))
		);
		assert_eq!(
			Upstream::from_dial("[::1]:3000"),
			Some(Upstream::Local(3000))
		);
		assert_eq!(
			Upstream::from_dial("http://API.Msupply.Internal:8000"),
			Some(Upstream::Name("api.msupply.internal".into()))
		);
		assert_eq!(Upstream::from_dial("unix//run/app.sock"), None);
		assert_eq!(Upstream::from_dial("{http.request.host}:80"), None);
	}
}
