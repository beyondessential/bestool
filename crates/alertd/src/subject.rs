//! What a check reports for, and which checks a subject admits.
//!
//! spec: SUBJ

use bestool_tamanu::ApiServerKind;

/// An application a sweep can report for.
///
/// The wire type is an open set, so this enumerates only what bestool itself
/// reports from a host: its Tamanu deployment, and the Postgres installation
/// under it. A machine commonly has both, and they are reported separately —
/// "Tamanu as seen through its database" and "the health of Postgres itself"
/// are different questions about different things.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApplicationKind {
	TamanuCentral,
	TamanuFacility,
	/// The Postgres installation on this machine.
	///
	/// Its own application, not a part of whatever uses it: the checks that
	/// grade a cluster's tuning, its checksums, its version and its
	/// reachability are about the server itself, and hold whether one Tamanu
	/// uses it, several do, or none does.
	Postgres,
}

impl ApplicationKind {
	/// Every kind bestool can report, for lookups that go from a wire key back
	/// to the kind that produced it.
	pub const ALL: [Self; 3] = [Self::TamanuCentral, Self::TamanuFacility, Self::Postgres];

	/// The application type as canopy names it.
	pub fn type_slug(self) -> &'static str {
		match self {
			Self::TamanuCentral => "tamanu-central",
			Self::TamanuFacility => "tamanu-facility",
			Self::Postgres => "postgres",
		}
	}

	/// Whether this application is a Tamanu deployment.
	pub fn is_tamanu(self) -> bool {
		matches!(self, Self::TamanuCentral | Self::TamanuFacility)
	}
}

/// One application instance: what it is, and which one of its kind.
///
/// A machine runs at most one Tamanu of each role but may run several Postgres
/// clusters, so a kind alone does not identify an application and the key
/// travels with it.
///
/// spec: SUBJ
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApplicationRef {
	pub kind: ApplicationKind,
	/// The key this instance is reported under, unique on its machine and
	/// stable across pushes.
	pub key: String,
}

impl ApplicationRef {
	/// The single Tamanu of this role on the machine.
	///
	/// One per role, so the type is enough to tell them apart, and the `host-`
	/// prefix keeps the key legible on the wire.
	pub fn tamanu(kind: ApplicationKind) -> Self {
		Self {
			kind,
			key: format!("host-{}", kind.type_slug()),
		}
	}

	/// A Postgres cluster running on this machine, identified by the port it
	/// answers on.
	///
	/// The port, never the version: an in-place major upgrade must not read as
	/// one application stopping and another starting. It is also the one
	/// identifier every connection form carries, a Unix socket being named
	/// `.s.PGSQL.<port>`.
	pub fn local_postgres(port: u16) -> Self {
		Self {
			kind: ApplicationKind::Postgres,
			key: format!("host-postgres-{port}"),
		}
	}

	/// A Postgres cluster reached at an address that is not this machine.
	///
	/// Keyed apart from the local form so the `host-` prefix never claims the
	/// machine hosts something it does not.
	pub fn remote_postgres(host: &str, port: u16) -> Self {
		Self {
			kind: ApplicationKind::Postgres,
			key: format!("remote-{host}-{port}"),
		}
	}
}

impl From<ApiServerKind> for ApplicationKind {
	fn from(kind: ApiServerKind) -> Self {
		match kind {
			ApiServerKind::Central => Self::TamanuCentral,
			ApiServerKind::Facility => Self::TamanuFacility,
		}
	}
}

/// One thing a sweep reports for: the machine, or an application on it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Subject {
	Machine,
	Application(ApplicationRef),
}

impl Subject {
	/// How this subject is written when qualifying a check name.
	///
	/// The application's *type*, not its key: selection names a check on a kind
	/// of subject, so `postgres:connect` reaches that check on every cluster
	/// rather than needing one invocation per port. Which instance a result came
	/// from is carried by its key, not by its name.
	pub fn slug(&self) -> &'static str {
		match self {
			Self::Machine => "machine",
			Self::Application(app) => app.kind.type_slug(),
		}
	}

	/// `subject:name`, the form a check is named and selected by.
	///
	/// A name identifies a check only together with its subject, so the two are
	/// never written apart.
	pub fn qualify(&self, name: &str) -> String {
		format!("{}:{name}", self.slug())
	}

	/// How one result is identified for display, by *instance* rather than by
	/// type.
	///
	/// Two Postgres clusters both answer to the selection name
	/// `postgres:connect`, so the type cannot tell their results apart; the key
	/// can. The `host-` prefix is a wire concern and is dropped here.
	pub fn identify(&self, name: &str) -> String {
		match self.key() {
			None => format!("machine:{name}"),
			Some(key) => format!("{}:{name}", key.strip_prefix("host-").unwrap_or(key)),
		}
	}

	/// The application key this subject reports under, if it is an application.
	pub fn key(&self) -> Option<&str> {
		match self {
			Self::Machine => None,
			Self::Application(app) => Some(&app.key),
		}
	}

	/// The application kind this subject is, if it is an application.
	pub fn kind(&self) -> Option<ApplicationKind> {
		match self {
			Self::Machine => None,
			Self::Application(app) => Some(app.kind),
		}
	}
}

