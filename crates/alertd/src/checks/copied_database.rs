//! Whether a Tamanu database was copied from another server.
//!
//! spec: CHK-COPY

use bestool_canopy::registration;

use super::{TamanuCx, query_error_check, row_error_check};
use crate::check::Check;

const NAME: &str = "copied_database";
const SQL: &str = "SELECT value FROM local_system_facts WHERE key = 'metaServerId'";

pub async fn run(ctx: TamanuCx) -> Check {
	let machine_server_id = match registration::load().await {
		Ok(reg) => reg.and_then(|reg| reg.server_id),
		Err(err) => {
			return Check::skip(
				NAME,
				"could not read the Canopy registration",
				err.to_string(),
			);
		}
	};
	let Some(machine_server_id) = machine_server_id else {
		return Check::skip(
			NAME,
			"no Canopy server id",
			"this machine has no Canopy server id",
		);
	};

	let Some(client) = ctx.db().await else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};
	let database_server_id: Option<String> = match client.query_opt(SQL, &[]).await {
		Ok(None) => None,
		Ok(Some(row)) => match row.try_get(0) {
			Ok(value) => value,
			Err(err) => return row_error_check(NAME, &err),
		},
		Err(err) => return query_error_check(NAME, &err),
	};

	grade(database_server_id.as_deref(), &machine_server_id)
}

/// spec: CHK-COPY#outcomes
fn grade(database_server_id: Option<&str>, machine_server_id: &str) -> Check {
	let Some(database_server_id) = database_server_id
		.map(str::trim)
		.filter(|id| !id.is_empty())
	else {
		return Check::pass(NAME, "database records no server id");
	};
	let machine_server_id = machine_server_id.trim();
	if database_server_id.eq_ignore_ascii_case(machine_server_id) {
		return Check::pass(NAME, "database belongs to this server");
	}
	Check::fail(
		NAME,
		"database copied from another server",
		"this database records another server's Canopy id, so it still holds that server's identity; \
		 clear it with Tamanu's restore-as-copy procedure",
	)
	.with_detail("databaseServerId", database_server_id)
	.with_detail("machineServerId", machine_server_id)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::check::CheckStatus;

	const MACHINE: &str = "11111111-2222-4333-8444-555555555555";
	const OTHER: &str = "99999999-8888-4777-8666-555555555555";

	#[test]
	fn passes_without_a_recorded_id() {
		assert!(matches!(grade(None, MACHINE).status, CheckStatus::Pass));
		assert!(matches!(grade(Some(""), MACHINE).status, CheckStatus::Pass));
	}

	#[test]
	fn passes_on_this_servers_id() {
		assert!(matches!(
			grade(Some(MACHINE), MACHINE).status,
			CheckStatus::Pass
		));
		let upper = MACHINE.to_uppercase();
		assert!(matches!(
			grade(Some(&upper), MACHINE).status,
			CheckStatus::Pass
		));
	}

	#[test]
	fn fails_on_another_servers_id() {
		let check = grade(Some(OTHER), MACHINE);
		assert!(matches!(check.status, CheckStatus::Fail(_)));
	}
}
