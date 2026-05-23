use std::{
	path::PathBuf,
	sync::Arc,
	time::Duration,
};

use bestool_alertd::{BackgroundTask, TaskContext, canopy::DEFAULT_CANOPY_URL};
use bestool_tamanu::config::TamanuConfig;
use futures::future::BoxFuture;
use miette::{Result, miette};
use node_semver::Version;
use reqwest::Url;
use tracing::warn;

use crate::actions::tamanu::doctor;

const DOCTOR_INTERVAL: Duration = Duration::from_secs(60);

pub struct DoctorTask {
	tamanu_version: Version,
	tamanu_root: PathBuf,
	config: Arc<TamanuConfig>,
	database_url: String,
	canopy_base_url: Url,
}

impl DoctorTask {
	pub fn new(
		tamanu_version: Version,
		tamanu_root: PathBuf,
		config: Arc<TamanuConfig>,
		database_url: String,
	) -> Self {
		Self {
			tamanu_version,
			tamanu_root,
			config,
			database_url,
			canopy_base_url: DEFAULT_CANOPY_URL
				.parse()
				.expect("default canopy URL is valid"),
		}
	}

	async fn tick(&self, ctx: &TaskContext) -> Result<()> {
		let sweep = doctor::perform_sweep(
			&self.tamanu_version,
			&self.tamanu_root,
			self.config.clone(),
			&self.database_url,
			&[],
		)
		.await?;

		let Some(server_id) = sweep.server_id else {
			warn!("no metaServerId available; skipping canopy status push");
			return Ok(());
		};

		let Some(canopy) = ctx.canopy_client.as_ref() else {
			warn!("no canopy client available; skipping canopy status push");
			return Ok(());
		};

		canopy
			.post_status(&self.canopy_base_url, &server_id, &sweep.payload)
			.await
			.map_err(|err| miette!("posting doctor status to canopy: {err}"))
	}
}

impl BackgroundTask for DoctorTask {
	fn name(&self) -> &'static str {
		"doctor"
	}

	fn interval(&self) -> Duration {
		DOCTOR_INTERVAL
	}

	fn run<'a>(&'a self, ctx: &'a TaskContext) -> BoxFuture<'a, Result<()>> {
		Box::pin(self.tick(ctx))
	}
}
