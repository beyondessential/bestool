use std::{collections::HashMap, path::Path};

use miette::Result;
use tracing::{debug, error, warn};
use walkdir::WalkDir;

use crate::{
	LogError,
	alert::AlertDefinition,
	glob_resolver::ResolvedPaths,
	targets::{AlertTargets, ExternalTarget},
};

pub struct LoadedAlerts {
	pub alerts: Vec<(AlertDefinition, Vec<crate::targets::ResolvedTarget>)>,
	pub external_targets: HashMap<String, Vec<ExternalTarget>>,
	pub definition_errors: Vec<DefinitionError>,
}

#[derive(Debug, Clone)]
pub struct DefinitionError {
	pub file: std::path::PathBuf,
	pub error: String,
}

pub fn load_alerts_from_paths(resolved: &ResolvedPaths) -> Result<LoadedAlerts> {
	let mut alerts = Vec::<AlertDefinition>::new();
	let mut external_targets = HashMap::new();
	let mut definition_errors = Vec::new();

	// Load external targets from files
	for external_targets_path in &resolved.files {
		if let Some(name) = external_targets_path.file_name()
			&& (name.to_ascii_lowercase() == "_targets.yml"
				|| name.to_ascii_lowercase() == "_targets.yaml")
		{
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
				debug!(path=?external_targets_path, count=targets.len(), "loaded external targets from file");
				for target in targets {
					debug!(id=%target.id, path=?external_targets_path, "adding external target");
					external_targets
						.entry(target.id.clone())
						.or_insert(Vec::new())
						.push(target);
				}
			}
		}
	}

	// Load external targets from directories
	for dir in &resolved.dirs {
		for external_targets_path in vec![dir.join("_targets.yml"), dir.join("_targets.yaml")] {
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
				debug!(path=?external_targets_path, count=targets.len(), "loaded external targets from directory");
				for target in targets {
					debug!(id=%target.id, path=?external_targets_path, "adding external target");
					external_targets
						.entry(target.id.clone())
						.or_insert(Vec::new())
						.push(target);
				}
			}
		}
	}

	// Load alerts from directories (recursively)
	for dir in &resolved.dirs {
		for entry in WalkDir::new(dir)
			.into_iter()
			.filter_map(|e| e.ok())
			.filter(|e| e.file_type().is_file())
		{
			match load_alert_from_file(entry.path()) {
				LoadAlertResult::Success(alert) => alerts.push(alert),
				LoadAlertResult::Error(err) => definition_errors.push(err),
				LoadAlertResult::Disabled | LoadAlertResult::Skip => {}
			}
		}
	}

	// Load alerts from individual files
	for file in &resolved.files {
		match load_alert_from_file(file) {
			LoadAlertResult::Success(alert) => alerts.push(alert),
			LoadAlertResult::Error(err) => definition_errors.push(err),
			LoadAlertResult::Disabled | LoadAlertResult::Skip => {}
		}
	}

	if !external_targets.is_empty() {
		debug!(
			count=%external_targets.len(),
			ids=?external_targets.keys().collect::<Vec<_>>(),
			"found external targets"
		);
	} else {
		warn!("no external targets found");
	}

	let alerts_with_targets: Vec<_> = alerts
		.into_iter()
		.filter_map(|alert| {
			let file = alert.file.clone();
			let send_target_ids: Vec<_> = alert.send.iter().map(|t| t.id()).collect();
			debug!(
				file=?file,
				send_targets=?send_target_ids,
				available_targets=?external_targets.keys().collect::<Vec<_>>(),
				"normalising alert"
			);
			match alert.normalise(&external_targets) {
				Ok(normalized) => Some(normalized),
				Err(err) => {
					error!(file=?file, "failed to normalise alert: {}", LogError(&err));
					definition_errors.push(DefinitionError {
						file: file.clone(),
						error: format!("{:#}", err),
					});
					None
				}
			}
		})
		.collect();

	debug!(count=%alerts_with_targets.len(), "found some alerts");

	if !definition_errors.is_empty() {
		warn!(count=%definition_errors.len(), "found alert definition errors");
	}

	Ok(LoadedAlerts {
		alerts: alerts_with_targets,
		external_targets,
		definition_errors,
	})
}

enum LoadAlertResult {
	Success(AlertDefinition),
	Disabled,
	Skip,
	Error(DefinitionError),
}

fn load_alert_from_file(file: &Path) -> LoadAlertResult {
	if !file.extension().is_some_and(|e| e == "yaml" || e == "yml") {
		return LoadAlertResult::Skip;
	}

	if file.file_stem().is_some_and(|n| n == "_targets") {
		return LoadAlertResult::Skip;
	}

	debug!(?file, "parsing YAML file");
	let content = match std::fs::read_to_string(file) {
		Ok(content) => content,
		Err(err) => {
			error!(?file, "failed to read file: {err}");
			return LoadAlertResult::Error(DefinitionError {
				file: file.to_path_buf(),
				error: format!("Failed to read file: {}", err),
			});
		}
	};

	let mut alert: AlertDefinition = match serde_yaml::from_str(&content) {
		Ok(alert) => alert,
		Err(err) => {
			error!(?file, "failed to parse YAML: {err}");
			return LoadAlertResult::Error(DefinitionError {
				file: file.to_path_buf(),
				error: format!("Failed to parse YAML: {}", err),
			});
		}
	};

	alert.file = file.to_path_buf();
	debug!(?alert, "parsed alert file");

	if alert.enabled {
		LoadAlertResult::Success(alert)
	} else {
		LoadAlertResult::Disabled
	}
}
