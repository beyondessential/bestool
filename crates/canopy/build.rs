//! Generates the `schema` module from canopy's OpenAPI document.
//!
//! The document is fetched live from the running canopy deployment so the wire
//! types track canopy as it evolves, with nothing generated committed — canopy
//! adding a field changes no committed source and never triggers a republish.
//! Its `components.schemas` are wrapped into a JSON-Schema root document and
//! handed to typify, which emits the wire types; the result is written to
//! `OUT_DIR` and `include!`d by the crate.
//!
//! A failed fetch is a **hard error** by default, rather than a silent fall back
//! to a possibly-stale snapshot: `cargo:warning` is hidden for dependencies, so
//! silently falling back would let a downstream build ship stale types with no
//! signal. The committed snapshot is used only where a live fetch is impossible
//! by design — docs.rs builds (which set `DOCS_RS`) and builds that explicitly
//! opt in with `CANOPY_OPENAPI_OFFLINE`.

use std::{env, fs, path::Path, time::Duration};

use schemars::schema::RootSchema;
use serde_json::Value;
use typify::{TypeSpace, TypeSpaceSettings};

// Kept in step with the snapshot-refresh step in .github/workflows/release-plz.yml.
const SPEC_URL: &str = "https://meta.tamanu.app/api/openapi.json";
const SNAPSHOT: &str = "openapi.snapshot.json";
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// docs.rs sets this in its (network-less) build environment.
const DOCS_RS_ENV: &str = "DOCS_RS";
/// Explicit opt-in to the committed snapshot for offline / sealed builds.
const OFFLINE_ENV: &str = "CANOPY_OPENAPI_OFFLINE";

fn main() {
	println!("cargo:rerun-if-changed=build.rs");
	println!("cargo:rerun-if-changed={SNAPSHOT}");
	println!("cargo:rerun-if-env-changed={DOCS_RS_ENV}");
	println!("cargo:rerun-if-env-changed={OFFLINE_ENV}");

	let spec_text = match fetch_live() {
		Ok(text) => text,
		Err(err) if snapshot_allowed() => {
			println!(
				"cargo:warning=canopy OpenAPI live fetch failed ({err}); using committed snapshot"
			);
			fs::read_to_string(SNAPSHOT).expect("reading canopy OpenAPI snapshot")
		}
		Err(err) => panic!(
			"canopy OpenAPI live fetch from {SPEC_URL} failed: {err}\n\
			 The generated schema tracks canopy live, so this build cannot proceed with a \
			 possibly-stale snapshot. Fix connectivity to canopy, or set {OFFLINE_ENV}=1 to \
			 build against the committed {SNAPSHOT} instead."
		),
	};

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
	let mut generated = rewrite_types(&prettyplease::unparse(&file));
	generated.push_str(&generate_client_methods(&spec));

	let out_dir = env::var_os("OUT_DIR").expect("OUT_DIR is set for build scripts");
	fs::write(Path::new(&out_dir).join("canopy_schema.rs"), generated)
		.expect("writing generated canopy schema");
}

const HTTP_VERBS: [&str; 5] = ["get", "post", "put", "delete", "patch"];

/// One endpoint's inputs, gathered from the OpenAPI operation.
struct Endpoint {
	/// Base method name derived from the path (params dropped, `-`→`_`).
	name: String,
	verb: String,
	path: String,
	/// Path-parameter names in order of appearance.
	params: Vec<String>,
	/// Rust type of the JSON request body, if any.
	body: Option<String>,
	/// Rust return type: `Some(ty)` parses JSON into `ty`, `None` expects no body.
	response: Option<String>,
	/// The operation's OpenAPI `summary`, if any.
	summary: Option<String>,
	/// The operation's OpenAPI `description`, if any.
	description: Option<String>,
}

