//! What this server may do with names, as canopy answers it.
//!
//! A server asks rather than remembering, because a grant can be withdrawn at
//! any time. The answer describes a machine hosting exactly one application in
//! its top-level fields, and a machine hosting several — or none — by leaving
//! those empty and listing the applications, each with its own domains, grants
//! and paused state.
//!
//! [`Entitlement`] flattens both forms into one list, so nothing downstream has
//! to know which shape came back. Asking is done on the union of that list,
//! because nothing on this side ties a Caddy site to an application; reporting
//! is done per entry, because canopy's answer says which application declares
//! each name.
//!
//! spec: NAM

use bes_canopy_api::schema::{ApplicationEntitlements, Entitlements, HeldCertificate};

use crate::certificates::name_within;

/// One application's entitlement, whichever shape canopy answered in.
#[derive(Clone, Debug)]
pub struct AppEntitlement {
	/// The application type as canopy names it, which is how a reporter
	/// correlates an entry to a workload it runs. `None` on a machine canopy
	/// answered for in its top-level fields, where there is one application and
	/// the answer is about it.
	pub type_slug: Option<String>,
	pub domains: Vec<String>,
	pub may_manage_dns: bool,
	pub may_manage_tls: bool,
	pub paused: bool,
	pub registered_names: Vec<String>,
	pub certificates: Vec<HeldCertificate>,
}

impl AppEntitlement {
	/// Whether this application could act on `name` right now: within its
	/// group's domains, holding the TLS grant, and not paused.
	pub fn may_certify(&self, name: &str) -> bool {
		self.may_manage_tls && !self.paused && self.covers(name)
	}

	/// Whether `name` sits within a domain this application's group controls,
	/// whatever grants it holds.
	pub fn covers(&self, name: &str) -> bool {
		self.domains.iter().any(|domain| name_within(name, domain))
	}

	/// What canopy holds for `name`, where it holds anything.
	pub fn certificate(&self, name: &str) -> Option<&HeldCertificate> {
		self.certificates
			.iter()
			.find(|held| held.name.eq_ignore_ascii_case(name))
	}
}

/// What canopy told this machine it may do, flattened across its applications.
///
/// spec: NAM#machines-hosting-several-applications
#[derive(Clone, Debug, Default)]
pub struct Entitlement {
	pub applications: Vec<AppEntitlement>,
}

impl Entitlement {
	/// Flatten canopy's answer.
	///
	/// The top-level fields are the answer for a machine hosting one
	/// application, and the list is the answer for one hosting several. A
	/// machine hosting none gets neither, and comes back with no applications —
	/// which is an empty answer rather than an error, asking what one may do not
	/// being a privileged act.
	pub fn from_wire(wire: &Entitlements) -> Self {
		if !wire.applications.is_empty() {
			return Self {
				applications: wire.applications.iter().map(from_application).collect(),
			};
		}

		// A machine with no grants and no domains at all is an empty answer, not
		// a single application holding nothing: carrying it as an entry would
		// have a reporter file results against an application that is not there.
		if wire.domains.is_empty()
			&& !wire.may_manage_dns
			&& !wire.may_manage_tls
			&& wire.certificates.is_empty()
			&& wire.registered_names.is_empty()
		{
			return Self::default();
		}

		Self {
			applications: vec![AppEntitlement {
				type_slug: None,
				domains: wire.domains.clone(),
				may_manage_dns: wire.may_manage_dns,
				may_manage_tls: wire.may_manage_tls,
				paused: wire.paused,
				registered_names: wire.registered_names.clone(),
				certificates: wire.certificates.clone(),
			}],
		}
	}

	/// Whether any application on this machine could obtain a certificate for
	/// `name`.
	///
	/// The union is what this side asks on: nothing here knows which application
	/// a Caddy site belongs to, so it errs towards asking and lets canopy —
	/// which resolves the application from the name — refuse what it must.
	///
	/// spec: NAM#machines-hosting-several-applications
	pub fn may_certify(&self, name: &str) -> bool {
		self.applications.iter().any(|app| app.may_certify(name))
	}

