use super::CheckContext;
use crate::{doctor::check::Check, server_info::get_or_create_server_id};

pub async fn run(ctx: CheckContext) -> Check {
	// Pass the DB through optionally: an already-provisioned host has the id
	// cached at the standard file path and can answer without a DB, which is
	// what lets the canopy push keep working when postgres is down.
	match get_or_create_server_id(ctx.db.as_deref()).await {
		Ok(id) => {
			Check::pass("server_id", format!("metaServerId: {id}")).with_detail("server_id", id)
		}
		Err(err) => Check::fail("server_id", "lookup failed", err.to_string()),
	}
}
