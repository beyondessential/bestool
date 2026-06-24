//! Backup run registry and the on-demand `run` endpoint.
//!
//! A backup of a given type runs at most once at a time inside the daemon, and
//! across types the daemon runs at most one backup at a time: a batch of due
//! backups is linearised through a single run slot, with a quiet period between
//! consecutive runs, rather than starting together and loading the server.
//! [`BackupRegistry::ensure_run`] is start-or-attach: it starts a run via the
//! injected [`BackupRunner`] when the type is idle, or hands back a subscription
//! to the in-flight run otherwise. A subscriber sees a replay of the latest
//! status, then live status events and a periodic heartbeat, then the run's
//! terminal event. [`BackupRegistry::running`] lists in-flight runs for the
//! daemon's status.
//!
//! This serialisation is a daemon concern only; a manual `bestool canopy backup`
//! invocation drives the backup driver directly and is bounded just by the
//! cross-process lock, not by this slot.
//!
//! The actual backup driver lives in the bestool binary; it's injected here as a
//! [`BackupRunner`] so this crate carries no backup logic of its own.

use std::{collections::HashMap, sync::Arc, time::Duration};

use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use jiff::Timestamp;
use miette::Result;
use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore, broadcast, mpsc};
use tokio_stream::wrappers::BroadcastStream;

use crate::{
	BackgroundTask, TaskContext, TaskEndpoint, TaskEndpointResponse, tasks::TaskEndpointHandler,
};

/// Runs one backup of `backup_type`, emitting JSON status events into the sink
/// and resolving when the run finishes. Injected by the binary that owns the
/// backup driver. The runner must emit a terminal `done`/`error` event before it
/// returns, so attached clients always see an end.
pub type BackupRunner = Arc<
	dyn Fn(String, mpsc::UnboundedSender<Value>) -> BoxFuture<'static, ()> + Send + Sync + 'static,
>;

const HEARTBEAT: Duration = Duration::from_secs(5);
const BROADCAST_CAPACITY: usize = 256;
/// Quiet gap held after each backup before the next queued one may start, so a
/// batch of due backups runs spread out rather than hammering the server
/// together. Deliberately coarse for now; tune once we have load data.
const QUIET_PERIOD: Duration = Duration::from_secs(30);

struct RunHandle {
	started_at: Timestamp,
	run_id: Mutex<Option<String>>,
	/// Most recent status event, replayed to a late attacher so it isn't blank.
	latest: Mutex<Value>,
	events: broadcast::Sender<Value>,
}

/// One in-flight backup, for the daemon's status.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct RunningBackup {
	pub r#type: String,
	pub run_id: Option<String>,
	pub started_at: String,
	pub latest: Value,
}

/// Tracks in-flight backup runs and fans their status out to subscribers.
pub struct BackupRegistry {
	runner: BackupRunner,
	running: Mutex<HashMap<String, Arc<RunHandle>>>,
	/// The backup types configured on this host, refreshed by the capabilities
	/// task as it (re-)reads the backups dir. Surfaced in the daemon's status so
	/// an operator can see what's registered without listing the config dir.
	configured: Mutex<Vec<String>>,
	/// Daemon-wide single-run slot: at most one backup actually runs at a time,
	/// so a batch of due backups is linearised instead of starting together.
	run_slot: Semaphore,
	/// Quiet gap held (still occupying [`run_slot`]) after each run completes.
	quiet_period: Duration,
}

impl BackupRegistry {
	pub fn new(runner: BackupRunner) -> Arc<Self> {
		Self::with_quiet_period(runner, QUIET_PERIOD)
	}

	fn with_quiet_period(runner: BackupRunner, quiet_period: Duration) -> Arc<Self> {
		Arc::new(Self {
			runner,
			running: Mutex::new(HashMap::new()),
			configured: Mutex::new(Vec::new()),
			run_slot: Semaphore::new(1),
			quiet_period,
		})
	}

	/// Record the configured backup types (called by the capabilities task each
	/// time it reads the backups dir).
	pub async fn set_configured(&self, mut types: Vec<String>) {
		types.sort();
		*self.configured.lock().await = types;
	}

	/// The configured backup types, for the status endpoint.
	pub async fn configured(&self) -> Vec<String> {
		self.configured.lock().await.clone()
	}

	/// Start a run for `backup_type`, or attach to the one already in flight.
	/// Returns a stream of JSON status events ending with the terminal event.
	pub async fn ensure_run(self: &Arc<Self>, backup_type: String) -> BoxStream<'static, Value> {
		let mut running = self.running.lock().await;

