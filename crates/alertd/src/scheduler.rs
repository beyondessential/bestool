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
	alert::{AlertDefinition, InternalContext},
	loader::{LoadedAlerts, load_alerts_from_dirs},
};

pub struct Scheduler {
	alert_dirs: Vec<PathBuf>,
	default_interval: Duration,
	ctx: Arc<InternalContext>,
	email: Option<EmailConfig>,
	dry_run: bool,
	tasks: Arc<RwLock<HashMap<PathBuf, JoinHandle<()>>>>,
}

impl Scheduler {
	pub fn new(
		alert_dirs: Vec<PathBuf>,
		default_interval: Duration,
		ctx: Arc<InternalContext>,
		email: Option<EmailConfig>,
		dry_run: bool,
	) -> Self {
		Self {
			alert_dirs,
			default_interval,
			ctx,
			email,
			dry_run,
			tasks: Arc::new(RwLock::new(HashMap::new())),
		}
	}

	pub async fn load_and_schedule_alerts(&self) -> Result<()> {
		info!("loading alerts from directories");
		let LoadedAlerts { alerts, .. } =
			load_alerts_from_dirs(&self.alert_dirs, self.default_interval)?;

		if alerts.is_empty() {
			warn!("no alerts found");
			return Ok(());
		}

		info!(count = alerts.len(), "scheduling alerts");

		let mut tasks = self.tasks.write().await;
		tasks.clear();

		for alert in alerts {
			let file = alert.file.clone();
			let task = self.spawn_alert_task(alert);
			tasks.insert(file, task);
		}

		Ok(())
	}

	pub async fn execute_all_alerts_once(&self) -> Result<()> {
		info!("executing all alerts once");
		let LoadedAlerts { alerts, .. } =
			load_alerts_from_dirs(&self.alert_dirs, self.default_interval)?;

		if alerts.is_empty() {
			warn!("no alerts found");
			return Ok(());
		}

		info!(count = alerts.len(), "executing alerts");

		for alert in alerts {
			let ctx = self.ctx.clone();
			let email = self.email.clone();
			let dry_run = self.dry_run;
			let file = alert.file.clone();

			debug!(?file, "executing alert");
			if let Err(err) = alert.execute(ctx, email.as_ref(), dry_run).await {
				error!(?file, "error executing alert: {err:?}");
			}
		}

		Ok(())
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

	fn spawn_alert_task(&self, alert: AlertDefinition) -> JoinHandle<()> {
		let ctx = self.ctx.clone();
		let email = self.email.clone();
		let dry_run = self.dry_run;
		let interval_duration = alert.interval;

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
				if let Err(err) = alert.execute(ctx.clone(), email.as_ref(), dry_run).await {
					error!(?file, "error executing alert: {err:?}");
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
