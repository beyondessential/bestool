mod fixture_pg;

use fixture_pg::{init_db, run_db};

#[test]
fn cli_tests() {
	let cases = trycmd::TestCases::new();
	cases
		.env("BESTOOL_MOCK_TIME", "1")
		.env("NO_COLOR", "1")
		.case("tests/cmd/*.toml");

	let handle_res = init_db().and_then(run_db);

	// Ignore tests that depend on Postgres if the Postgres test fixture failed.
	// Add more `cases.skip()` here if any test use Postgres.
	if handle_res.is_err() {
		cases.skip("tests/cmd/alerts.toml");
	}

	cases.run();
}
