use super::CheckContext;
use crate::doctor::check::Check;

pub async fn run(ctx: CheckContext) -> Check {
	let Some(client) = ctx.db.as_deref() else {
		return Check::fail("db_version", "no DB connection", "db_connect failed");
	};

	match client.query_one("SELECT version()", &[]).await {
		Ok(row) => match row.try_get::<_, String>(0) {
			Ok(v) => Check::pass("db_version", v.clone()).with_detail("pg_version", v),
			Err(err) => Check::fail("db_version", "row decode failed", err.to_string()),
		},
		Err(err) => Check::fail("db_version", "SELECT version() failed", err.to_string()),
	}
}
