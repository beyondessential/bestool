use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use jiff::Timestamp;
use miette::Result;
use tokio::{
	sync::RwLock,
	task::JoinHandle,
	time::{interval, sleep},
};
use tracing::{debug, error, info, warn};

use crate::{
	EmailConfig, LogError,
	alert::{AlertDefinition, InternalContext, TicketSource},
	events::{EventContext, EventManager, EventType},
	glob_resolver::{GlobResolver, ResolvedPaths},
	loader::{LoadedAlerts, load_alerts_from_paths},
	metrics,
	targets::ResolvedTarget,
};

#[derive(Debug, Clone)]
pub struct AlertState {
	pub definition: AlertDefinition,
	pub resolved_targets: Vec<ResolvedTarget>,
	pub triggered_at: Option<Timestamp>,
	pub last_sent_at: Option<Timestamp>,
	pub last_output: Option<String>,
	pub paused_until: Option<Timestamp>,
}

impl AlertState {
	pub fn new(definition: AlertDefinition, resolved_targets: Vec<ResolvedTarget>) -> Self {
		Self {
			definition,
			resolved_targets,
			triggered_at: None,
			last_sent_at: None,
			last_output: None,
			paused_until: None,
		}
	}

	pub fn preserve_state_from(&mut self, old_state: &AlertState) {
		self.triggered_at = old_state.triggered_at;
		self.last_sent_at = old_state.last_sent_at;
		self.last_output = old_state.last_output.clone();
		self.paused_until = old_state.paused_until;
	}
}

pub struct Scheduler {
	glob_resolver: GlobResolver,
	resolved_paths: Arc<RwLock<ResolvedPaths>>,
	ctx: Arc<InternalContext>,
	email: Option<EmailConfig>,
	dry_run: bool,
	alerts: Arc<RwLock<HashMap<PathBuf, Arc<RwLock<AlertState>>>>>,
	tasks: Arc<RwLock<HashMap<PathBuf, JoinHandle<()>>>>,
	event_manager: Arc<RwLock<Option<EventManager>>>,
	external_targets:
		Arc<RwLock<std::collections::HashMap<String, Vec<crate::targets::ExternalTarget>>>>,
}

impl Scheduler {
	pub fn new(
		alert_globs: Vec<String>,
		ctx: Arc<InternalContext>,
		email: Option<EmailConfig>,
		dry_run: bool,
	) -> Self {
		let glob_resolver = GlobResolver::new(alert_globs);
		Self {
			glob_resolver,
			resolved_paths: Arc::new(RwLock::new(ResolvedPaths {
				dirs: Vec::new(),
				files: Vec::new(),
			})),
			ctx,
			email,
			dry_run,
			alerts: Arc::new(RwLock::new(HashMap::new())),
			tasks: Arc::new(RwLock::new(HashMap::new())),
			event_manager: Arc::new(RwLock::new(None)),
			external_targets: Arc::new(RwLock::new(HashMap::new())),
		}
	}

	pub fn get_event_manager(&self) -> Arc<RwLock<Option<EventManager>>> {
		self.event_manager.clone()
	}

	pub async fn get_loaded_alerts(&self) -> Vec<PathBuf> {
		let alerts = self.alerts.read().await;
		let mut files: Vec<PathBuf> = alerts.keys().cloned().collect();
		files.sort();
		files
	}

	pub async fn get_alert_states(&self) -> HashMap<PathBuf, AlertState> {
		let alerts = self.alerts.read().await;
		let mut states = HashMap::new();
		for (path, state_lock) in alerts.iter() {
			let state = state_lock.read().await;
			states.insert(path.clone(), state.clone());
		}
		states
	}

	pub async fn pause_alert(&self, path: &PathBuf, until: Timestamp) -> Result<bool> {
		let alerts = self.alerts.read().await;
		if let Some(alert_state) = alerts.get(path) {
			let mut state = alert_state.write().await;
			state.paused_until = Some(until);
			info!(?path, until = %until, "paused alert");
			Ok(true)
		} else {
			Ok(false)
		}
	}

	pub async fn get_external_targets(
		&self,
	) -> std::collections::HashMap<String, Vec<crate::targets::ExternalTarget>> {
		self.external_targets.read().await.clone()
	}

