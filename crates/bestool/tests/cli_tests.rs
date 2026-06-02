#[test]
fn cli_tests() {
	trycmd::TestCases::new()
		.env("BESTOOL_MOCK_TIME", "1")
		.env("NO_COLOR", "1")
		.case("tests/cmd/*.toml")
		.run();
}
