use bestool_tamanu::ApiServerKind;

use super::CheckContext;
use crate::doctor::check::Check;

pub async fn run(ctx: CheckContext) -> Check {
	let kind = match ctx.kind {
		ApiServerKind::Facility => "facility",
		ApiServerKind::Central => "central",
	};
	let summary = format!(
		"Tamanu {} at {} ({kind})",
		ctx.tamanu_version,
		ctx.tamanu_root.display()
	);
	Check::pass("tamanu_found", summary)
		.with_detail("version", ctx.tamanu_version.to_string())
		.with_detail("kind", kind)
}
