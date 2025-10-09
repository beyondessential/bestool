use clap::Parser;
use comfy_table::{modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, ContentArrangement, Table};
use detect_targets::detect_targets;
use miette::{IntoDiagnostic, Result};
use reqwest::{Client, Url};
use serde::Deserialize;
use std::str::FromStr;
use target_tuples::{Target, OS};

use crate::actions::Context;

use super::TamanuArgs;

/// List available artifacts for a Tamanu version.
///
/// Fetches and displays the available artifacts (downloads) for a specific Tamanu version.
///
/// Alias: art
#[derive(Debug, Clone, Parser)]
pub struct ArtifactsArgs {
	/// Version to list artifacts for.
	#[arg(value_name = "VERSION")]
	pub version: String,

	/// Platform to list artifacts for.
	///
	/// Use `host` (default) for the auto-detected current platform, `container` for container artifacts,
	/// `os-arch` for specific targets (e.g., `linux-x86_64`), and `all` to list all platforms.
	#[arg(short, long, value_name = "PLATFORM", default_value = "host")]
	pub platform: Platform,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Platform {
	All,
	Host,
	Match(String),
}

impl FromStr for Platform {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_ascii_lowercase().as_str() {
			"all" => Ok(Platform::All),
			"host" => Ok(Platform::Host),
			s => Ok(Platform::Match(s.to_string())),
		}
	}
}

#[derive(Debug, Deserialize)]
pub struct Artifact {
	pub artifact_type: String,
	pub platform: String,
	pub download_url: Url,
}

pub async fn get_artifacts(version: &str, for_platform: &Platform) -> Result<Vec<Artifact>> {
	let url = format!("https://meta.tamanu.app/versions/{version}/artifacts");
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
		return Ok(Vec::new());
	}

	let host_targets = detect_targets()
		.await
		.into_iter()
		.filter_map(|target| Target::from_str(&target).ok())
		.collect::<Vec<Target>>();

	artifacts.retain(|artifact| match &for_platform {
		Platform::All => true,
		Platform::Host => {
			if artifact.platform == "any" {
				return true;
			}

			let os = if artifact.platform.contains("linux") {
				host_targets
					.iter()
					.any(|t| t.operating_system() == Some(OS::Linux))
			} else if artifact.platform.contains("windows") {
				host_targets
					.iter()
					.any(|t| t.operating_system() == Some(OS::Win32))
			} else if artifact.platform.contains("macos") {
				host_targets
					.iter()
					.any(|t| t.operating_system() == Some(OS::Darwin))
			} else {
				false
			};

			let arch = if artifact.platform.contains("amd64") {
				host_targets.iter().any(|t| t.arch_name() == "x86_64")
			} else if artifact.platform.contains("arm64") {
				host_targets.iter().any(|t| t.arch_name() == "aarch64")
			} else if !artifact.platform.contains("-") {
				true
			} else {
				false
			};

			os && arch
		}
		Platform::Match(s) => artifact.platform.contains(s),
	});

	artifacts.sort_by(|a, b| a.platform.cmp(&b.platform));
	artifacts.sort_by(|a, b| a.artifact_type.cmp(&b.artifact_type));

	Ok(artifacts)
}

pub async fn run(ctx: Context<TamanuArgs, ArtifactsArgs>) -> Result<()> {
	let ArtifactsArgs { version, platform } = ctx.args_sub;
	let artifacts = get_artifacts(&version, &platform).await?;

	if artifacts.is_empty() {
		println!(
			"No artifacts found for version {} with the specified platform filter",
			version
		);
		return Ok(());
	}

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
			artifact.download_url.to_string(),
		]);
	}
	println!("{table}");

	Ok(())
}
