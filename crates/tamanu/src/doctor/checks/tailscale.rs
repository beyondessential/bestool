use super::CheckContext;
use crate::{doctor::check::Check, server_info::get_tailscale_info};

pub async fn run(_ctx: CheckContext) -> Check {
	let (ip, name) = get_tailscale_info();
	match (ip, name) {
		(Some(ip), Some(name)) => Check::pass("tailscale", format!("{name} ({ip})"))
			.with_detail("ip", ip)
			.with_detail("name", name)
			.with_detail("online", true),
		(Some(ip), None) => Check::warning(
			"tailscale",
			format!("partial tailscale info ({ip})"),
			"DNS name unavailable",
		)
		.with_detail("ip", ip)
		.with_detail("online", true),
		_ => Check::pass("tailscale", "tailscale not present").with_detail("online", false),
	}
}