		if let Some(handle) = running.get(&backup_type) {
			let attached = json!({
				"event": "attached",
				"runId": *handle.run_id.lock().await,
				"startedAt": handle.started_at.to_string(),
				"latest": *handle.latest.lock().await,
			});
			return subscription(Some(attached), handle.events.subscribe());
		}

		let (events, receiver) = broadcast::channel(BROADCAST_CAPACITY);
		let handle = Arc::new(RunHandle {
			started_at: Timestamp::now(),
			run_id: Mutex::new(None),
			latest: Mutex::new(json!({ "event": "starting" })),
			events: events.clone(),
		});
		running.insert(backup_type.clone(), handle.clone());
		drop(running);

		let (sink, run_rx) = mpsc::unbounded_channel::<Value>();
		let runner = (self.runner)(backup_type.clone(), sink);
		let registry = self.clone();
		tokio::spawn(async move {
			// Wait for the daemon-wide run slot: only one backup runs at a time,
			// so multiple due backups queue here rather than loading the server
			// all at once. The type stays registered (and shows as running) while
			// queued, so a repeat request still attaches instead of double-starting.
			let queued = json!({ "event": "queued" });
			*handle.latest.lock().await = queued.clone();
			let _ = events.send(queued);
			let Ok(_permit) = registry.run_slot.acquire().await else {
				return; // semaphore closed; daemon shutting down
			};

			tokio::spawn(runner);
			registry
				.clone()
				.pump(backup_type, handle, run_rx, events)
				.await;

			// Hold the slot through a quiet period so the next queued backup
			// doesn't start back-to-back with this one.
			tokio::time::sleep(registry.quiet_period).await;
		});

		subscription(None, receiver)
	}

	/// Drain the runner's events into the broadcast (updating the handle's
	/// latest/run id), emitting a heartbeat between events, until the runner
	/// finishes; then deregister the type.
	async fn pump(
		self: Arc<Self>,
		backup_type: String,
		handle: Arc<RunHandle>,
		mut run_rx: mpsc::UnboundedReceiver<Value>,
		events: broadcast::Sender<Value>,
	) {
		let mut heartbeat = tokio::time::interval(HEARTBEAT);
		heartbeat.tick().await; // the first tick is immediate; skip it
		loop {
			tokio::select! {
				message = run_rx.recv() => match message {
					Some(event) => {
						if let Some(id) = event.get("runId").and_then(Value::as_str) {
							*handle.run_id.lock().await = Some(id.to_owned());
						}
						*handle.latest.lock().await = event.clone();
						let _ = events.send(event);
					}
					None => break, // runner finished
				},
				_ = heartbeat.tick() => {
					let _ = events.send(json!({ "event": "heartbeat" }));
				}
			}
		}
		self.running.lock().await.remove(&backup_type);
		// `events` drops here, closing subscribers after the terminal event.
	}

	/// In-flight runs, for the status endpoint.
	pub async fn running(&self) -> Vec<RunningBackup> {
		let map = self.running.lock().await;
		let mut out = Vec::with_capacity(map.len());
		for (backup_type, handle) in map.iter() {
			out.push(RunningBackup {
				r#type: backup_type.clone(),
				run_id: handle.run_id.lock().await.clone(),
				started_at: handle.started_at.to_string(),
				latest: handle.latest.lock().await.clone(),
			});
		}
		out
	}
}

fn subscription(
	replay: Option<Value>,
	receiver: broadcast::Receiver<Value>,
) -> BoxStream<'static, Value> {
	// Drop lagged markers; end the stream when the broadcast closes.
	let live = BroadcastStream::new(receiver).filter_map(|item| async move { item.ok() });
	match replay {
		Some(value) => futures::stream::once(async move { value })
			.chain(live)
			.boxed(),
		None => live.boxed(),
	}
}

/// Exposes `GET /tasks/backup/run?type=X` (start-or-attach, streaming status)
/// and `GET /tasks/backup/running` (in-flight runs). Holds no periodic work; the
/// registry drives runs on demand.
pub struct BackupTask {
	registry: Arc<BackupRegistry>,
}

impl BackupTask {
	pub fn new(registry: Arc<BackupRegistry>) -> Self {
		Self { registry }
	}
}

