//! Patients missing a date of birth or sex.
//!
//! Tamanu's UI treats both as mandatory but the columns are nullable. This
//! check is temporary: it finds out whether any deployment holds such rows
//! before the columns are made NOT NULL. Soft-deleted patients are counted too,
//! as the constraint would apply to them as well.

use super::{TamanuCx, query_error_check, row_error_check};
use crate::Stat;
use crate::check::Check;

const NAME: &str = "patient_mandatory_nulls";

const SQL: &str = "SELECT \
	count(*) FILTER (WHERE date_of_birth IS NULL) AS missing_dob, \
	count(*) FILTER (WHERE sex IS NULL) AS missing_sex, \
	coalesce((array_agg(id::text ORDER BY id) \
		FILTER (WHERE date_of_birth IS NULL OR sex IS NULL))[1:10], '{}') AS sample \
	FROM patients";

pub async fn run(ctx: TamanuCx) -> Check {
	let Some(client) = ctx.db().await else {
		return Check::skip(NAME, "no DB connection", "db unavailable");
	};

	let row = match client.query_one(SQL, &[]).await {
		Ok(r) => r,
		Err(err) => return query_error_check(NAME, &err),
	};

	let (missing_dob, missing_sex, sample) = match (
		row.try_get::<_, i64>("missing_dob"),
		row.try_get::<_, i64>("missing_sex"),
		row.try_get::<_, Vec<String>>("sample"),
	) {
		(Ok(dob), Ok(sex), Ok(sample)) => (dob, sex, sample),
		(Err(err), _, _) | (_, Err(err), _) | (_, _, Err(err)) => {
			return row_error_check(NAME, &err);
		}
	};

	let check = if missing_dob == 0 && missing_sex == 0 {
		Check::pass(NAME, "all patients have a date of birth and sex")
	} else {
		Check::warning(
			NAME,
			format!("patients missing date of birth: {missing_dob}, sex: {missing_sex}"),
			"patient(s) missing mandatory fields",
		)
		.with_detail("sample_patient_ids", sample)
	};

	check
		.with_stat(
			Stat::gauge("missing_date_of_birth", missing_dob as f64)
				.help("Patients with a null date_of_birth"),
		)
		.with_stat(Stat::gauge("missing_sex", missing_sex as f64).help("Patients with a null sex"))
}

#[cfg(test)]
mod tests {
	use crate::check::CheckStatus;
	use crate::checks::test_support::{central_ctx, facility_ctx};

	#[tokio::test]
	async fn runs_against_central() {
		let Some(ctx) = central_ctx().await else {
			return;
		};
		let check = super::run(ctx).await;
		assert_eq!(check.name, "patient_mandatory_nulls");
		assert!(matches!(
			check.status,
			CheckStatus::Pass | CheckStatus::Warning(_)
		));
	}

	#[tokio::test]
	async fn skips_on_facility() {
		let check = super::run(facility_ctx()).await;
		assert!(check.status.is_skip());
	}
}
