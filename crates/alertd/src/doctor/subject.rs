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

/// Which subjects a check reports for.
///
/// Distinct from the inputs a check needs, which the registry's category
/// decides: `caddyfile_version` reports for the machine but needs the
/// application's version to grade what it finds, so the two axes cross.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckScope {
	/// The machine itself.
	Machine,
	/// The machine's Postgres installation.
	Postgres,
	/// Any Tamanu deployment, central or facility.
	Tamanu,
	/// Tamanu central only.
	Central,
	/// Tamanu facility only.
	Facility,
}

impl CheckScope {
	/// Whether a check of this scope reports for `subject`.
	///
	/// A check that does not is absent from that subject's report rather than
	/// reported for it as skipped.
	pub fn admits(self, subject: &Subject) -> bool {
		let Some(kind) = subject.kind() else {
			return self == Self::Machine;
		};
		match self {
			Self::Machine => false,
			Self::Postgres => kind == ApplicationKind::Postgres,
			Self::Tamanu => kind.is_tamanu(),
			Self::Central => kind == ApplicationKind::TamanuCentral,
			Self::Facility => kind == ApplicationKind::TamanuFacility,
		}
	}

	/// Every application kind a check of this scope could report for. Empty for
	/// a machine check.
	pub fn possible_kinds(self) -> Vec<ApplicationKind> {
		match self {
			Self::Machine => Vec::new(),
			Self::Postgres => vec![ApplicationKind::Postgres],
			Self::Tamanu => vec![
				ApplicationKind::TamanuCentral,
				ApplicationKind::TamanuFacility,
			],
			Self::Central => vec![ApplicationKind::TamanuCentral],
			Self::Facility => vec![ApplicationKind::TamanuFacility],
		}
	}

	/// Every `subject:name` slug a check of this scope could be selected by.
	pub fn possible_slugs(self) -> Vec<&'static str> {
		if self == Self::Machine {
			return vec!["machine"];
		}
		self.possible_kinds()
			.into_iter()
			.map(ApplicationKind::type_slug)
			.collect()
	}

}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn machine_scope_admits_only_the_machine() {
		assert!(CheckScope::Machine.admits(Subject::Machine));
		for kind in [
			ApplicationKind::TamanuCentral,
			ApplicationKind::TamanuFacility,
			ApplicationKind::Postgres,
		] {
			assert!(!CheckScope::Machine.admits(Subject::Application(kind)));
		}
	}

	#[test]
	fn application_scopes_never_admit_the_machine() {
		for scope in [
			CheckScope::Postgres,
			CheckScope::Tamanu,
			CheckScope::Central,
			CheckScope::Facility,
		] {
			assert!(
				!scope.admits(Subject::Machine),
				"{scope:?} admitted the machine"
			);
		}
	}

	#[test]
	fn postgres_and_tamanu_scopes_do_not_overlap() {
		// A check grading the Postgres server is not about the Tamanu that uses
		// it, and a check reading Tamanu's tables is not about the server.
		let postgres = Subject::Application(ApplicationKind::Postgres);
		let central = Subject::Application(ApplicationKind::TamanuCentral);
		assert!(CheckScope::Postgres.admits(postgres));
		assert!(!CheckScope::Postgres.admits(central));
		assert!(!CheckScope::Tamanu.admits(postgres));
		assert!(CheckScope::Tamanu.admits(central));
	}

	#[test]
	fn kind_scopes_are_mutually_exclusive() {
		let central = Subject::Application(ApplicationKind::TamanuCentral);
		let facility = Subject::Application(ApplicationKind::TamanuFacility);
		assert!(CheckScope::Central.admits(central));
		assert!(!CheckScope::Central.admits(facility));
		assert!(CheckScope::Facility.admits(facility));
		assert!(!CheckScope::Facility.admits(central));
	}

	#[test]
	fn possible_subjects_agree_with_admits() {
		let every = [
			Subject::Machine,
			Subject::Application(ApplicationKind::TamanuCentral),
			Subject::Application(ApplicationKind::TamanuFacility),
			Subject::Application(ApplicationKind::Postgres),
		];
		for scope in [
			CheckScope::Machine,
			CheckScope::Postgres,
			CheckScope::Tamanu,
			CheckScope::Central,
			CheckScope::Facility,
		] {
			let possible = scope.possible_subjects();
			for subject in every {
				assert_eq!(
					possible.contains(&subject),
					scope.admits(subject),
					"{scope:?} disagrees with itself about {subject:?}",
				);
			}
		}
	}

	#[test]
	fn qualified_names_read_as_the_selection_syntax() {
		assert_eq!(Subject::Machine.qualify("disk_free"), "machine:disk_free");
		assert_eq!(
			Subject::Application(ApplicationKind::TamanuCentral).qualify("migrations"),
			"tamanu-central:migrations",
		);
	}

	#[test]
	fn application_keys_are_prefixed_types() {
		assert_eq!(ApplicationKind::TamanuCentral.key(), "host-tamanu-central");
		assert_eq!(ApplicationKind::Postgres.key(), "host-postgres");
	}
}
