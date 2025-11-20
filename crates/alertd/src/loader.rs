use std::{collections::HashMap, path::Path};

use miette::Result;
use tracing::{debug, error, warn};
use walkdir::WalkDir;

use crate::{
	alert::AlertDefinition,
	glob_resolver::ResolvedPaths,
	targets::{AlertTargets, ExternalTarget},
};

pub struct LoadedAlerts {
	pub alerts: Vec<(AlertDefinition, Vec<crate::targets::ResolvedTarget>)>,
	pub external_targets: HashMap<String, Vec<ExternalTarget>>,
}

pub fn load_alerts_from_paths(resolved: &ResolvedPaths) -> Result<LoadedAlerts> {
	let mut alerts = Vec::<AlertDefinition>::new();
	let mut external_targets = HashMap::new();

	// Load external targets from directories
	for dir in &resolved.dirs {
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
					.entry(target.id.clone())
					.or_insert(Vec::new())
					.push(target);
			}
		}
	}

	// Load alerts from directories (recursively)
	for dir in &resolved.dirs {
		alerts.extend(
			WalkDir::new(dir)
				.into_iter()
				.filter_map(|e| e.ok())
				.filter(|e| e.file_type().is_file())
				.filter_map(|entry| load_alert_from_file(entry.path())),
		);
	}

	// Load alerts from individual files
	for file in &resolved.files {
		if let Some(alert) = load_alert_from_file(file) {
			alerts.push(alert);
		}
	}

	if !external_targets.is_empty() {
		debug!(count=%external_targets.len(), "found some external targets");
	}

	let alerts_with_targets: Vec<_> = alerts
		.into_iter()
		.filter_map(|alert| {
			let file = alert.file.clone();
			match alert.normalise(&external_targets) {
				Ok(normalized) => Some(normalized),
				Err(err) => {
					error!(file=?file, "failed to normalise alert: {err:?}");
					None
				}
			}
		})
		.collect();

	debug!(count=%alerts_with_targets.len(), "found some alerts");

	Ok(LoadedAlerts {
		alerts: alerts_with_targets,
		external_targets,
	})
}

fn load_alert_from_file(file: &Path) -> Option<AlertDefinition> {
	if !file.extension().is_some_and(|e| e == "yaml" || e == "yml") {
		return None;
	}

	if file.file_stem().is_some_and(|n| n == "_targets") {
		return None;
	}

	debug!(?file, "parsing YAML file");
	let content = match std::fs::read_to_string(file) {
		Ok(content) => content,
		Err(err) => {
			error!(?file, "failed to read file: {err:?}");
			return None;
		}
	};

	let mut alert: AlertDefinition = match serde_yaml::from_str(&content) {
		Ok(alert) => alert,
		Err(err) => {
			error!(?file, "failed to parse YAML: {err:?}");
			return None;
		}
	};

	alert.file = file.to_path_buf();
	debug!(?alert, "parsed alert file");

	if alert.enabled { Some(alert) } else { None }
}