	pub async fn load_and_schedule_alerts(&self) -> Result<()> {
		info!("resolving glob patterns and loading alerts");

		let resolved = self.glob_resolver.resolve()?;
		debug!(
			dirs = resolved.dirs.len(),
			files = resolved.files.len(),
			"resolved paths from globs"
		);

		let LoadedAlerts {
			alerts,
			external_targets,
			definition_errors,
		} = load_alerts_from_paths(&resolved)?;

		// Update resolved paths
		*self.resolved_paths.write().await = resolved;

		// Separate event alerts from regular alerts
		let (event_alerts, regular_alerts): (Vec<_>, Vec<_>) = alerts
			.into_iter()
			.partition(|(alert, _)| matches!(alert.source, TicketSource::Event { .. }));

		// Store external targets
		*self.external_targets.write().await = external_targets.clone();

		// Create event manager with event alerts and external targets
		let event_manager = EventManager::new(event_alerts, &external_targets);
		*self.event_manager.write().await = Some(event_manager.clone());

		// Send definition error events for any failed alert loads
		if !definition_errors.is_empty() {
			info!(
				count = definition_errors.len(),
				"triggering definition-error events for failed alerts"
			);
		}
		for def_err in definition_errors {
			info!(
				file = %def_err.file.display(),
				"triggering definition-error event"
			);
			let event_context = EventContext::DefinitionError {
				alert_file: def_err.file.display().to_string(),
				error_message: def_err.error.clone(),
			};
			if let Err(err) = event_manager
				.trigger_event(
					EventType::DefinitionError,
					&self.ctx,
					self.email.as_ref(),
					self.dry_run,
					event_context,
				)
				.await
			{
				error!(
					"failed to trigger definition-error event: {}",
					LogError(&err)
				);
			}
		}

		if regular_alerts.is_empty() {
			warn!("no regular alerts found");
			return Ok(());
		}

		info!(count = regular_alerts.len(), "scheduling regular alerts");

		// Get old alerts to preserve state
		let old_alerts = self.alerts.read().await.clone();

		let mut new_alerts = HashMap::new();
		let mut tasks = HashMap::new();

		for (definition, resolved_targets) in regular_alerts {
			let file = definition.file.clone();

			// Create new alert state
			let mut new_state = AlertState::new(definition.clone(), resolved_targets.clone());

			// Preserve state from old alert if it exists
			if let Some(old_alert_lock) = old_alerts.get(&file) {
				let old_state = old_alert_lock.read().await;
				new_state.preserve_state_from(&old_state);
			}

			let state_lock = Arc::new(RwLock::new(new_state));
			let task = self.spawn_alert_task(state_lock.clone());

			new_alerts.insert(file.clone(), state_lock);
			tasks.insert(file, task);
		}

		// Update alerts and tasks atomically
		*self.alerts.write().await = new_alerts;
		*self.tasks.write().await = tasks;

		// Update metrics with count of loaded alerts
		metrics::set_alerts_loaded(self.alerts.read().await.len());

		Ok(())
	}

	pub async fn execute_all_alerts_once(&self) -> Result<()> {
		info!("executing all alerts once");

		let resolved = self.glob_resolver.resolve()?;
		let LoadedAlerts {
			alerts,
			external_targets,
			definition_errors,
		} = load_alerts_from_paths(&resolved)?;

		// Separate event alerts from regular alerts
		let (event_alerts, regular_alerts): (Vec<_>, Vec<_>) = alerts
			.into_iter()
			.partition(|(alert, _)| matches!(alert.source, TicketSource::Event { .. }));

		// Store external targets
		*self.external_targets.write().await = external_targets.clone();

		// Update event manager
		let event_manager = EventManager::new(event_alerts, &external_targets);
		*self.event_manager.write().await = Some(event_manager.clone());

		// Send definition error events for any failed alert loads
		if !definition_errors.is_empty() {
			info!(
				count = definition_errors.len(),
				"triggering definition-error events for failed alerts"
			);
		}
		for def_err in definition_errors {
			info!(
				file = %def_err.file.display(),
				"triggering definition-error event"
			);
			let event_context = EventContext::DefinitionError {
				alert_file: def_err.file.display().to_string(),
				error_message: def_err.error.clone(),
			};
			if let Err(err) = event_manager
				.trigger_event(
					EventType::DefinitionError,
					&self.ctx,
					self.email.as_ref(),
					self.dry_run,
					event_context,
				)
				.await
			{
				error!(
					"failed to trigger definition-error event: {}",
					LogError(&err)
				);
			}
		}

		if regular_alerts.is_empty() {
			warn!("no regular alerts found");
			return Ok(());
		}

		info!(count = regular_alerts.len(), "executing alerts");

		for (alert, resolved_targets) in regular_alerts {
			let ctx = self.ctx.clone();
			let email = self.email.clone();
			let dry_run = self.dry_run;
			let file = alert.file.clone();

			debug!(?file, "executing alert");
			if let Err(err) = alert
				.execute(ctx, email.as_ref(), dry_run, &resolved_targets)
				.await
			{
				error!(?file, "error executing alert: {}", LogError(&err));
			}
		}

		Ok(())
	}