impl BackgroundTask for BackupTask {
	fn name(&self) -> &'static str {
		"backup"
	}

	fn interval(&self) -> Duration {
		// On-demand only; no periodic work. A long interval keeps the watchdog
		// from treating idleness as a hang.
		Duration::from_secs(3600)
	}

	fn run<'a>(&'a self, _ctx: &'a TaskContext) -> BoxFuture<'a, Result<()>> {
		Box::pin(async { Ok(()) })
	}

	fn http_endpoints(&self) -> Vec<TaskEndpoint> {
		let run_handler: TaskEndpointHandler = {
			let registry = self.registry.clone();
			Arc::new(move |ctx| {
				let registry = registry.clone();
				Box::pin(async move {
					let Some(backup_type) = ctx.query.get("type").cloned() else {
						return TaskEndpointResponse::Error {
							status: 400,
							message: "missing ?type= query parameter".into(),
						};
					};
					TaskEndpointResponse::JsonLines(registry.ensure_run(backup_type).await)
				})
			})
		};

		let running_handler: TaskEndpointHandler = {
			let registry = self.registry.clone();
			Arc::new(move |_ctx| {
				let registry = registry.clone();
				Box::pin(async move {
					TaskEndpointResponse::Json(json!({ "running": registry.running().await }))
				})
			})
		};

		vec![
			TaskEndpoint {
				name: "run",
				handler: run_handler,
			},
			TaskEndpoint {
				name: "running",
				handler: running_handler,
			},
		]
	}
}

#[cfg(test)]
mod tests {
	use tokio::sync::Notify;

	use super::*;

	fn event(value: &Value) -> String {
		value
			.get("event")
			.and_then(Value::as_str)
			.unwrap_or_default()
			.to_owned()
	}

	#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
	async fn start_then_attach_then_finish() {
		// A runner that announces, waits for the gate, then finishes — so a
		// second request can attach while it's in flight.
		let gate = Arc::new(Notify::new());
		let runner: BackupRunner = {
			let gate = gate.clone();
			Arc::new(move |_type, sink: mpsc::UnboundedSender<Value>| {
				let gate = gate.clone();
				Box::pin(async move {
					let _ = sink.send(json!({ "event": "started", "runId": "r1" }));
					gate.notified().await;
					let _ = sink.send(json!({ "event": "done", "success": true }));
				})
			})
		};
		let registry = BackupRegistry::new(runner);

		let mut starter = registry.ensure_run("pg".into()).await;
		// The run queues for the daemon-wide slot (free here, so instantly) before
		// the runner emits its first event.
		assert_eq!(event(&starter.next().await.unwrap()), "queued");
		assert_eq!(event(&starter.next().await.unwrap()), "started");

		// A second request for the same type attaches rather than starting a
		// second run.
		let mut attacher = registry.ensure_run("pg".into()).await;
		assert_eq!(event(&attacher.next().await.unwrap()), "attached");
		assert_eq!(registry.running().await.len(), 1);

		// Release the run; both subscribers see the terminal event and end.
		gate.notify_one();
		let starter_events: Vec<String> = starter.map(|v| event(&v)).collect().await;
		assert!(starter_events.contains(&"done".to_owned()));
		let attacher_events: Vec<String> = attacher.map(|v| event(&v)).collect().await;
		assert!(attacher_events.contains(&"done".to_owned()));
	}

	#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
	async fn distinct_types_run_sequentially_not_concurrently() {
		use std::sync::atomic::{AtomicUsize, Ordering};

		// Each run records how many are executing at once; the slot must keep that
		// at 1 even though three types are triggered together.
		let concurrent = Arc::new(AtomicUsize::new(0));
		let max_seen = Arc::new(AtomicUsize::new(0));
		let runner: BackupRunner = {
			let concurrent = concurrent.clone();
			let max_seen = max_seen.clone();
			Arc::new(move |_type, sink: mpsc::UnboundedSender<Value>| {
				let concurrent = concurrent.clone();
				let max_seen = max_seen.clone();
				Box::pin(async move {
					let now = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
					max_seen.fetch_max(now, Ordering::SeqCst);
					let _ = sink.send(json!({ "event": "started" }));
					tokio::time::sleep(Duration::from_millis(50)).await;
					concurrent.fetch_sub(1, Ordering::SeqCst);
					let _ = sink.send(json!({ "event": "done", "success": true }));
				})
			})
		};
		// No quiet period so the test stays fast; serialisation is the slot, not it.
		let registry = BackupRegistry::with_quiet_period(runner, Duration::ZERO);

		let streams =
			futures::future::join_all(["a", "b", "c"].map(|t| registry.ensure_run(t.to_owned())))
				.await;
		for mut stream in streams {
			while stream.next().await.is_some() {}
		}

		assert_eq!(
			max_seen.load(Ordering::SeqCst),
			1,
			"backups of distinct types must not overlap"
		);
	}
}