	/// Whether any application holds the TLS grant at all, pause aside.
	///
	/// What a check skips on: a server that may not obtain certificates is not
	/// failing to, it is not trying.
	pub fn holds_tls_grant(&self) -> bool {
		self.applications.iter().any(|app| app.may_manage_tls)
	}

	/// Whether any application may publish DNS records.
	pub fn holds_dns_grant(&self) -> bool {
		self.applications.iter().any(|app| app.may_manage_dns)
	}

	/// Whether every application canopy answered for is paused.
	///
	/// A machine with no applications is not paused: there is nothing to pause.
	pub fn fully_paused(&self) -> bool {
		!self.applications.is_empty() && self.applications.iter().all(|app| app.paused)
	}

	/// Whether `name` sits within some application's domains, grants aside.
	///
	/// What the delivery endpoint declines on: a name outside the domains the
	/// group controls is not this server's to serve.
	pub fn covers(&self, name: &str) -> bool {
		self.applications.iter().any(|app| app.covers(name))
	}

	/// The application matching `type_slug`, for a reporter filing per
	/// application.
	///
	/// A machine canopy answered for in its top-level fields hosts one
	/// application, and that entry is the answer whatever the reporter calls it
	/// — so it matches any type asked for.
	///
	/// spec: CHK-CCO#which-names-it-grades
	pub fn for_type(&self, type_slug: &str) -> Option<&AppEntitlement> {
		self.applications
			.iter()
			.find(|app| app.type_slug.as_deref() == Some(type_slug))
			.or_else(|| self.applications.iter().find(|app| app.type_slug.is_none()))
	}

	/// Every domain any application on this machine controls.
	pub fn domains(&self) -> Vec<String> {
		let mut out: Vec<String> = self
			.applications
			.iter()
			.flat_map(|app| app.domains.iter().cloned())
			.collect();
		out.sort_unstable();
		out.dedup();
		out
	}

	/// Every name any application has registered addresses for.
	pub fn registered_names(&self) -> Vec<String> {
		let mut out: Vec<String> = self
			.applications
			.iter()
			.flat_map(|app| app.registered_names.iter().cloned())
			.collect();
		out.sort_unstable();
		out.dedup();
		out
	}

	/// What canopy holds for `name`, from whichever application declares it.
	pub fn certificate(&self, name: &str) -> Option<&HeldCertificate> {
		self.applications
			.iter()
			.find_map(|app| app.certificate(name))
	}

	/// Every certificate canopy holds for this machine, across its applications.
	pub fn certificates(&self) -> Vec<&HeldCertificate> {
		self.applications
			.iter()
			.flat_map(|app| app.certificates.iter())
			.collect()
	}
}

