use std::{num::NonZeroU16, path::PathBuf};

use clap::Parser;
use miette::{IntoDiagnostic, Result};
use tera::{Context as TeraContext, Tera};
use tokio::{fs::File, io::AsyncWriteExt};
use tracing::info;

use crate::actions::Context;

use super::CaddyArgs;

const CADDYFILE_TEMPLATE: &str = include_str!("tamanu.Caddyfile.tera");
#[test]
fn test_caddyfile_template() {
	let mut tera = Tera::default();
	tera.add_raw_template("Caddyfile", CADDYFILE_TEMPLATE)
		.into_diagnostic()
		.unwrap();
}

pub const DEFAULT_CADDYFILE_PATH: &str = if cfg!(windows) {
	r"C:\Caddy\Caddyfile"
} else {
	"/etc/caddy/Caddyfile"
};

/// Configure Caddy for a Tamanu installation.
#[derive(Debug, Clone, Parser)]
pub struct ConfigureTamanuArgs {
	/// Path to the Caddyfile.
	#[arg(long, default_value = DEFAULT_CADDYFILE_PATH)]
	pub path: PathBuf,

	/// Print the Caddyfile, don't write it to disk.
	#[arg(long)]
	pub print: bool,

	/// Tamanu domain name.
	#[arg(long, value_name = "DOMAIN")]
	pub domain: String,

	/// Tamanu API server port.
	#[arg(long, value_name = "PORT")]
	pub api_port: NonZeroU16,

	/// Tamanu server version to configure.
	#[arg(long, value_name = "VERSION")]
	pub api_version: String,

	/// Tamanu frontend version to configure.
	#[arg(long, value_name = "VERSION")]
	pub web_version: String,

	/// Email for TLS issuance.
	#[arg(long)]
	pub email: Option<String>,

	/// ZeroSSL API Key.
	///
	/// If not provided, ZeroSSL will still be used as per default Caddy config, but rate limited.
	#[arg(long)]
	pub zerossl_api_key: Option<String>,
}

pub async fn run(ctx: Context<CaddyArgs, ConfigureTamanuArgs>) -> Result<()> {
	let ConfigureTamanuArgs {
		path,
		print,
		domain,
		api_port,
		api_version,
		web_version,
		email,
		zerossl_api_key,
	} = ctx.args_sub;

	let mut tera = Tera::default();
	tera.add_raw_template("Caddyfile", CADDYFILE_TEMPLATE)
		.into_diagnostic()?;

	let mut context = TeraContext::new();
	context.insert("domain", &domain);
	context.insert("api_port", &api_port);
	context.insert("api_version", &api_version);
	context.insert("web_version", &web_version);
	context.insert("windows", &cfg!(windows));

	context.insert("has_email", &email.is_some());
	if let Some(email) = email {
		context.insert("email", &email);
	}

	context.insert("has_zerossl", &zerossl_api_key.is_some());
	if let Some(zerossl_api_key) = zerossl_api_key {
		context.insert("zerossl_api_key", &zerossl_api_key);
	}

	let rendered = tera.render("Caddyfile", &context).into_diagnostic()?;

	if print {
		println!("{rendered}");
		return Ok(());
	}

	info!(?path, "writing new Caddyfile");
	let mut file = File::create(&path).await.into_diagnostic()?;
	file.write_all(rendered.as_bytes())
		.await
		.into_diagnostic()?;

	Ok(())
}
