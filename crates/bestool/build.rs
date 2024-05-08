use std::path::Path;

fn main() {
	windows_exe_info::versioninfo::link_cargo_env();
	windows_exe_info::manifest(Path::new("windows-manifest.xml"));
	build_data::set_GIT_BRANCH();
	build_data::set_GIT_COMMIT();
	build_data::set_GIT_DIRTY();
	build_data::set_SOURCE_TIMESTAMP();
	build_data::no_debug_rebuilds();
}
