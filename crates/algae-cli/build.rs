use std::path::Path;

fn main() {
	if std::env::var("DOCS_RS").is_ok() {
		return;
	}

	windows_exe_info::versioninfo::link_cargo_env();
	windows_exe_info::manifest("windows-manifest.xml");
}
