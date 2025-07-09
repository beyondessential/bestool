use clap::Parser;
use comfy_table::{modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, ContentArrangement, Table};
use miette::{IntoDiagnostic, Result};
use reqwest::Client;
use serde::Deserialize;

use crate::actions::Context;

use super::TamanuArgs;

/// List available artifacts for a Tamanu version.
///
/// Fetches and displays the available artifacts (downloads) for a specific Tamanu version.
#[derive(Debug, Clone, Parser)]
pub struct ArtifactsArgs {
	/// Version to list artifacts for.
	#[arg(value_name = "VERSION")]
	pub version: String,
}

#[derive(Debug, Deserialize)]
struct Artifact {
	pub artifact_type: String,
	pub platform: String,
	pub download_url: String,
}

pub async fn run(ctx: Context<TamanuArgs, ArtifactsArgs>) -> Result<()> {
	let ArtifactsArgs { version } = ctx.args_sub;

	let url = format!("https://meta.tamanu.app/versions/{}/artifacts", version);
	let client = Client::new();

	let response = client.get(&url).send().await.into_diagnostic()?;

	if !response.status().is_success() {
		return Err(miette::miette!(
			"Failed to fetch artifacts for version {}: HTTP {}",
			version,
			response.status()
		));
	}

	let mut artifacts: Vec<Artifact> = response.json().await.into_diagnostic()?;
	if artifacts.is_empty() {
		println!("No artifacts found for version {}", version);
		return Ok(());
	}

	artifacts.sort_by(|a, b| a.platform.cmp(&b.platform));
	artifacts.sort_by(|a, b| a.artifact_type.cmp(&b.artifact_type));

	let mut table = Table::new();
	table
		.load_preset(UTF8_FULL)
		.apply_modifier(UTF8_ROUND_CORNERS)
		.set_content_arrangement(ContentArrangement::Dynamic)
		.set_header(vec!["Type", "Platform", "Download URL"]);
	for artifact in artifacts {
		table.add_row(vec![
			artifact.artifact_type,
			artifact.platform,
			artifact.download_url,
		]);
	}
	println!("{table}");

	Ok(())
}