/// Which Tamanu deployments a check reports for.
///
/// Carried inside the registry's Tamanu arm rather than beside the runner, so a
/// runner paired with a scope that cannot describe its context is not
/// representable. There is no machine variant and no Postgres one: each of
/// those is an arm of its own, whose subject needs no narrowing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TamanuScope {
	/// Any Tamanu deployment, central or facility.
	Any,
	/// Tamanu central only.
	Central,
	/// Tamanu facility only.
	Facility,
}

impl TamanuScope {
	/// Whether a check of this scope reports for `app`.
	///
	/// A check that does not is absent from that application's report rather
	/// than reported for it as skipped.
	pub fn admits(self, app: &ApplicationRef) -> bool {
		match self {
			Self::Any => app.kind.is_tamanu(),
			Self::Central => app.kind == ApplicationKind::TamanuCentral,
			Self::Facility => app.kind == ApplicationKind::TamanuFacility,
		}
	}

	/// Every application kind a check of this scope could report for.
	pub fn possible_kinds(self) -> Vec<ApplicationKind> {
		match self {
			Self::Any => vec![
				ApplicationKind::TamanuCentral,
				ApplicationKind::TamanuFacility,
			],
			Self::Central => vec![ApplicationKind::TamanuCentral],
			Self::Facility => vec![ApplicationKind::TamanuFacility],
		}
	}

	/// Every `subject:name` slug a check of this scope could be selected by.
	pub fn possible_slugs(self) -> Vec<&'static str> {
		self.possible_kinds()
			.into_iter()
			.map(ApplicationKind::type_slug)
			.collect()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn app_ref(kind: ApplicationKind) -> ApplicationRef {
		match kind {
			ApplicationKind::Postgres => ApplicationRef::local_postgres(5432),
			other => ApplicationRef::tamanu(other),
		}
	}

	fn app(kind: ApplicationKind) -> Subject {
		Subject::Application(app_ref(kind))
	}

	#[test]
	fn no_tamanu_scope_admits_a_cluster() {
		// A check reading Tamanu's tables is not about the server underneath it.
		// The reverse needs no scope to say so: a Postgres check is its own arm.
		let postgres = app_ref(ApplicationKind::Postgres);
		for scope in [
			TamanuScope::Any,
			TamanuScope::Central,
			TamanuScope::Facility,
		] {
			assert!(!scope.admits(&postgres), "{scope:?} admitted a cluster");
		}
		assert!(TamanuScope::Any.admits(&app_ref(ApplicationKind::TamanuCentral)));
	}

	#[test]
	fn kind_scopes_are_mutually_exclusive() {
		let central = app_ref(ApplicationKind::TamanuCentral);
		let facility = app_ref(ApplicationKind::TamanuFacility);
		assert!(TamanuScope::Central.admits(&central));
		assert!(!TamanuScope::Central.admits(&facility));
		assert!(TamanuScope::Facility.admits(&facility));
		assert!(!TamanuScope::Facility.admits(&central));
	}

	#[test]
	fn possible_kinds_agree_with_admits() {
		for scope in [
			TamanuScope::Any,
			TamanuScope::Central,
			TamanuScope::Facility,
		] {
			let possible = scope.possible_kinds();
			for kind in ApplicationKind::ALL {
				assert_eq!(
					possible.contains(&kind),
					scope.admits(&app_ref(kind)),
					"{scope:?} disagrees with itself about {kind:?}",
				);
			}
		}
	}

	#[test]
	fn qualified_names_use_the_type_not_the_key() {
		// Selection names a check on a kind of subject, so one invocation reaches
		// the check on every cluster rather than needing one per port.
		assert_eq!(Subject::Machine.qualify("disk_free"), "machine:disk_free");
		assert_eq!(
			app(ApplicationKind::TamanuCentral).qualify("migrations"),
			"tamanu-central:migrations",
		);
		let a = Subject::Application(ApplicationRef::local_postgres(5432));
		let b = Subject::Application(ApplicationRef::local_postgres(5433));
		assert_eq!(a.qualify("connect"), b.qualify("connect"));
		assert_ne!(a.key(), b.key());
	}

	#[test]
	fn a_cluster_is_keyed_by_port_not_version() {
		// An in-place major upgrade changes the version but not the port, so the
		// key holds and canopy does not read it as a different application.
		assert_eq!(
			ApplicationRef::local_postgres(5432).key,
			"host-postgres-5432"
		);
		assert_ne!(
			ApplicationRef::local_postgres(5432).key,
			ApplicationRef::local_postgres(5433).key,
		);
	}

	#[test]
	fn a_remote_cluster_is_keyed_apart_from_a_local_one() {
		// The `host-` prefix must never claim the machine hosts something it
		// does not.
		let remote = ApplicationRef::remote_postgres("db.example.com", 5432);
		assert_eq!(remote.key, "remote-db.example.com-5432");
		assert_ne!(remote.key, ApplicationRef::local_postgres(5432).key);
	}

	#[test]
	fn tamanu_keys_are_one_per_role() {
		assert_eq!(
			ApplicationRef::tamanu(ApplicationKind::TamanuCentral).key,
			"host-tamanu-central"
		);
		assert_eq!(
			ApplicationRef::tamanu(ApplicationKind::TamanuFacility).key,
			"host-tamanu-facility"
		);
	}
}
