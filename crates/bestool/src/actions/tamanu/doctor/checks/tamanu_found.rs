use super::CheckContext;
use crate::actions::tamanu::doctor::check::Check;

pub async fn run(ctx: CheckContext) -> Check {
	let kind = if ctx.config.is_facility() {
		"facility"
	} else {
		"central"
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
