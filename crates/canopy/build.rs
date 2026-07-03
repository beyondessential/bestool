//! Generates the `schema` module from canopy's OpenAPI document.
//!
//! The document is fetched live from the running canopy deployment, falling
//! back to the committed snapshot when the network is unavailable (offline or
//! sealed CI builds). Its `components.schemas` are wrapped into a JSON-Schema
//! root document and handed to typify, which emits the wire types. The result
//! is written to `OUT_DIR` and `include!`d by the crate — nothing generated is
//! committed, so canopy adding a field changes no committed source and never
//! triggers a republish.

use std::{env, fs, path::Path, time::Duration};

use schemars::schema::RootSchema;
use serde_json::Value;
use typify::{TypeSpace, TypeSpaceSettings};

const SPEC_URL: &str = "https://meta.tamanu.app/api/openapi.json";
const SNAPSHOT: &str = "openapi.snapshot.json";
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

fn main() {
	println!("cargo:rerun-if-changed=build.rs");
	println!("cargo:rerun-if-changed={SNAPSHOT}");

	let spec_text = fetch_live().unwrap_or_else(|err| {
		println!(
			"cargo:warning=canopy OpenAPI live fetch failed ({err}); using committed snapshot"
		);
		fs::read_to_string(SNAPSHOT).expect("reading canopy OpenAPI snapshot")
	});

	let spec: Value = serde_json::from_str(&spec_text).expect("parsing canopy OpenAPI document");
	let schemas = spec
		.get("components")
		.and_then(|components| components.get("schemas"))
		.and_then(Value::as_object)
		.expect("canopy OpenAPI document has no components.schemas")
		.clone();

	let root = serde_json::json!({
		"$schema": "https://json-schema.org/draft/2020-12/schema",
		"definitions": schemas,
	});
	let root_schema: RootSchema =
		serde_json::from_value(root).expect("building JSON-Schema root from canopy schemas");

	let mut settings = TypeSpaceSettings::default();
	settings.with_struct_builder(false);
	let mut type_space = TypeSpace::new(&settings);
	type_space
		.add_root_schema(root_schema)
		.expect("generating types from canopy schemas");

	let file = syn::parse2(type_space.to_stream()).expect("parsing generated canopy schema tokens");
	let generated = rewrite_types(&prettyplease::unparse(&file));

	let out_dir = env::var_os("OUT_DIR").expect("OUT_DIR is set for build scripts");
	fs::write(Path::new(&out_dir).join("canopy_schema.rs"), generated)
		.expect("writing generated canopy schema");
}

/// Post-process the generated wire types.
///
/// typify emits every field as a plain type from the JSON Schema, which loses
/// two properties bestool relies on: timestamps should be `jiff::Timestamp`
/// (canopy models them as `date-time` strings, or bare strings for the
/// credential expiry), and credential secrets must not be printable. This
/// rewrites those fields in the generated source: `date-time` fields and the
/// credential expiry become `jiff::Timestamp`, and each secret field is wrapped
/// in [`crate::Redacted`] so it stays out of `Debug` output and logs.
///
/// The rewrites are string substitutions keyed on the exact field lines typify
/// produces. The asserts below fail the build if a substitution stops matching
/// — a schema or codegen change that silently dropped redaction would otherwise
/// compile clean.
fn rewrite_types(generated: &str) -> String {
	let generated = generated
		.replace(
			"::chrono::DateTime<::chrono::offset::Utc>",
			"::jiff::Timestamp",
		)
		.replace(
			"pub expiration: ::std::string::String,",
			"pub expiration: ::jiff::Timestamp,",
		)
		.replace(
			"pub secret_access_key: ::std::string::String,",
			"pub secret_access_key: crate::Redacted<::std::string::String>,",
		)
		.replace(
			"pub session_token: ::std::string::String,",
			"pub session_token: crate::Redacted<::std::string::String>,",
		)
		.replace(
			"pub repo_password: ::std::string::String,",
			"pub repo_password: crate::Redacted<::std::string::String>,",
		);

	assert!(
		!generated.contains("chrono"),
		"generated canopy schema still references chrono after rewrite to jiff"
	);
	for needle in [
		"pub expiration: ::jiff::Timestamp,",
		"pub secret_access_key: crate::Redacted<::std::string::String>,",
		"pub session_token: crate::Redacted<::std::string::String>,",
		"pub repo_password: crate::Redacted<::std::string::String>,",
	] {
		assert!(
			generated.contains(needle),
			"canopy schema rewrite did not apply; expected `{needle}` in the generated source \
			 (did canopy's OpenAPI field names change?)"
		);
	}

	generated
}

fn fetch_live() -> Result<String, ureq::Error> {
	let config = ureq::Agent::config_builder()
		.timeout_global(Some(FETCH_TIMEOUT))
		.build();
	let agent: ureq::Agent = config.into();
	agent.get(SPEC_URL).call()?.body_mut().read_to_string()
}
