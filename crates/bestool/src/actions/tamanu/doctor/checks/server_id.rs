use super::CheckContext;
use crate::actions::tamanu::{doctor::check::Check, server_info::get_or_create_server_id};

pub async fn run(ctx: CheckContext) -> Check {
	let Some(client) = ctx.db.as_deref() else {
		return Check::fail("server_id", "no DB connection", "db_connect failed");
	};

	match get_or_create_server_id(client).await {
		Ok(id) => Check::pass("server_id", format!("metaServerId: {id}"))
			.with_detail("server_id", id),
		Err(err) => Check::fail("server_id", "lookup failed", err.to_string()),
	}
}
