use std::{
	collections::{HashMap, HashSet},
	path::PathBuf,
	sync::Arc,
	time::Duration,
};

use jiff::Timestamp;
use miette::Result;
use tokio::{
	sync::{Mutex, Notify, RwLock},
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
	state_file::{PersistedAlertState, PersistedState},
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
	/// Was the last source read for this alert an error?
	///
	/// Used to detect the transition error → OK so a clearing canopy event
	/// can be sent for the source-error issue.
	pub source_was_erroring: bool,
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
			source_was_erroring: false,
		}
	}

	pub fn preserve_state_from(&mut self, old_state: &AlertState) {
		self.triggered_at = old_state.triggered_at;
		self.last_sent_at = old_state.last_sent_at;
		self.last_output = old_state.last_output.clone();
		self.paused_until = old_state.paused_until;
		self.source_was_erroring = old_state.source_was_erroring;
	}

	pub fn hydrate_from_persisted(&mut self, entry: &PersistedAlertState) {
		self.triggered_at = entry.triggered_at;
		self.last_sent_at = entry.last_sent_at;
		self.last_output = entry.last_output.clone();
		self.paused_until = entry.paused_until;
		self.source_was_erroring = entry.source_was_erroring;
	}

	pub fn to_persisted(&self) -> PersistedAlertState {
		PersistedAlertState {
			triggered_at: self.triggered_at,
			last_sent_at: self.last_sent_at,
			last_output: self.last_output.clone(),
			paused_until: self.paused_until,
			source_was_erroring: self.source_was_erroring,
		}
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
	state_dirty: Arc<Notify>,
	pending_hydration: Arc<Mutex<Option<PersistedState>>>,
	/// Files that errored during definition loading on the previous
	/// scheduling pass. Used to detect recovery so we can clear the
	/// corresponding canopy issue.
	last_definition_error_files: Arc<RwLock<HashSet<PathBuf>>>,
	/// Mirrors the daemon's database-down tracking.
	///
	/// Kept on the scheduler so it can be persisted in the state snapshot
	/// and restored on the next start — that way a recovery that happens
	/// while the daemon was down still produces a canopy clear.
	database_was_down: Arc<RwLock<bool>>,
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
			state_dirty: Arc::new(Notify::new()),
			pending_hydration: Arc::new(Mutex::new(None)),
			last_definition_error_files: Arc::new(RwLock::new(HashSet::new())),
			database_was_down: Arc::new(RwLock::new(false)),
		}
	}

	/// Read the persisted database-down flag.
	pub async fn database_was_down(&self) -> bool {
		*self.database_was_down.read().await
	}

	/// Update the persisted database-down flag.
	pub async fn set_database_was_down(&self, value: bool) {
		*self.database_was_down.write().await = value;
		self.state_dirty.notify_one();
	}

	/// Handle used by the persistence task to wake when alert state changes.
	pub fn state_dirty(&self) -> Arc<Notify> {
		self.state_dirty.clone()
	}

	/// Seed the next `load_and_schedule_alerts` call with persisted state.
	///
	/// Consumed on the next load (cold start). Reload calls leave previously
	/// in-memory state in place via `preserve_state_from`, so hydration is a
	/// cold-start-only concern.
	pub async fn set_pending_hydration(&self, state: PersistedState) {
		*self.pending_hydration.lock().await = Some(state);
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
			drop(state);
			self.state_dirty.notify_one();
			Ok(true)
		} else {
			Ok(false)
		}
	}

	/// Snapshot the in-memory state for serialisation by the persistence task.
	pub async fn snapshot_for_persistence(&self) -> PersistedState {
		let alerts = self.alerts.read().await;
		let mut out = HashMap::with_capacity(alerts.len());
		for (path, state_lock) in alerts.iter() {
			let state = state_lock.read().await;
			out.insert(path.clone(), state.to_persisted());
		}
		PersistedState {
			saved_at: Some(Timestamp::now()),
			alerts: out,
			database_was_down: *self.database_was_down.read().await,
			definition_error_files: self.last_definition_error_files.read().await.clone(),
		}
	}

	pub async fn get_external_targets(
		&self,
	) -> std::collections::HashMap<String, Vec<crate::targets::ExternalTarget>> {
		self.external_targets.read().await.clone()
	}

	pub async fn load_and_schedule_alerts(&self) -> Result<()> {
		info!("resolving glob patterns and loading alerts");

		// Consume any pending hydration first so subsequent code can rely on
		// hydrated daemon-level state (database_was_down, last definition
		// errors). Per-alert hydration happens later from the same value.
		let hydration = self.pending_hydration.lock().await.take();
		if let Some(ref h) = hydration {
			*self.database_was_down.write().await = h.database_was_down;
			*self.last_definition_error_files.write().await = h.definition_error_files.clone();
		}

		let resolved = self.glob_resolver.resolve()?;
		debug!(
			dirs = resolved.dirs.len(),
			files = resolved.files.len(),
			"resolved paths from globs"
		);

		let canopy_available = self.ctx.canopy_client.is_some();
		let LoadedAlerts {
			alerts,
			external_targets,
			definition_errors,
		} = load_alerts_from_paths(&resolved, canopy_available)?;

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
		let new_error_files: HashSet<PathBuf> =
			definition_errors.iter().map(|e| e.file.clone()).collect();
		for def_err in definition_errors {
			info!(
				file = %def_err.file.display(),
				"triggering definition-error event"
			);
			let entity_key = def_err.file.display().to_string();
			let event_context = EventContext::DefinitionError {
				alert_file: entity_key.clone(),
				error_message: def_err.error.clone(),
			};
			if let Err(err) = event_manager
				.trigger_event(
					EventType::DefinitionError,
					&self.ctx,
					self.email.as_ref(),
					self.dry_run,
					event_context,
					Some(&entity_key),
				)
				.await
			{
				error!(
					"failed to trigger definition-error event: {}",
					LogError(&err)
				);
			}
		}

		// Clear definition-error events for files that errored last time but
		// loaded cleanly this time.
		let mut last_def_errors = self.last_definition_error_files.write().await;
		for recovered in last_def_errors.difference(&new_error_files) {
			info!(
				file = %recovered.display(),
				"clearing definition-error event (file now loads cleanly)"
			);
			let entity_key = recovered.display().to_string();
			if let Err(err) = event_manager
				.trigger_clear(
					EventType::DefinitionError,
					&self.ctx,
					self.dry_run,
					Some(&entity_key),
				)
				.await
			{
				error!("failed to clear definition-error event: {}", LogError(&err));
			}
		}
		*last_def_errors = new_error_files;
		drop(last_def_errors);

		if regular_alerts.is_empty() {
			warn!("no regular alerts found");
			return Ok(());
		}

		info!(count = regular_alerts.len(), "scheduling regular alerts");

		// Get old alerts to preserve state across hot reload.
		let old_alerts = self.alerts.read().await.clone();

		// Hydration was taken at the top of this method; reuse it here for
		// per-alert state. On subsequent reloads it'll be None and
		// preserve_state_from carries in-memory state forward instead.
		let mut hydrated_count = 0usize;

		let mut new_alerts = HashMap::new();
		let mut tasks = HashMap::new();

		for (definition, resolved_targets) in regular_alerts {
			let file = definition.file.clone();

			// Create new alert state
			let mut new_state = AlertState::new(definition.clone(), resolved_targets.clone());

			if let Some(old_alert_lock) = old_alerts.get(&file) {
				let old_state = old_alert_lock.read().await;
				new_state.preserve_state_from(&old_state);
			} else if let Some(entry) = hydration.as_ref().and_then(|h| h.alerts.get(&file)) {
				new_state.hydrate_from_persisted(entry);
				hydrated_count += 1;
			}

			let state_lock = Arc::new(RwLock::new(new_state));
			let task = self.spawn_alert_task(state_lock.clone());

			new_alerts.insert(file.clone(), state_lock);
			tasks.insert(file, task);
		}

		if hydrated_count > 0 {
			info!(count = hydrated_count, "hydrated alert state from disk");
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
		let canopy_available = self.ctx.canopy_client.is_some();
		let LoadedAlerts {
			alerts,
			external_targets,
			definition_errors,
		} = load_alerts_from_paths(&resolved, canopy_available)?;

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
			let entity_key = def_err.file.display().to_string();
			let event_context = EventContext::DefinitionError {
				alert_file: entity_key.clone(),
				error_message: def_err.error.clone(),
			};
			if let Err(err) = event_manager
				.trigger_event(
					EventType::DefinitionError,
					&self.ctx,
					self.email.as_ref(),
					self.dry_run,
					event_context,
					Some(&entity_key),
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
		let ctx = self.ctx.clone();
		let email = self.email.clone();
		let dry_run = self.dry_run;
		let event_manager = self.event_manager.clone();
		let state_dirty = self.state_dirty.clone();

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
				metrics::record_activity();

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
				let (
					alert,
					resolved_targets,
					was_triggered,
					was_source_erroring,
					always_send,
					when_changed,
				) = {
					let state = alert_state.read().await;
					(
						state.definition.clone(),
						state.resolved_targets.clone(),
						state.triggered_at.is_some(),
						state.source_was_erroring,
						state.definition.always_send.clone(),
						state.definition.when_changed.clone(),
					)
				};

				// Check the triggering state
				let now = jiff::Timestamp::now();
				let mut tera_ctx = crate::templates::build_context(&alert, now);
				let not_before = now - alert.interval_duration;

				let mut state_changed = false;

				let is_triggering = match alert
					.read_sources(&ctx.pg_pool, not_before, &mut tera_ctx, was_triggered)
					.await
				{
					Ok(flow) => {
						// Source recovered: clear the source-error canopy issue.
						if was_source_erroring {
							info!(?file, "source recovered, clearing source-error event");
							if let Some(ref event_mgr) = *event_manager.read().await {
								let entity_key = file.display().to_string();
								if let Err(event_err) = event_mgr
									.trigger_clear(
										EventType::SourceError,
										&ctx,
										dry_run,
										Some(&entity_key),
									)
									.await
								{
									error!(
										"failed to clear source_error event: {}",
										LogError(&event_err)
									);
								}
							}
							let mut state = alert_state.write().await;
							state.source_was_erroring = false;
							state_changed = true;
						}
						flow.is_continue()
					}
					Err(err) => {
						error!(?file, "error reading sources: {}", LogError(&err));
						metrics::inc_alerts_failed();

						// Trigger source_error event
						if let Some(ref event_mgr) = *event_manager.read().await {
							let entity_key = file.display().to_string();
							let event_context = EventContext::SourceError {
								alert_file: entity_key.clone(),
								error_message: format!("{err:?}"),
							};
							if let Err(event_err) = event_mgr
								.trigger_event(
									EventType::SourceError,
									&ctx,
									email.as_ref(),
									dry_run,
									event_context,
									Some(&entity_key),
								)
								.await
							{
								error!(
									"failed to trigger source_error event: {}",
									LogError(&event_err)
								);
							}
						}
						if !was_source_erroring {
							let mut state = alert_state.write().await;
							state.source_was_erroring = true;
							state_dirty.notify_one();
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
						let current_digest =
							digest_context_for_comparison(&tera_ctx, &when_changed);

						let output_changed = match &state.last_output {
							Some(prev_digest) => prev_digest != &current_digest,
							None => true, // First run, consider it changed
						};

						if output_changed {
							debug!(?file, "output changed, will send");
							state.last_output = Some(current_digest);
							state_changed = true;
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
								.send(&alert, &mut tera_ctx, email.as_ref(), &ctx, dry_run)
								.await
							{
								error!("sending: {}", LogError(&err));
							}
						}

						metrics::inc_alerts_sent();

						// Update last sent timestamp
						state.last_sent_at = Some(Timestamp::now());
						state_changed = true;
					} else {
						debug!(?file, "alert still triggered, not sending (already sent)");
					}

					// Update the triggered timestamp even if we didn't send
					if !was_triggered {
						state.triggered_at = Some(Timestamp::now());
						state_changed = true;
					}
				} else {
					// Alert is not in triggering state
					if was_triggered {
						info!(?file, "alert is no longer triggering, sending clear");
						let all_cleared =
							send_clear_to_targets(&resolved_targets, &alert, &ctx, dry_run).await;
						if all_cleared {
							let mut state = alert_state.write().await;
							state.triggered_at = None;
							state.last_sent_at = None;

							// Clear last output when alert clears
							if !matches!(when_changed, crate::alert::WhenChanged::Boolean(false)) {
								state.last_output = None;
							}
							state_changed = true;
						} else {
							warn!(
								?file,
								"send_clear failed for one or more targets; will retry on next tick"
							);
						}
					}
				}

				if state_changed {
					state_dirty.notify_one();
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

/// Compute a stable digest of the alert's row output for `when-changed`
/// comparison. Hashing rather than storing the serialised rows keeps state.json
/// from growing without bound when a high-cardinality SQL alert holds many rows.
fn digest_context_for_comparison(
	context: &tera::Context,
	when_changed: &crate::alert::WhenChanged,
) -> String {
	use crate::alert::WhenChanged;

	let rows = match context.get("rows") {
		Some(value) => value,
		None => return blake3_hex(b""),
	};

	let rows_array = match rows.as_array() {
		Some(arr) => arr,
		None => return blake3_hex(serde_json::to_string(rows).unwrap_or_default().as_bytes()),
	};

	match when_changed {
		WhenChanged::Boolean(true) => {
			blake3_hex(serde_json::to_string(rows).unwrap_or_default().as_bytes())
		}
		WhenChanged::Boolean(false) => blake3_hex(b""),
		WhenChanged::Detailed(config) => {
			let filtered_rows: Vec<serde_json::Map<String, serde_json::Value>> = rows_array
				.iter()
				.filter_map(|row| {
					let obj = row.as_object()?;
					let mut filtered = serde_json::Map::new();
					for (key, value) in obj {
						let include = if !config.only.is_empty() {
							config.only.contains(key)
						} else if !config.except.is_empty() {
							!config.except.contains(key)
						} else {
							true
						};
						if include {
							filtered.insert(key.clone(), value.clone());
						}
					}
					Some(filtered)
				})
				.collect();
			blake3_hex(
				serde_json::to_string(&filtered_rows)
					.unwrap_or_default()
					.as_bytes(),
			)
		}
	}
}

fn blake3_hex(bytes: &[u8]) -> String {
	blake3::hash(bytes).to_hex().to_string()
}

/// Send a clear notification to every resolved target.
///
/// Returns `true` if every target's `send_clear` succeeded; `false` if any
/// failed. Caller should leave `triggered_at` set when this returns `false`
/// so the next scheduler tick retries — otherwise a transient failure
/// (network blip, canopy 5xx, TLS handshake during cert rollover) silently
/// leaves stateful targets like canopy stuck on `active=true`.
async fn send_clear_to_targets(
	targets: &[ResolvedTarget],
	alert: &AlertDefinition,
	ctx: &InternalContext,
	dry_run: bool,
) -> bool {
	let mut all_ok = true;
	for target in targets {
		if let Err(err) = target.send_clear(alert, ctx, dry_run).await {
			error!("sending clear: {}", LogError(&err));
			all_ok = false;
		}
	}
	all_ok
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::{
		canopy::{DEFAULT_CANOPY_URL, Severity},
		targets::{CanopyConfig, TargetCanopy, TargetConnection, TargetEmail},
	};

	async fn test_internal_context() -> InternalContext {
		let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
		let pg_pool = bestool_postgres::pool::create_pool(&db_url, "bestool-alertd-test")
			.await
			.unwrap();
		InternalContext {
			pg_pool,
			http_client: reqwest::Client::new(),
			canopy_client: None,
		}
	}

	fn email_target() -> ResolvedTarget {
		ResolvedTarget {
			target_id: "ops".into(),
			subject: None,
			template: "body".into(),
			conn: TargetConnection::Email(TargetEmail {
				addresses: vec!["ops@example.com".into()],
			}),
		}
	}

	fn canopy_target() -> ResolvedTarget {
		ResolvedTarget {
			target_id: "default".into(),
			subject: None,
			template: "body".into(),
			conn: TargetConnection::Canopy(TargetCanopy {
				canopy: CanopyConfig {
					url: DEFAULT_CANOPY_URL.parse().unwrap(),
					source: "test".into(),
					severity: Some(Severity::Error),
				},
			}),
		}
	}

	fn test_alert() -> AlertDefinition {
		AlertDefinition {
			file: "test.yml".into(),
			..Default::default()
		}
	}

	#[tokio::test]
	async fn send_clear_to_targets_returns_true_for_non_stateful_only() {
		let ctx = test_internal_context().await;
		let targets = vec![email_target()];
		assert!(send_clear_to_targets(&targets, &test_alert(), &ctx, false).await);
	}

	#[tokio::test]
	async fn send_clear_to_targets_returns_false_when_canopy_lacks_client() {
		// Canopy target configured but ctx.canopy_client is None — send_clear
		// returns Err and the helper should report failure so the caller
		// leaves triggered_at set and retries on the next tick.
		let ctx = test_internal_context().await;
		let targets = vec![canopy_target()];
		assert!(!send_clear_to_targets(&targets, &test_alert(), &ctx, false).await);
	}

	#[tokio::test]
	async fn send_clear_to_targets_reports_failure_even_when_only_one_target_fails() {
		// Mixed bag: email (succeeds) + canopy with no client (fails).
		// Helper must return false so the scheduler keeps the alert in the
		// triggered state and retries.
		let ctx = test_internal_context().await;
		let targets = vec![email_target(), canopy_target()];
		assert!(!send_clear_to_targets(&targets, &test_alert(), &ctx, false).await);
	}

	#[tokio::test]
	async fn send_clear_to_targets_in_dry_run_succeeds_regardless_of_canopy_client() {
		// In dry-run mode, canopy never tries to use the missing client.
		let ctx = test_internal_context().await;
		let targets = vec![canopy_target()];
		assert!(send_clear_to_targets(&targets, &test_alert(), &ctx, true).await);
	}

	#[test]
	fn digest_is_stable_hex_length() {
		let mut ctx = tera::Context::new();
		ctx.insert("rows", &serde_json::json!([{"a": 1}]));
		let d = digest_context_for_comparison(&ctx, &crate::alert::WhenChanged::Boolean(true));
		assert_eq!(d.len(), 64, "blake3 hex digest should be 64 chars");
		assert!(d.chars().all(|c| c.is_ascii_hexdigit()));
	}

	#[test]
	fn digest_does_not_grow_with_row_count() {
		// A digest of one row and a digest of ten thousand rows should both
		// be the same fixed size — this is the whole point of hashing.
		let mut small = tera::Context::new();
		small.insert("rows", &serde_json::json!([{"a": 1}]));

		let big_rows: Vec<serde_json::Value> = (0..10_000)
			.map(|i| serde_json::json!({"a": i, "b": "padding-".repeat(8)}))
			.collect();
		let mut big = tera::Context::new();
		big.insert("rows", &serde_json::Value::Array(big_rows));

		let d_small =
			digest_context_for_comparison(&small, &crate::alert::WhenChanged::Boolean(true));
		let d_big = digest_context_for_comparison(&big, &crate::alert::WhenChanged::Boolean(true));
		assert_eq!(d_small.len(), d_big.len());
		assert_ne!(d_small, d_big);
	}

	#[test]
	fn digest_changes_when_rows_change() {
		let mut a = tera::Context::new();
		a.insert("rows", &serde_json::json!([{"x": 1}]));
		let mut b = tera::Context::new();
		b.insert("rows", &serde_json::json!([{"x": 2}]));
		assert_ne!(
			digest_context_for_comparison(&a, &crate::alert::WhenChanged::Boolean(true)),
			digest_context_for_comparison(&b, &crate::alert::WhenChanged::Boolean(true)),
		);
	}

	#[test]
	fn digest_only_filter_ignores_other_columns() {
		use crate::alert::{WhenChanged, WhenChangedConfig};
		let cfg = WhenChanged::Detailed(WhenChangedConfig {
			only: vec!["id".into()],
			except: Vec::new(),
		});

		let mut a = tera::Context::new();
		a.insert("rows", &serde_json::json!([{"id": 1, "noise": "x"}]));
		let mut b = tera::Context::new();
		b.insert("rows", &serde_json::json!([{"id": 1, "noise": "y"}]));

		assert_eq!(
			digest_context_for_comparison(&a, &cfg),
			digest_context_for_comparison(&b, &cfg),
			"changes to non-`only` columns should not change the digest"
		);
	}

	#[test]
	fn digest_except_filter_ignores_named_columns() {
		use crate::alert::{WhenChanged, WhenChangedConfig};
		let cfg = WhenChanged::Detailed(WhenChangedConfig {
			except: vec!["ts".into()],
			only: Vec::new(),
		});

		let mut a = tera::Context::new();
		a.insert("rows", &serde_json::json!([{"id": 1, "ts": "2026-01-01"}]));
		let mut b = tera::Context::new();
		b.insert("rows", &serde_json::json!([{"id": 1, "ts": "2026-02-01"}]));

		assert_eq!(
			digest_context_for_comparison(&a, &cfg),
			digest_context_for_comparison(&b, &cfg),
			"changes to excluded columns should not change the digest"
		);
	}
}
