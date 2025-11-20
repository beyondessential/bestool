use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use miette::Result;
use tokio::{
	sync::RwLock,
	task::JoinHandle,
	time::{interval, sleep},
};
use tracing::{debug, error, info, warn};

use crate::{
	EmailConfig,
	alert::{AlertDefinition, InternalContext, TicketSource},
	events::{EventContext, EventManager, EventType},
	glob_resolver::{GlobResolver, ResolvedPaths},
	loader::{LoadedAlerts, load_alerts_from_paths},
	metrics,
	targets::ResolvedTarget,
};

pub struct Scheduler {
	glob_resolver: GlobResolver,
	resolved_paths: Arc<RwLock<ResolvedPaths>>,
	default_interval: Duration,
	ctx: Arc<InternalContext>,
	email: Option<EmailConfig>,
	dry_run: bool,
	tasks: Arc<RwLock<HashMap<PathBuf, JoinHandle<()>>>>,
	event_manager: Arc<RwLock<Option<EventManager>>>,
}

impl Scheduler {
	pub fn new(
		alert_globs: Vec<String>,
		default_interval: Duration,
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
			default_interval,
			ctx,
			email,
			dry_run,
			tasks: Arc::new(RwLock::new(HashMap::new())),
			event_manager: Arc::new(RwLock::new(None)),
		}
	}

	pub fn get_event_manager(&self) -> Arc<RwLock<Option<EventManager>>> {
		self.event_manager.clone()
	}

	pub async fn get_loaded_alerts(&self) -> Vec<PathBuf> {
		let tasks = self.tasks.read().await;
		let mut files: Vec<PathBuf> = tasks.keys().cloned().collect();
		files.sort();
		files
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
		} = load_alerts_from_paths(&resolved, self.default_interval)?;

		// Update resolved paths
		*self.resolved_paths.write().await = resolved;

		// Separate event alerts from regular alerts
		let (event_alerts, regular_alerts): (Vec<_>, Vec<_>) = alerts
			.into_iter()
			.partition(|(alert, _)| matches!(alert.source, TicketSource::Event { .. }));

		// Create event manager with event alerts and external targets
		let event_manager = EventManager::new(event_alerts, &external_targets);
		*self.event_manager.write().await = Some(event_manager);

		if regular_alerts.is_empty() {
			warn!("no regular alerts found");
			return Ok(());
		}

		info!(count = regular_alerts.len(), "scheduling regular alerts");

		let mut tasks = self.tasks.write().await;
		tasks.clear();

		for (alert, resolved_targets) in regular_alerts {
			let file = alert.file.clone();
			let task = self.spawn_alert_task(alert, resolved_targets);
			tasks.insert(file, task);
		}

		// Update metrics with count of loaded alerts
		metrics::set_alerts_loaded(tasks.len());

		Ok(())
	}

	pub async fn execute_all_alerts_once(&self) -> Result<()> {
		info!("executing all alerts once");

		let resolved = self.glob_resolver.resolve()?;
		let LoadedAlerts {
			alerts,
			external_targets,
		} = load_alerts_from_paths(&resolved, self.default_interval)?;

		// Separate event alerts from regular alerts
		let (event_alerts, regular_alerts): (Vec<_>, Vec<_>) = alerts
			.into_iter()
			.partition(|(alert, _)| matches!(alert.source, TicketSource::Event { .. }));

		// Update event manager
		let event_manager = EventManager::new(event_alerts, &external_targets);
		*self.event_manager.write().await = Some(event_manager);

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
				error!(?file, "error executing alert: {err:?}");
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

	fn spawn_alert_task(
		&self,
		alert: AlertDefinition,
		resolved_targets: Vec<ResolvedTarget>,
	) -> JoinHandle<()> {
		let ctx = self.ctx.clone();
		let email = self.email.clone();
		let dry_run = self.dry_run;
		let interval_duration = alert.interval;

		let event_manager = self.event_manager.clone();

		tokio::spawn(async move {
			let file = alert.file.clone();
			debug!(?file, ?interval_duration, "starting alert task");

			// Add a small random delay to prevent all alerts from firing at exactly the same time
			let jitter = Duration::from_millis(rand::random::<u64>() % 5000);
			sleep(jitter).await;

			let mut ticker = interval(interval_duration);
			ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

			loop {
				ticker.tick().await;

				debug!(?file, "executing alert");
				match alert
					.execute(ctx.clone(), email.as_ref(), dry_run, &resolved_targets)
					.await
				{
					Ok(()) => {
						metrics::inc_alerts_sent();
					}
					Err(err) => {
						error!(?file, "error executing alert: {err:?}");
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
								error!("failed to trigger source_error event: {event_err:?}");
							}
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