/// Generate an `impl crate::CanopyClient` block with one method per OpenAPI
/// operation, routing through the client's shared transport (`call_json` /
/// `call_empty`). Method names come from the path; where a path is served by
/// more than one verb the verb is prefixed to disambiguate.
fn generate_client_methods(spec: &Value) -> String {
	let paths = spec
		.get("paths")
		.and_then(Value::as_object)
		.expect("canopy OpenAPI document has no paths");

	let mut endpoints = Vec::new();
	for (path, item) in paths {
		let item = item.as_object().expect("openapi path item is an object");
		for (verb, op) in item {
			if !HTTP_VERBS.contains(&verb.as_str()) {
				continue;
			}
			let params: Vec<String> = path
				.split('/')
				.filter_map(|seg| seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')))
				.map(str::to_owned)
				.collect();
			let name = path
				.split('/')
				.filter(|seg| !seg.is_empty() && !seg.starts_with('{'))
				.map(|seg| seg.replace('-', "_"))
				.collect::<Vec<_>>()
				.join("_");
			endpoints.push(Endpoint {
				name,
				verb: verb.clone(),
				path: path.clone(),
				params,
				body: request_body_type(spec, op),
				response: response_type(op),
				summary: op.get("summary").and_then(Value::as_str).map(str::to_owned),
				description: op
					.get("description")
					.and_then(Value::as_str)
					.map(str::to_owned),
			});
		}
	}

	// Disambiguate names shared by multiple verbs (e.g. /versions/{version}).
	let mut counts = std::collections::HashMap::<&str, usize>::new();
	for ep in &endpoints {
		*counts.entry(ep.name.as_str()).or_default() += 1;
	}
	let collides: std::collections::HashSet<String> = endpoints
		.iter()
		.filter(|ep| counts[ep.name.as_str()] > 1)
		.map(|ep| ep.name.clone())
		.collect();

	endpoints.sort_by(|a, b| (&a.path, &a.verb).cmp(&(&b.path, &b.verb)));

	let mut out = String::from("impl crate::CanopyClient {\n");
	for ep in &endpoints {
		let method_name = if collides.contains(&ep.name) {
			format!("{}_{}", ep.verb, ep.name)
		} else {
			ep.name.clone()
		};
		let http_method = format!("::reqwest::Method::{}", ep.verb.to_uppercase());

		let mut args = String::new();
		for param in &ep.params {
			args.push_str(&format!(", {param}: &str"));
		}
		if let Some(body) = &ep.body {
			args.push_str(&format!(", body: &{body}"));
		}

		let path_expr = if ep.params.is_empty() {
			format!("{:?}", ep.path)
		} else {
			let mut template = ep.path.clone();
			for param in &ep.params {
				template = template.replace(&format!("{{{param}}}"), "{}");
			}
			format!("&format!({template:?}, {})", ep.params.join(", "))
		};
		let body_arg = if ep.body.is_some() {
			"Some(body)"
		} else {
			"None::<&()>"
		};

		let (call, ret) = match &ep.response {
			Some(ty) => ("call_json", format!("::miette::Result<{ty}>")),
			None => ("call_empty", "::miette::Result<()>".to_owned()),
		};

		let mut doc = String::new();
		for text in [&ep.summary, &ep.description].into_iter().flatten() {
			for line in text.lines() {
				if line.is_empty() {
					doc.push_str("    ///\n");
				} else {
					doc.push_str(&format!("    /// {line}\n"));
				}
			}
			doc.push_str("    ///\n");
		}
		doc.push_str(&format!(
			"    /// `{} {}`\n",
			ep.verb.to_uppercase(),
			ep.path
		));

		out.push_str(&format!(
			"{doc}    pub async fn {method_name}(&self{args}) -> {ret} {{\n        self.{call}({http_method}, {path_expr}, {body_arg}).await\n    }}\n",
		));
	}
	out.push_str("}\n");
	out
}

/// Rust type for an operation's JSON request body, or `None` if it has none.
///
/// A `$ref` to an open schema (an `allOf` that includes a free-form object, like
/// canopy's `StatusPayload`) can't be losslessly typed, so it maps to
/// `serde_json::Value`; other `$ref`s use the generated type.
fn request_body_type(spec: &Value, op: &Value) -> Option<String> {
	let schema = op.pointer("/requestBody/content/application~1json/schema")?;
	Some(match schema.get("$ref").and_then(Value::as_str) {
		Some(reference) => {
			let name = type_name(reference);
			if is_open_schema(spec, &name) {
				"::serde_json::Value".to_owned()
			} else {
				name
			}
		}
		None => "::serde_json::Value".to_owned(),
	})
}

/// Rust return type for an operation's 200 response: `Some(ty)` to parse JSON,
/// `None` when there's no JSON body to parse.
fn response_type(op: &Value) -> Option<String> {
	let schema = op.pointer("/responses/200/content/application~1json/schema")?;
	if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
		return Some(type_name(reference));
	}
	match schema.get("type").and_then(Value::as_str) {
		Some("array") => {
			let item = schema
				.pointer("/items/$ref")
				.and_then(Value::as_str)
				.map(type_name)
				.unwrap_or_else(|| "::serde_json::Value".to_owned());
			Some(format!("::std::vec::Vec<{item}>"))
		}
		_ => Some("::serde_json::Value".to_owned()),
	}
}

/// Last path segment of a `#/components/schemas/Foo` reference.
fn type_name(reference: &str) -> String {
	reference
		.rsplit('/')
		.next()
		.expect("schema $ref is non-empty")
		.to_owned()
}

/// Whether `components.schemas[name]` is an open `allOf` — one whose members
/// include a free-form object (no `properties`, no `$ref`). typify drops the
/// free-form part, so such schemas can't be sent losslessly as their typed form.
fn is_open_schema(spec: &Value, name: &str) -> bool {
	let Some(schema) = spec.pointer(&format!("/components/schemas/{name}")) else {
		return false;
	};
	let Some(all_of) = schema.get("allOf").and_then(Value::as_array) else {
		return false;
	};
	all_of.iter().any(|member| {
		member.get("type").and_then(Value::as_str) == Some("object")
			&& member.get("properties").is_none()
			&& member.get("$ref").is_none()
	})
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

/// Whether a failed live fetch may fall back to the committed snapshot: only on
/// docs.rs or when an offline build is explicitly requested.
fn snapshot_allowed() -> bool {
	env::var_os(DOCS_RS_ENV).is_some() || env::var_os(OFFLINE_ENV).is_some()
}

fn fetch_live() -> Result<String, reqwest::Error> {
	reqwest::blocking::Client::builder()
		.timeout(FETCH_TIMEOUT)
		.build()?
		.get(SPEC_URL)
		.send()?
		.error_for_status()?
		.text()
}