	pub async fn check_and_reload_if_paths_changed(&self) -> Result<()> {
		debug!("checking if resolved paths have changed");

		let new_resolved = self.glob_resolver.resolve()?;
		let old_resolved = self.resolved_paths.read().await;

		if new_resolved.differs_from(&old_resolved) {
			drop(old_resolved); // Release read lock before reloading
			info!("resolved paths have changed, reloading alerts");
			self.reload_alerts().await?;
		}

		Ok(())
	}

	pub async fn get_resolved_paths(&self) -> Vec<PathBuf> {
		let resolved = self.resolved_paths.read().await;
		resolved
			.all_paths()
			.iter()
			.map(|p| p.to_path_buf())
			.collect()
	}

	pub async fn reload_alerts(&self) -> Result<()> {
		info!("reloading alerts");

		// Cancel all existing tasks
		{
			let mut tasks = self.tasks.write().await;
			for (path, handle) in tasks.drain() {
				debug!(?path, "cancelling alert task");
				handle.abort();
			}
		}

		// Load and schedule new alerts
		self.load_and_schedule_alerts().await
	}

	fn spawn_alert_task(&self, alert_state: Arc<RwLock<AlertState>>) -> JoinHandle<()> {
		fn serialize_context_for_comparison(
			context: &tera::Context,
			when_changed: &crate::alert::WhenChanged,
		) -> String {
			use crate::alert::WhenChanged;

			// Get the rows from the context
			let rows = match context.get("rows") {
				Some(value) => value,
				None => return String::new(),
			};

			// Parse rows as array of objects
			let rows_array = match rows.as_array() {
				Some(arr) => arr,
				None => return serde_json::to_string(rows).unwrap_or_default(),
			};

			match when_changed {
				WhenChanged::Boolean(true) => {
					// Simple mode: serialize everything
					serde_json::to_string(rows).unwrap_or_default()
				}
				WhenChanged::Boolean(false) => {
					// Not enabled
					String::new()
				}
				WhenChanged::Detailed(config) => {
					// Filter columns based on config
					let filtered_rows: Vec<serde_json::Map<String, serde_json::Value>> = rows_array
						.iter()
						.filter_map(|row| {
							let obj = row.as_object()?;
							let mut filtered = serde_json::Map::new();

							for (key, value) in obj {
								let include = if !config.only.is_empty() {
									// Only mode: include only specified columns
									config.only.contains(key)
								} else if !config.except.is_empty() {
									// Except mode: include all except specified columns
									!config.except.contains(key)
								} else {
									// No filters specified, include all
									true
								};

								if include {
									filtered.insert(key.clone(), value.clone());
								}
							}

							Some(filtered)
						})
						.collect();

					serde_json::to_string(&filtered_rows).unwrap_or_default()
				}
			}
		}

		let ctx = self.ctx.clone();
		let email = self.email.clone();
		let dry_run = self.dry_run;
		let event_manager = self.event_manager.clone();

		tokio::spawn(async move {
			// Read initial values from state
			let (file, interval_duration) = {
				let state = alert_state.read().await;
				(
					state.definition.file.clone(),
					state.definition.interval_duration,
				)
			};
			debug!(?file, ?interval_duration, "starting alert task");

			// Add a small random delay to prevent all alerts from firing at exactly the same time
			let jitter = Duration::from_millis(rand::random::<u64>() % 5000);
			sleep(jitter).await;

			let mut ticker = interval(interval_duration);
			ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

			loop {
				ticker.tick().await;

				// Check if alert is paused
				let is_paused = {
					let state = alert_state.read().await;
					if let Some(until) = state.paused_until {
						let now = Timestamp::now();
						now < until
					} else {
						false
					}
				};

				if is_paused {
					debug!(?file, "alert is paused, skipping execution");
					continue;
				}

				debug!(?file, "executing alert");

				// Get alert definition and state
				let (alert, resolved_targets, was_triggered, always_send, when_changed) = {
					let state = alert_state.read().await;
					(
						state.definition.clone(),
						state.resolved_targets.clone(),
						state.triggered_at.is_some(),
						state.definition.always_send.clone(),
						state.definition.when_changed.clone(),
					)
				};

				// Check the triggering state
				let mut tera_ctx = crate::templates::build_context(&alert, chrono::Utc::now());
				let now = chrono::Utc::now();
				let not_before = now - alert.interval_duration;

				let is_triggering = match alert
					.read_sources(&ctx.pg_pool, not_before, &mut tera_ctx, was_triggered)
					.await
				{
					Ok(flow) => flow.is_continue(),
					Err(err) => {
						error!(?file, "error reading sources: {}", LogError(&err));
						metrics::inc_alerts_failed();

						// Trigger source_error event
						if let Some(ref event_mgr) = *event_manager.read().await {
							let event_context = EventContext::SourceError {
								alert_file: file.display().to_string(),
								error_message: format!("{err:?}"),
							};
							if let Err(event_err) = event_mgr
								.trigger_event(
									EventType::SourceError,
									&ctx,
									email.as_ref(),
									dry_run,
									event_context,
								)
								.await
							{
								error!(
									"failed to trigger source_error event: {}",
									LogError(&event_err)
								);
							}
						}
						continue;
					}
				};

				if is_triggering {
					// Alert is in triggering state
					let mut state = alert_state.write().await;

					let mut should_send = match &always_send {
						crate::alert::AlwaysSend::Boolean(true) => true,
						crate::alert::AlwaysSend::Boolean(false) => !was_triggered,
						crate::alert::AlwaysSend::Timed(config) => {
							// Check if enough time has passed since last send
							match state.last_sent_at {
								Some(last_sent_time) => {
									let now = Timestamp::now();
									let elapsed = now.duration_since(last_sent_time);
									if let Ok(after_duration) =
										jiff::SignedDuration::try_from(config.after_duration)
									{
										elapsed >= after_duration
									} else {
										false
									}
								}
								None => true, // Never sent before, should send
							}
						}
					};

					// Check when-changed logic if configured
					if should_send
						&& !matches!(when_changed, crate::alert::WhenChanged::Boolean(false))
					{
						let current_output =
							serialize_context_for_comparison(&tera_ctx, &when_changed);

						let output_changed = match &state.last_output {
							Some(prev_output) => prev_output != &current_output,
							None => true, // First run, consider it changed
						};

						if output_changed {
							debug!(?file, "output changed, will send");
							state.last_output = Some(current_output);
						} else {
							debug!(?file, "output unchanged, skipping");
							should_send = false;
						}
					}

					if should_send {
						debug!(?file, "alert triggered, sending notifications");

						// Send to targets
						for target in &resolved_targets {
							if let Err(err) = target
								.send(&alert, &mut tera_ctx, email.as_ref(), dry_run)
								.await
							{
								error!("sending: {}", LogError(&err));
							}
						}

						metrics::inc_alerts_sent();

						// Update last sent timestamp
						state.last_sent_at = Some(Timestamp::now());
					} else {
						debug!(?file, "alert still triggered, not sending (already sent)");
					}

					// Update the triggered timestamp even if we didn't send
					if !was_triggered {
						state.triggered_at = Some(Timestamp::now());
					}
				} else {
					// Alert is not in triggering state
					if was_triggered {
						info!(?file, "alert cleared");
						let mut state = alert_state.write().await;
						state.triggered_at = None;
						state.last_sent_at = None;

						// Clear last output when alert clears
						if !matches!(when_changed, crate::alert::WhenChanged::Boolean(false)) {
							state.last_output = None;
						}
					}
				}
			}
		})
	}

	pub async fn shutdown(&self) {
		info!("shutting down scheduler");
		let mut tasks = self.tasks.write().await;
		for (path, handle) in tasks.drain() {
			debug!(?path, "cancelling alert task");
			handle.abort();
		}
	}
}
