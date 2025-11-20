use std::{collections::HashMap, path::PathBuf, time::Duration};

use miette::{Context as _, IntoDiagnostic, Result};
use tracing::{debug, error, warn};
use walkdir::WalkDir;

use crate::{
	alert::AlertDefinition,
	targets::{AlertTargets, ExternalTarget},
};

pub struct LoadedAlerts {
	pub alerts: Vec<AlertDefinition>,
	#[expect(dead_code, reason = "kept for potential future use")]
	pub external_targets: HashMap<String, Vec<ExternalTarget>>,
}

pub fn load_alerts_from_dirs(dirs: &[PathBuf], default_interval: Duration) -> Result<LoadedAlerts> {
	let mut alerts = Vec::<AlertDefinition>::new();
	let mut external_targets = HashMap::new();

	for dir in dirs {
		if !dir.exists() {
			warn!(?dir, "alert directory does not exist, skipping");
			continue;
		}

		let external_targets_path = dir.join("_targets.yml");
		if let Some(AlertTargets { targets }) = std::fs::read_to_string(&external_targets_path)
			.ok()
			.and_then(|content| {
				debug!(path=?external_targets_path, "parsing external targets");
				serde_yaml::from_str::<AlertTargets>(&content)
					.map_err(
						|err| warn!(path=?external_targets_path, "_targets.yml has errors! {err}"),
					)
					.ok()
			}) {
			for target in targets {
				external_targets
					.entry(target.id().into())
					.or_insert(Vec::new())
					.push(target);
			}
		}

		alerts.extend(
			WalkDir::new(dir)
				.into_iter()
				.filter_map(|e| e.ok())
				.filter(|e| e.file_type().is_file())
				.map(|entry| {
					let file = entry.path();

					if !file.extension().is_some_and(|e| e == "yaml" || e == "yml") {
						return Ok(None);
					}

					if file.file_stem().is_some_and(|n| n == "_targets") {
						return Ok(None);
					}

					debug!(?file, "parsing YAML file");
					let content = std::fs::read_to_string(file)
						.into_diagnostic()
						.wrap_err(format!("{file:?}"))?;
					let mut alert: AlertDefinition = serde_yaml::from_str(&content)
						.into_diagnostic()
						.wrap_err(format!("{file:?}"))?;

					alert.file = file.to_path_buf();
					alert.interval = default_interval;
					debug!(?alert, "parsed alert file");
					Ok(if alert.enabled { Some(alert) } else { None })
				})
				.filter_map(|def: Result<Option<AlertDefinition>>| match def {
					Err(err) => {
						error!("{err:?}");
						None
					}
					Ok(def) => def,
				}),
		);
	}

	if !external_targets.is_empty() {
		debug!(count=%external_targets.len(), "found some external targets");
	}

	for alert in &mut alerts {
		*alert = std::mem::take(alert).normalise(&external_targets);
	}
	debug!(count=%alerts.len(), "found some alerts");

	Ok(LoadedAlerts {
		alerts,
		external_targets,
	})
}
