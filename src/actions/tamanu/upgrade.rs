use std::{fs, path::PathBuf};

use clap::Parser;
use miette::{bail, miette, Context as _, IntoDiagnostic, Result};
use node_semver::Version;
use regex::RegexBuilder;
use tracing::info;

use crate::actions::{caddy::configure_tamanu::DEFAULT_CADDYFILE_PATH, Context};

use super::{
	find_package, find_tamanu, prepare_upgrade::UPGRADED_SIGNAL_NAME, ApiServerKind, TamanuArgs,
};

/// Perform an upgrade.
///
/// This will incur downtime.
///
/// `bestool tamanu pre-upgrade` must be run before this command.
///
/// This command will switch the server to upgrade mode, take down API servers, migrate the database,
/// start new API servers, upgrade the frontend, healthcheck, and finally switch out of upgrade mode.
#[derive(Debug, Clone, Parser)]
pub struct UpgradeArgs {
	/// Version to update to.
	#[arg(value_name = "VERSION")]
	pub version: Version,

	/// Package to upgrade.
	///
	/// By default, this command looks for the most recent installed version of Tamanu.
	/// If both central and facility servers are present and
	/// configured, it will pick one arbitrarily.
	#[arg(short, long)]
	pub kind: Option<ApiServerKind>,

	/// Path to the Caddyfile.
	#[arg(long, default_value = DEFAULT_CADDYFILE_PATH)]
	pub caddyfile_path: PathBuf,
}

pub async fn run(ctx: Context<TamanuArgs, UpgradeArgs>) -> Result<()> {
	let UpgradeArgs {
		kind,
		version,
		caddyfile_path,
	} = ctx.args_sub;

	let (_, root) = find_tamanu(&ctx.args_top)?;
	let new_root = root.parent().unwrap().join(format!("release-v{version}"));

	let kind = match kind {
		Some(kind) => kind,
		None => find_package(&root)?,
	};
	info!(?kind, "using");

	if version < Version::parse("2.0.0").unwrap() {
		bail!("version is too low, bestool doesn't support Tamanu <2.0.0");
	}

	if !new_root.exists() || !new_root.join(UPGRADED_SIGNAL_NAME).exists() {
		bail!("new Tamanu not found, try running `prepare-upgrade`");
	}

	// Caddyfile does not support automation, and this would be easier with JSON.
	// It's rewriting the config files however since we already use Caddyfile and automated parts are not large.
	info!(?caddyfile_path, "updating Caddy config file");
	let re = RegexBuilder::new(r#"c:\\tamanu\\tamanu-web-[0-9]\.[0-9]\.[0-9]"#)
		.case_insensitive(true)
		.build()
		.unwrap();
	let old_caddyfile = fs::read_to_string(&caddyfile_path).into_diagnostic()?;
	let new_caddyfile =
		re.replace_all(&old_caddyfile, format!(r#"C:\tamanu\tamanu-web-{version}"#));
	fs::write(&caddyfile_path, new_caddyfile.as_bytes()).into_diagnostic()?;

	// Caddy recommends against using config files and the API at the same time.
	// This uses the API anyways because this is only a temporary change.
	// See also https://caddyserver.com/docs/getting-started#api-vs-config-files
	info!("switch Caddy to upgrading mode");

	let caddyjson = reqwest::Client::new()
		.post("http://localhost:2019/adapt")
		.header("Content-Type", "text/caddyfile")
		.body(new_caddyfile.into_owned())
		.send()
		.await
		.into_diagnostic()?
		.json::<serde_json::Value>()
		.await
		.into_diagnostic()?;
	let caddyjson = caddyjson
		.get("result")
		.ok_or_else(|| miette!("unexpected response body from Caddy API"))?;

	let mut caddyjson_upgrading = caddyjson.clone();
	caddyjson_upgrading
		.pointer_mut("/apps/http/servers/srv0/routes/0/handle/0/routes")
		.and_then(|v| v.as_array_mut())
		.ok_or_else(|| miette!("failed to parse Caddy config"))?
		.insert(
			1,
			serde_json::json!({
				"handle": [
					{
						"error": "Upgrading",
						"handler": "error",
						"status_code": 526
					}
				],
				"match": [
					{
						"not": [
							{
								"remote_ip": {
									"ranges": [
										"127.0.0.1/8",
										"fd00::/8",
										"::1"
									]
								}
							}
						]
					}
				]
			}),
		);

	reqwest::Client::new()
		.post("http://localhost:2019/load")
		.header("Content-Type", "application/json")
		.body(caddyjson_upgrading.to_string())
		.send()
		.await
		.into_diagnostic()?;

	duct::cmd!("cmd", "/C", "pm2", "delete", "all")
		.run()
		.into_diagnostic()
		.wrap_err("failed to run pm2")?;

	info!("running migrations");
	duct::cmd!("node", "dist", "migrate")
		.dir(&new_root.join("packages").join(kind.package_name()))
		.run()
		.into_diagnostic()?;

	info!("starting the server");
	duct::cmd!("cmd", "/C", "pm2", "start", "pm2.config.cjs")
		.dir(&new_root)
		.run()
		.into_diagnostic()
		.wrap_err("failed to run pm2")?;

	info!("verifying the server is running");
	let healthcheck_response = reqwest::Client::new()
		.get("http://localhost")
		.send()
		.await
		.into_diagnostic()
		.wrap_err("the Tamanu server is not responding")?
		.error_for_status()
		.into_diagnostic()
		.wrap_err("the Tamanu server failed to correctly startup")?;
	info!(?healthcheck_response);

	duct::cmd!("cmd", "/C", "pm2", "logs", "--nostream")
		.run()
		.into_diagnostic()?;

	duct::cmd!("cmd", "/C", "pm2", "save")
		.run()
		.into_diagnostic()
		.wrap_err("failed to run pm2")?;

	info!("load the new config and switch Caddy back to normal mode");
	reqwest::Client::new()
		.post("http://localhost:2019/load")
		.header("Content-Type", "application/json")
		.body(caddyjson.to_string())
		.send()
		.await
		.into_diagnostic()?;

	Ok(())
}
