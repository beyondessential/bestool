//! Whether Tamanu can provision its reporting database roles.
//!
//! From 2.60 Tamanu owns `tamanu_reporting` and `tamanu_raw`: it creates them at
//! startup, sets their passwords, and grants them the reporting schema. Doing
//! that needs CREATEROLE, and on Postgres 16+ altering a role someone else
//! created also needs ADMIN on that role.
//!
//! Without either, `initReporting` throws, the server logs it and carries on
//! with reporting switched off. Nothing else says so: database-defined reports
//! error, the DB Schema dropdown vanishes, and report imports fail with an
//! unrelated-looking validation message. Catching it here means a deployment
//! that will break on upgrade is visible before someone runs the upgrade.

use node_semver::Version;

use super::{CheckContext, query_error_check};
use crate::doctor::check::Check;

const NAME: &str = "reporting_roles";

/// The release where Tamanu took ownership of the reporting roles.
const OWNS_ROLES_FROM: (u64, u64) = (2, 60);

const SQL: &str = "SELECT \
	r.rolcreaterole AS has_createrole, \
	r.rolsuper AS is_superuser, \
	(SELECT count(*) FROM pg_roles x \
		WHERE x.rolname IN ('tamanu_reporting', 'tamanu_raw')) AS roles_present, \
	(SELECT count(*) FROM pg_auth_members m \
		JOIN pg_roles t ON t.oid = m.roleid \
		WHERE t.rolname IN ('tamanu_reporting', 'tamanu_raw') \
		AND m.member = r.oid AND m.admin_option) AS roles_administrable \
	FROM pg_roles r WHERE r.rolname = current_user";

pub async fn run(ctx: CheckContext) -> Check {
	if is_unknown_version(&ctx.tamanu_version) {
		return Check::skip(
			NAME,
			"Tamanu version unknown",
			"no install on disk and no recorded currentVersion, so the check can't tell whether \
			 this release provisions its own reporting roles",
		);
	}

	if !owns_reporting_roles(&ctx.tamanu_version) {
		return Check::skip(
			NAME,
			format!(
				"not applicable before {}.{}",
				OWNS_ROLES_FROM.0, OWNS_ROLES_FROM.1
			),
			"reporting roles are provisioned outside Tamanu on this release",
		);
	}

	let Some(client) = ctx.db.as_ref() else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let row = match client.query_opt(SQL, &[]).await {
		Ok(Some(row)) => row,
		Ok(None) => {
			return Check::broken(
				NAME,
				"connected role not in pg_roles",
				"current_user has no pg_roles row, which should not be possible",
			);
		}
		Err(err) => return query_error_check(NAME, &err),
	};

	let state = RoleState {
		has_createrole: row.try_get("has_createrole").unwrap_or(false),
		is_superuser: row.try_get("is_superuser").unwrap_or(false),
		present: row.try_get("roles_present").unwrap_or(0),
		administrable: row.try_get("roles_administrable").unwrap_or(0),
	};

	verdict(&state)
		.with_detail("has_createrole", state.has_createrole)
		.with_detail("roles_present", state.present)
		.with_detail("roles_administrable", state.administrable)
}

#[derive(Debug, Clone, Copy)]
struct RoleState {
	has_createrole: bool,
	is_superuser: bool,
	/// How many of the two reporting roles already exist.
	present: i64,
	/// How many of those the connected role holds ADMIN on.
	administrable: i64,
}

fn verdict(state: &RoleState) -> Check {
	if state.is_superuser {
		return Check::pass(NAME, "reporting roles manageable");
	}

	if !state.has_createrole {
		return Check::fail(
			NAME,
			"cannot provision reporting roles",
			"the Tamanu database role has no CREATEROLE, so initReporting fails at startup and \
			 reporting is silently unavailable. Fix with: ALTER ROLE <tamanu_db_user> WITH CREATEROLE;",
		);
	}

	if state.administrable < state.present {
		return Check::fail(
			NAME,
			"reporting roles not administrable",
			"the reporting roles exist but were created by someone else, and on Postgres 16+ \
			 altering them needs ADMIN. initReporting fails at startup and reporting is silently \
			 unavailable. Fix with: GRANT tamanu_reporting, tamanu_raw TO <tamanu_db_user> WITH ADMIN OPTION;",
		);
	}

	Check::pass(NAME, "reporting roles manageable")
}

fn is_unknown_version(version: &Version) -> bool {
	version.major == 0 && version.minor == 0 && version.patch == 0
}

fn owns_reporting_roles(version: &Version) -> bool {
	(version.major, version.minor) >= OWNS_ROLES_FROM
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::doctor::check::CheckStatus;
	use crate::doctor::checks::test_support::central_ctx;

	fn version(s: &str) -> Version {
		Version::parse(s).unwrap()
	}

	#[test]
	fn applies_from_2_60_onwards() {
		assert!(!owns_reporting_roles(&version("2.59.12")));
		assert!(owns_reporting_roles(&version("2.60.0")));
		assert!(owns_reporting_roles(&version("2.63.1")));
		assert!(owns_reporting_roles(&version("3.0.0")));
	}

	#[test]
	fn superuser_passes_whatever_else_is_true() {
		let check = verdict(&RoleState {
			has_createrole: false,
			is_superuser: true,
			present: 0,
			administrable: 0,
		});
		assert!(matches!(check.status, CheckStatus::Pass));
	}

	#[test]
	fn missing_createrole_fails() {
		let check = verdict(&RoleState {
			has_createrole: false,
			is_superuser: false,
			present: 2,
			administrable: 2,
		});
		assert!(matches!(check.status, CheckStatus::Fail(_)));
	}

	#[test]
	fn hand_created_roles_fail() {
		let check = verdict(&RoleState {
			has_createrole: true,
			is_superuser: false,
			present: 2,
			administrable: 0,
		});
		assert!(matches!(check.status, CheckStatus::Fail(_)));
	}

	#[test]
	fn createrole_with_no_roles_yet_passes() {
		let check = verdict(&RoleState {
			has_createrole: true,
			is_superuser: false,
			present: 0,
			administrable: 0,
		});
		assert!(matches!(check.status, CheckStatus::Pass));
	}

	#[test]
	fn tamanu_created_roles_pass() {
		let check = verdict(&RoleState {
			has_createrole: true,
			is_superuser: false,
			present: 2,
			administrable: 2,
		});
		assert!(matches!(check.status, CheckStatus::Pass));
	}

	#[tokio::test]
	async fn skips_on_unknown_version() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert!(check.status.is_skip());
	}

	#[tokio::test]
	async fn runs_against_central() {
		let Some(mut ctx) = central_ctx().await else {
			return;
		};
		ctx.tamanu_version = version("2.60.0");
		let check = super::run(ctx).await;
		assert_eq!(check.name, NAME);
		assert!(matches!(
			check.status,
			CheckStatus::Pass | CheckStatus::Fail(_)
		));
	}
}
