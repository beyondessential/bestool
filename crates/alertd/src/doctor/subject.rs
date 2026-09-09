//! What a check reports for, and which checks a subject admits.
//!
//! spec: SUBJ

use bestool_tamanu::ApiServerKind;

/// An application a sweep can report for.
///
/// The wire type is an open set, so this enumerates only what bestool itself
/// reports from a host: its one Tamanu deployment, or — on a host with no
/// Tamanu but a plain `DATABASE_URL` — the Postgres that URL points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApplicationKind {
	TamanuCentral,
	TamanuFacility,
	/// A Postgres reached through the generic `DATABASE_URL` fallback, with no
	/// Tamanu on the host. The generic database checks are about it, so it is
	/// an application in its own right rather than a nameless database hanging
	/// off the machine.
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

	/// The key this application is reported under.
	///
	/// An agent on a machine reports one application per type, so the type is
	/// enough to tell them apart, and prefixing it keeps the key legible on the
	/// wire. The `host-` prefix is what the substrate will supply once it can
	/// drive sweeps for applications elsewhere.
	pub fn key(self) -> String {
		format!("host-{}", self.type_slug())
	}

	/// Whether this application has a database the generic database checks can
	/// run against. Every kind does today; stated rather than assumed so a
	/// database-less application added later does not silently inherit them.
	pub fn has_database(self) -> bool {
		match self {
			Self::TamanuCentral | Self::TamanuFacility | Self::Postgres => true,
		}
	}

	/// Whether this application is a Tamanu deployment.
	pub fn is_tamanu(self) -> bool {
		matches!(self, Self::TamanuCentral | Self::TamanuFacility)
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Subject {
	Machine,
	Application(ApplicationKind),
}

impl Subject {
	/// How this subject is written when qualifying a check name.
	pub fn slug(self) -> &'static str {
		match self {
			Self::Machine => "machine",
			Self::Application(kind) => kind.type_slug(),
		}
	}

	/// `subject:name`, the form a check is named and selected by.
	///
	/// A name identifies a check only together with its subject, so the two are
	/// never written apart.
	pub fn qualify(self, name: &str) -> String {
		format!("{}:{name}", self.slug())
	}

	/// The subject a [`Self::slug`] names, or `None` for one bestool does not
	/// report.
	pub fn from_slug(slug: &str) -> Option<Self> {
		if slug == "machine" {
			return Some(Self::Machine);
		}
		ApplicationKind::ALL
			.into_iter()
			.find(|kind| kind.type_slug() == slug)
			.map(Self::Application)
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
	/// Any application with a database, a bare Postgres included.
	Database,
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
	pub fn admits(self, subject: Subject) -> bool {
		match (self, subject) {
			(Self::Machine, Subject::Machine) => true,
			(Self::Machine, Subject::Application(_)) | (_, Subject::Machine) => false,
			(Self::Database, Subject::Application(kind)) => kind.has_database(),
			(Self::Tamanu, Subject::Application(kind)) => kind.is_tamanu(),
			(Self::Central, Subject::Application(kind)) => kind == ApplicationKind::TamanuCentral,
			(Self::Facility, Subject::Application(kind)) => kind == ApplicationKind::TamanuFacility,
		}
	}

	/// Every subject a check of this scope could ever report for.
	///
	/// Drives name validation, which answers from the registry rather than from
	/// what this host happens to run, so the same invocation is an error for the
	/// same reason everywhere.
	pub fn possible_subjects(self) -> Vec<Subject> {
		match self {
			Self::Machine => vec![Subject::Machine],
			Self::Database => vec![
				Subject::Application(ApplicationKind::TamanuCentral),
				Subject::Application(ApplicationKind::TamanuFacility),
				Subject::Application(ApplicationKind::Postgres),
			],
			Self::Tamanu => vec![
				Subject::Application(ApplicationKind::TamanuCentral),
				Subject::Application(ApplicationKind::TamanuFacility),
			],
			Self::Central => vec![Subject::Application(ApplicationKind::TamanuCentral)],
			Self::Facility => vec![Subject::Application(ApplicationKind::TamanuFacility)],
		}
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
			CheckScope::Database,
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
	fn tamanu_scope_excludes_a_bare_postgres() {
		// The generic-database host has no Tamanu, so a check that reads Tamanu's
		// tables must not be filed against the Postgres standing in for it.
		let postgres = Subject::Application(ApplicationKind::Postgres);
		assert!(!CheckScope::Tamanu.admits(postgres));
		assert!(CheckScope::Database.admits(postgres));
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
			CheckScope::Database,
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
