#[test]
fn cli_tests() {
	trycmd::TestCases::new()
		.env("BESTOOL_TIMELESS", "1")
		.case("tests/cmd/*.toml");
}
