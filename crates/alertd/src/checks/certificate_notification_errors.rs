//! Certificate notifications that errored within the lookback window.

use super::{AppCx, util::tiered_rows_check};
use crate::check::Check;

const NAME: &str = "certificate_notification_errors";
const SQL: &str = "SELECT * FROM certificate_notifications \
	WHERE status = 'Error' AND created_at > $1 ORDER BY created_at DESC";

// Lookback window for recent-error checks.
const LOOKBACK_HOURS: i64 = 1;

pub async fn run(ctx: AppCx) -> Check {
	let Some(client) = ctx.db().await else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	tiered_rows_check(
		&client,
		"certificate_notification_errors",
		"no recent certificate notification errors",
		"certificate notification errors: ",
		SQL,
		LOOKBACK_HOURS,
		1,
		10,
	)
	.await
}

#[cfg(test)]
mod tests {
	use crate::checks::test_support::{central_ctx, facility_ctx};

	#[tokio::test]
	async fn runs_against_central() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, "certificate_notification_errors");
	}

	#[tokio::test]
	async fn skips_on_facility() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
