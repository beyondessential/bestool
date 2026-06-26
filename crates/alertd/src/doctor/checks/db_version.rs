use super::{CheckContext, query_error_check};
use crate::doctor::check::Check;

pub async fn run(ctx: CheckContext) -> Check {
	let Some(client) = ctx.db.as_deref() else {
		return Check::fail("db_version", "no DB connection", "db_connect failed");
	};

	match client.query_one("SELECT version()", &[]).await {
		Ok(row) => match row.try_get::<_, String>(0) {
			Ok(v) => Check::pass("db_version", v.clone()).with_detail("pg_version", v),
			// version() returned a row that didn't decode as text — a check
			// mismatch, not a database fault. Mirrors how `query_error_check`
			// treats 42xxx schema errors.
			Err(err) => Check::broken("db_version", "row decode failed", err.to_string()),
		},
		Err(err) => query_error_check("db_version", &err),
	}
}
