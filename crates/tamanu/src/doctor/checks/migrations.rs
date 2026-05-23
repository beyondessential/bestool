use super::CheckContext;
use crate::doctor::check::Check;

pub async fn run(ctx: CheckContext) -> Check {
	let Some(client) = ctx.db.as_deref() else {
		return Check::fail("migrations", "no DB connection", "db_connect failed");
	};

	let query = r#"SELECT name FROM "SequelizeMeta" ORDER BY name DESC LIMIT 1"#;
	match client.query_opt(query, &[]).await {
		Ok(Some(row)) => match row.try_get::<_, String>(0) {
			Ok(name) => Check::pass("migrations", format!("last: {name}"))
				.with_detail("last_migration", name),
			Err(err) => Check::fail("migrations", "row decode failed", err.to_string()),
		},
		Ok(None) => Check::warning(
			"migrations",
			"no migrations applied",
			"SequelizeMeta is empty",
		),
		Err(err) => Check::fail("migrations", "query failed", err.to_string()),
	}
}