fn from_application(app: &ApplicationEntitlements) -> AppEntitlement {
	AppEntitlement {
		type_slug: Some(app.type_.to_string()),
		domains: app.domains.clone(),
		may_manage_dns: app.may_manage_dns,
		may_manage_tls: app.may_manage_tls,
		paused: app.paused,
		registered_names: app.registered_names.clone(),
		certificates: app.certificates.clone(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn held(name: &str) -> HeldCertificate {
		HeldCertificate::builder()
			.key_fingerprint("ff".to_string())
			.key_must_be_replaced(false)
			.name(name.to_string())
			.revoked(false)
			.usable(true)
			.build()
	}

	fn flat(domains: &[&str], tls: bool, paused: bool) -> Entitlements {
		Entitlements::builder()
			.applications(vec![])
			.certificates(vec![])
			.domains(domains.iter().map(|d| (*d).to_string()).collect::<Vec<_>>())
			.may_manage_dns(false)
			.may_manage_tls(tls)
			.paused(paused)
			.registered_names(vec![])
			.build()
	}

	#[test]
	fn a_server_with_no_grants_gets_an_empty_answer_not_an_error() {
		// Asking what one may do is not a privileged act, so nothing about the
		// empty answer reads as a failure — and it carries no application, so
		// nothing files a result against one that is not there.
		let e = Entitlement::from_wire(&flat(&[], false, false));
		assert!(e.applications.is_empty());
		assert!(!e.holds_tls_grant());
		assert!(!e.may_certify("app.example.com"));
		assert!(!e.fully_paused());
	}

	#[test]
	fn a_single_application_machine_is_answered_in_the_flat_fields() {
		let e = Entitlement::from_wire(&flat(&["example.com"], true, false));
		assert_eq!(e.applications.len(), 1);
		assert!(e.may_certify("app.example.com"));
		assert!(!e.may_certify("app.elsewhere.test"));
	}

	#[test]
	fn a_name_outside_the_groups_domains_is_not_acted_on() {
		let e = Entitlement::from_wire(&flat(&["example.com"], true, false));
		assert!(!e.covers("app.elsewhere.test"));
		assert!(!e.may_certify("app.elsewhere.test"));
	}

	#[test]
	fn a_paused_server_asks_for_nothing_while_it_is_paused() {
		let e = Entitlement::from_wire(&flat(&["example.com"], true, true));
		assert!(e.fully_paused());
		assert!(!e.may_certify("app.example.com"));
		// It still holds the grant, which is what keeps the pause distinct from
		// a withdrawal for anything reading the two apart.
		assert!(e.holds_tls_grant());
		// And what it collected is still within reach, which is what the
		// delivery endpoint turns on.
		assert!(e.covers("app.example.com"));
	}

	fn applications() -> Entitlements {
		let one = ApplicationEntitlements::builder()
			.certificates(vec![held("a.one.test")])
			.domains(vec!["one.test".to_string()])
			.may_manage_dns(false)
			.may_manage_tls(true)
			.paused(false)
			.registered_names(vec!["a.one.test".to_string()])
			.type_("tamanu-central".parse().unwrap())
			.build();
		let two = ApplicationEntitlements::builder()
			.certificates(vec![])
			.domains(vec!["two.test".to_string()])
			.may_manage_dns(true)
			.may_manage_tls(false)
			.paused(false)
			.registered_names(vec![])
			.type_("tamanu-facility".parse().unwrap())
			.build();
		Entitlements::builder()
			.applications(vec![one, two])
			.certificates(vec![])
			.domains(vec![])
			.may_manage_dns(false)
			.may_manage_tls(false)
			.paused(false)
			.registered_names(vec![])
			.build()
	}

	#[test]
	fn a_machine_with_an_applications_list_acts_on_their_union() {
		// Nothing on this side ties a Caddy site to an application, so the ask
		// is the union: a name any application could act on is asked about.
		let e = Entitlement::from_wire(&applications());
		assert_eq!(e.applications.len(), 2);
		assert!(e.may_certify("a.one.test"));
		// two.test holds DNS but not TLS, so it is covered but not certifiable.
		assert!(e.covers("b.two.test"));
		assert!(!e.may_certify("b.two.test"));
		assert!(e.holds_tls_grant());
		assert!(e.holds_dns_grant());
		assert_eq!(e.domains(), vec!["one.test", "two.test"]);
	}

	#[test]
	fn what_is_held_is_still_attributed_per_application() {
		// Asking as the machine and reporting per application sit together: the
		// answer says which application declares each name.
		let e = Entitlement::from_wire(&applications());
		let central = e.for_type("tamanu-central").unwrap();
		assert!(central.covers("a.one.test"));
		assert!(!central.covers("b.two.test"));
		assert!(central.certificate("a.one.test").is_some());

		let facility = e.for_type("tamanu-facility").unwrap();
		assert!(facility.certificate("a.one.test").is_none());
		assert!(!facility.may_manage_tls);
	}

	#[test]
	fn a_single_application_answer_matches_whatever_type_asks_for_it() {
		// There is one application and the flat answer is about it, so a
		// reporter running for that application finds its entry without canopy
		// having named a type.
		let e = Entitlement::from_wire(&flat(&["example.com"], true, false));
		assert!(e.for_type("tamanu-central").is_some());
		assert!(e.for_type("tamanu-facility").is_some());
	}

	#[test]
	fn one_application_paused_leaves_the_others_acting() {
		let mut e = Entitlement::from_wire(&applications());
		e.applications[0].paused = true;
		assert!(!e.fully_paused());
		assert!(!e.may_certify("a.one.test"));
		assert!(e.for_type("tamanu-facility").unwrap().may_manage_dns);
	}
}
