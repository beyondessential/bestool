#[test]
fn cli_tests() {
	trycmd::TestCases::new()
		.env("BESTOOL_TIMELESS", "1")
		.env("RUST_LOG", "warn")
		.env("NO_COLOR", "1")
		.case("tests/cmd/*.toml");
}
