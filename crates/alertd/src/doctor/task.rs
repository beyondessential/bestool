use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use bestool_canopy::schema::{CheckSeverity, StatusPayload};
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use jiff::Timestamp;
use miette::{Result, miette};
use serde_json::{Value, json};
use tokio::sync::{Mutex, mpsc};
use tracing::warn;

use crate::doctor::{
	self,
	check::{Check, CheckStatus},
	progress::DoctorEvent,
	stat::{MetricsSnapshot, StatusCounts},
};
use crate::tasks::TaskEndpointHandler;
use crate::{BackgroundTask, TaskContext, TaskEndpoint, TaskEndpointResponse};

const DOCTOR_INTERVAL: Duration = Duration::from_secs(60);

/// Invoked with the `backup_now` list from canopy's status response.
///
/// alertd has no backup logic of its own; the bestool binary supplies this to
/// run the in-process backup driver. Fire-and-forget: the callback spawns its
/// own work and guards against overlapping runs.
pub type BackupDispatch = Arc<dyn Fn(Vec<String>) + Send + Sync>;

/// Apply the effective-severity ceiling to a single streamed check, if a
/// mapping is available (a no-op otherwise). Mirrors
/// [`doctor::SweepResult::apply_severities`] for the one-check streaming case.
fn cap_check(check: Check, severities: Option<&HashMap<String, CheckSeverity>>) -> Check {
	match severities {
		Some(map) => {
			let ceiling = doctor::sweep::severity_ceiling(map, check.name);
			Check {
				status: check.status.cap_to(ceiling),
				..check
			}
		}
		None => check,
	}
}

/// Periodic doctor sweep, plus on-demand `latest` / `recompute` HTTP endpoints.
///
/// The outer struct just holds an `Arc<Inner>` so we can hand inner clones to
/// the `'static` HTTP endpoint handlers without forcing the trait method
/// `http_endpoints` to take `self: Arc<Self>`.
pub struct DoctorTask {
	inner: Arc<DoctorTaskInner>,
}

/// Where a sweep's Tamanu context comes from.
enum TamanuSource {
	/// Whatever was handed to [`DoctorTask::new`], for the lifetime of the
	/// daemon. This build has no Tamanu integration wired up, so there's nothing
	/// to discover.
	Fixed,
	/// Re-discovered before every sweep from `root` (the `--root` override, when
	/// one was given), so an in-place upgrade lands without a daemon restart.
	Discover { root: Option<PathBuf> },
}

struct DoctorTaskInner {
	binary_version: String,
	/// Tamanu context for the next sweep, refreshed by
	/// [`DoctorTaskInner::resolve_tamanu`] when discovery is enabled. `None` on
	/// hosts with no Tamanu deployment: sweeps still run (and post), with all
	/// Tamanu-dependent checks skipped.
	tamanu: Mutex<Option<doctor::SweepTamanu>>,
	tamanu_source: TamanuSource,
	/// `SELECT version()` result, populated on the first tick that succeeds in
	/// reaching the database. Stable for the lifetime of the PG instance, so we
	/// reuse it across ticks instead of re-querying every minute.
	pg_version_cache: Mutex<Option<String>>,
	/// Latest sweep, captured on every successful tick. Served by the `latest`
	/// HTTP endpoint so `bestool tamanu doctor` can read what the daemon
	/// already computed instead of re-running the checks itself.
	latest: Mutex<Option<LatestSweep>>,
	/// Effective-severity ceilings canopy last returned on a status push, keyed
	/// by check name. `None` until the first successful push. Applied to the
	/// sweeps this daemon serves locally (`latest` / `recompute`) so operators
	/// see the same severities the CLI and canopy show; the payload posted to
	/// canopy stays raw. See [`doctor::SweepResult::apply_severities`].
	check_severities: Mutex<Option<HashMap<String, CheckSeverity>>>,
	/// Runs the backup driver for the types canopy asks for via `backup_now`.
	/// `None` when backups aren't compiled in.
	backup_dispatch: Option<BackupDispatch>,
}

#[derive(Clone)]
struct LatestSweep {
	computed_at: Timestamp,
	/// The raw sweep result, kept typed so the `latest` endpoint can apply the
	/// current severity ceilings on read rather than baking them in at sweep time.
	sweep: doctor::SweepResult,
}

impl DoctorTask {
	pub fn new(binary_version: String, tamanu: Option<doctor::SweepTamanu>) -> Self {
		Self {
			inner: Arc::new(DoctorTaskInner {
				binary_version,
				tamanu: Mutex::new(tamanu),
				tamanu_source: TamanuSource::Fixed,
				pg_version_cache: Mutex::new(None),
				latest: Mutex::new(None),
				check_severities: Mutex::new(None),
				backup_dispatch: None,
			}),
		}
	}

	/// Re-discover the Tamanu install before every sweep instead of reusing the
	/// context passed to [`DoctorTask::new`], with `root` as the `--root`
	/// override.
	///
	/// Call right after [`DoctorTask::new`] (before the task is shared).
	pub fn with_tamanu_discovery(self, root: Option<PathBuf>) -> Self {
		let mut inner =
			Arc::try_unwrap(self.inner).unwrap_or_else(|_| panic!("DoctorTask already shared"));
		inner.tamanu_source = TamanuSource::Discover { root };
		Self {
			inner: Arc::new(inner),
		}
	}

	/// Attach the backup dispatcher invoked when canopy requests a backup.
	///
	/// Call right after [`DoctorTask::new`] (before the task is shared).
	pub fn with_backup_dispatch(self, dispatch: BackupDispatch) -> Self {
		let mut inner =
			Arc::try_unwrap(self.inner).unwrap_or_else(|_| panic!("DoctorTask already shared"));
		inner.backup_dispatch = Some(dispatch);
		Self {
			inner: Arc::new(inner),
		}
	}

	/// A cloneable handle the HTTP `/metrics` endpoint uses to read the latest
	/// sweep's declared stats and status census.
	pub fn metrics_handle(&self) -> DoctorMetricsHandle {
		DoctorMetricsHandle {
			inner: self.inner.clone(),
		}
	}
}

/// Read-only view of the doctor task's latest sweep for the metrics endpoint.
///
/// Capping is applied on read (via [`DoctorTaskInner::capped`]) so the status
/// census reflects canopy's current severity ceilings, matching what the
/// `latest` endpoint and the CLI show.
#[derive(Clone)]
pub struct DoctorMetricsHandle {
	inner: Arc<DoctorTaskInner>,
}

impl DoctorMetricsHandle {
	/// The latest sweep rendered into a [`MetricsSnapshot`], or `None` if the
	/// daemon hasn't completed a sweep yet.
	pub async fn snapshot(&self) -> Option<MetricsSnapshot> {
		let latest = self.inner.latest.lock().await.clone()?;
		let sweep = self.inner.capped(latest.sweep).await;

		let counts = census(&sweep.results);
		let stats = sweep
			.results
			.iter()
			.flat_map(|(check, _)| check.stats.iter().map(|stat| (check.name, stat.clone())))
			.collect();

		Some(MetricsSnapshot {
			computed_at: latest.computed_at,
			stats,
			counts,
		})
	}
}

/// Tally check outcomes into a [`StatusCounts`]. Expects statuses already capped
/// to canopy's ceilings, so the census matches what operators see elsewhere.
fn census(results: &[(Check, bool)]) -> StatusCounts {
	let mut counts = StatusCounts::default();
	for (check, _) in results {
		match &check.status {
			CheckStatus::Pass => counts.passing += 1,
			CheckStatus::Warning(_) => counts.warning += 1,
			CheckStatus::Fail(_) => counts.failing += 1,
			CheckStatus::Skip(_) => counts.skipped += 1,
			CheckStatus::Broken(_) => counts.broken += 1,
		}
	}
	counts
}

impl DoctorTaskInner {
	/// The Tamanu context to sweep against.
	///
	/// A Tamanu upgrade replaces the version, the install root and the config
	/// under a running daemon. Resolving once at startup would pin us to the
	/// pre-upgrade snapshot for the life of the process: the status payload would
	/// keep reporting the old `tamanuVersion` and `tamanuRoot`, and every
	/// version-aware check would compare against a stale baseline. So re-discover
	/// per sweep, keeping the last good answer when discovery errors — a
	/// transient failure shouldn't blank out every Tamanu check.
	async fn resolve_tamanu(&self) -> Option<doctor::SweepTamanu> {
		let TamanuSource::Discover { root } = &self.tamanu_source else {
			return self.tamanu.lock().await.clone();
		};

		self.apply_discovery(doctor::discover_sweep_tamanu(root.as_deref()).await)
			.await
	}

	/// Fold a discovery attempt into the stored context and return what the sweep
	/// should use. `Ok(None)` is recorded as-is: Tamanu really is gone from this
	/// host, and continuing to report the install we last saw would be a lie.
	async fn apply_discovery(
		&self,
		discovered: Result<Option<doctor::SweepTamanu>>,
	) -> Option<doctor::SweepTamanu> {
		let mut guard = self.tamanu.lock().await;
		match discovered {
			Ok(resolved) => *guard = resolved,
			Err(err) => warn!(
				%err,
				"could not resolve the Tamanu install; sweeping against the last known context"
			),
		}
		guard.clone()
	}

	async fn run_sweep(
		self: &Arc<Self>,
		ctx: &TaskContext,
		progress: Option<doctor::progress::ProgressSender>,
		enable_heal: bool,
	) -> Result<doctor::SweepResult> {
		let cached = self.pg_version_cache.lock().await.clone();
		let tamanu = self.resolve_tamanu().await;
		// Hand checks the shared canopy client so a heal action can reach canopy;
		// only the periodic tick enables healing, so an on-demand recompute
		// driven by `doctor --fresh` stays side-effect-free. See
		// [`crate::doctor::heal`].
		let sweep = doctor::perform_sweep(
			&self.binary_version,
			tamanu,
			ctx.http_client.clone(),
			&[],
			&[],
			cached,
			progress,
			ctx.canopy_client.clone(),
			enable_heal,
		)
		.await?;

		if let Some(ref version) = sweep.pg_version {
			let mut guard = self.pg_version_cache.lock().await;
			if guard.is_none() {
				*guard = Some(version.clone());
			}
		}

		let latest = LatestSweep {
			computed_at: Timestamp::now(),
			sweep: sweep.clone(),
		};
		*self.latest.lock().await = Some(latest);

		Ok(sweep)
	}

	/// Snapshot the severity ceilings canopy last returned, if any.
	async fn severities_snapshot(&self) -> Option<HashMap<String, CheckSeverity>> {
		self.check_severities.lock().await.clone()
	}

	/// Apply the current severity ceilings to a sweep, if we have any. A no-op
	/// (leaving the raw sweep) until canopy has returned a mapping.
	async fn capped(&self, mut sweep: doctor::SweepResult) -> doctor::SweepResult {
		if let Some(severities) = self.severities_snapshot().await {
			sweep.apply_severities(&severities);
		}
		sweep
	}

	async fn tick(self: &Arc<Self>, ctx: &TaskContext) -> Result<()> {
		let sweep = self.run_sweep(ctx, None, true).await?;

		let Some(server_id) = sweep.server_id else {
			warn!("no metaServerId available; skipping canopy status push");
			return Ok(());
		};

		let Some(canopy) = ctx.canopy_client.as_ref() else {
			warn!("no canopy client available; skipping canopy status push");
			return Ok(());
		};

		// The sweep builds the payload as a free-form JSON object; the canopy
		// client takes the typed `StatusPayload`, whose flattened `extra` map
		// carries the server facts alongside the reserved `health` array.
		let payload: StatusPayload = serde_json::from_value(sweep.payload)
			.map_err(|err| miette!("building canopy status payload: {err}"))?;
		let response = canopy
			.status(&server_id, &payload)
			.await
			.map_err(|err| miette!("posting doctor status to canopy: {err}"))?;

		// Cache the effective-severity ceilings for the sweeps we serve locally.
		// The payload we just posted stays raw: canopy is the source of truth and
		// maps severities itself.
		*self.check_severities.lock().await = Some(response.check_severities);

		// Refresh the on-disk tags cache from the effective tags canopy echoes
		// back. Checks that read tags (e.g. billing_tags) and offline `canopy
		// tags` consult this cache; without this the daemon would never update it.
		let tags = response.tags.0.into_iter().collect();
		if let Err(err) = bestool_tamanu::server_info::save_cached_tags(&tags) {
			warn!(%err, "could not refresh tags cache from status response");
		}

		let backup_now = response.backup_now;

		if !backup_now.is_empty() {
			match &self.backup_dispatch {
				Some(dispatch) => dispatch(backup_now),
				None => warn!(
					?backup_now,
					"canopy requested a backup but no backup dispatcher is configured"
				),
			}
		}

		Ok(())
	}

	/// `GET /tasks/doctor/latest` — return the last sweep this daemon
	/// computed, or 404 if it hasn't ticked yet.
	async fn endpoint_latest(self: Arc<Self>) -> TaskEndpointResponse {
		let snapshot = self.latest.lock().await.clone();
		match snapshot {
			Some(s) => {
				let sweep = self.capped(s.sweep).await;
				TaskEndpointResponse::Json(json!({
					"computedAt": s.computed_at.to_string(),
					"serverId": sweep.server_id,
					"payload": sweep.payload,
				}))
			}
			None => TaskEndpointResponse::Error {
				status: 503,
				message: "no doctor sweep cached yet (daemon may have just started)".into(),
			},
		}
	}

	/// `GET /tasks/doctor/recompute` — drive a fresh sweep and stream each
	/// progress event back as NDJSON. Final line is the full sweep result.
	async fn endpoint_recompute(self: Arc<Self>, ctx: TaskContext) -> TaskEndpointResponse {
		let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<DoctorEvent>();
		let (out_tx, out_rx) = mpsc::unbounded_channel::<Value>();

		// Snapshot the ceilings once so the streamed per-check events and the
		// final payload are capped consistently, matching what `latest` serves.
		let severities = self.severities_snapshot().await;

		let task_self = self.clone();
		tokio::spawn(async move {
			let progress_forward_tx = out_tx.clone();
			let stream_severities = severities.clone();
			let forwarder = tokio::spawn(async move {
				while let Some(event) = progress_rx.recv().await {
					let DoctorEvent::Completed(check) = event;
					let check = cap_check(check, stream_severities.as_ref());
					let _ = progress_forward_tx.send(json!({
						"event": "check",
						"check": check.to_streaming_json(),
					}));
				}
			});

			match task_self.run_sweep(&ctx, Some(progress_tx), false).await {
				Ok(mut sweep) => {
					if let Some(severities) = &severities {
						sweep.apply_severities(severities);
					}
					// Make sure all `Completed` events arrived before we emit
					// `done` — perform_sweep drops the sender on return, which
					// closes the forwarder loop above.
					let _ = forwarder.await;
					let _ = out_tx.send(json!({
						"event": "done",
						"computedAt": Timestamp::now().to_string(),
						"serverId": sweep.server_id,
						"payload": sweep.payload,
					}));
				}
				Err(err) => {
					let _ = forwarder.await;
					let _ = out_tx.send(json!({
						"event": "error",
						"message": format!("{err:?}"),
					}));
				}
			}
		});

		let stream: BoxStream<'static, Value> =
			Box::pin(tokio_stream::wrappers::UnboundedReceiverStream::new(out_rx).map(|v| v));
		TaskEndpointResponse::JsonLines(stream)
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
		let inner = self.inner.clone();
		Box::pin(async move { inner.tick(ctx).await })
	}

	fn http_endpoints(&self) -> Vec<TaskEndpoint> {
		let latest_handler: TaskEndpointHandler = {
			let inner = self.inner.clone();
			Arc::new(move |_ctx| {
				let inner = inner.clone();
				Box::pin(async move { inner.endpoint_latest().await })
			})
		};

		let recompute_handler: TaskEndpointHandler = {
			let inner = self.inner.clone();
			Arc::new(move |ctx| {
				let inner = inner.clone();
				Box::pin(async move { inner.endpoint_recompute(ctx).await })
			})
		};

		vec![
			TaskEndpoint {
				name: "latest",
				handler: latest_handler,
			},
			TaskEndpoint {
				name: "recompute",
				handler: recompute_handler,
			},
		]
	}
}

#[cfg(test)]
mod tests {
	use node_semver::Version;

	use bestool_tamanu::config::{Database, TamanuConfig};

	use super::*;
	use crate::doctor::check::CheckStatus;

	const DB_URL: &str = "postgres://u:p@localhost/tamanu";

	fn sweep_tamanu(version: &str) -> doctor::SweepTamanu {
		doctor::SweepTamanu {
			version: Version::parse(version).unwrap(),
			root: PathBuf::from("/opt/tamanu"),
			config: Arc::new(TamanuConfig::from_database(
				Database::from_url(DB_URL).unwrap(),
			)),
			database_url: DB_URL.into(),
			has_install: true,
			is_tamanu: true,
		}
	}

	fn inner(tamanu: Option<doctor::SweepTamanu>, tamanu_source: TamanuSource) -> DoctorTaskInner {
		DoctorTaskInner {
			binary_version: "0.0.0-test".into(),
			tamanu: Mutex::new(tamanu),
			tamanu_source,
			pg_version_cache: Mutex::new(None),
			latest: Mutex::new(None),
			check_severities: Mutex::new(None),
			backup_dispatch: None,
		}
	}

	#[tokio::test]
	async fn discovery_replaces_the_previous_tamanu_context() {
		// The upgrade case: the daemon started on 2.54.0 and Tamanu has since been
		// upgraded in place. The sweep must run against the version now on disk,
		// and the new context must stick for subsequent sweeps too.
		let inner = inner(Some(sweep_tamanu("2.54.0")), TamanuSource::Fixed);
		let resolved = inner
			.apply_discovery(Ok(Some(sweep_tamanu("2.55.0"))))
			.await
			.expect("a context");
		assert_eq!(resolved.version, Version::parse("2.55.0").unwrap());
		assert_eq!(
			inner.tamanu.lock().await.as_ref().unwrap().version,
			Version::parse("2.55.0").unwrap()
		);
	}

	#[tokio::test]
	async fn discovery_failure_keeps_the_last_known_context() {
		// Discovery can fail transiently (an unreadable root, a config that won't
		// parse mid-write). Falling back to `None` would skip every Tamanu check;
		// the last known install is the better answer.
		let inner = inner(Some(sweep_tamanu("2.54.0")), TamanuSource::Fixed);
		let resolved = inner
			.apply_discovery(Err(miette!("no tamanu discovered")))
			.await
			.expect("the last known context");
		assert_eq!(resolved.version, Version::parse("2.54.0").unwrap());
	}

	#[tokio::test]
	async fn discovery_clears_the_context_when_tamanu_is_gone() {
		// A successful discovery that finds nothing is a fact, not a failure:
		// Tamanu is no longer on this host, so stop reporting the install.
		let inner = inner(Some(sweep_tamanu("2.54.0")), TamanuSource::Fixed);
		assert!(inner.apply_discovery(Ok(None)).await.is_none());
		assert!(inner.tamanu.lock().await.is_none());
	}

	#[tokio::test]
	async fn fixed_source_reuses_the_context_it_was_given() {
		// Builds with no Tamanu integration wired up have nothing to discover, so
		// `resolve_tamanu` must not go looking for an install.
		let inner = inner(Some(sweep_tamanu("2.54.0")), TamanuSource::Fixed);
		let resolved = inner.resolve_tamanu().await.expect("a context");
		assert_eq!(resolved.version, Version::parse("2.54.0").unwrap());
	}

	#[test]
	fn cap_check_applies_ceiling_when_present() {
		let mut severities = HashMap::new();
		severities.insert("disk_free".to_string(), CheckSeverity::Warn);
		let check = Check::fail("disk_free", "1% free", "out of space");
		let capped = cap_check(check, Some(&severities));
		match capped.status {
			CheckStatus::Warning(r) => assert_eq!(r, "out of space"),
			other => panic!("expected Warning, got {other:?}"),
		}
	}

	#[test]
	fn cap_check_absent_check_defaults_to_warn() {
		// No entry for this check: canopy's default ceiling is warn, so a fail
		// streams as a warning.
		let check = Check::fail("brand_new", "bad", "reason");
		let capped = cap_check(check, Some(&HashMap::new()));
		assert!(matches!(capped.status, CheckStatus::Warning(_)));
	}

	#[test]
	fn cap_check_no_mapping_is_a_noop() {
		let check = Check::fail("disk_free", "1% free", "out of space");
		let capped = cap_check(check, None);
		assert!(matches!(capped.status, CheckStatus::Fail(_)));
	}

	#[test]
	fn census_counts_each_status() {
		let results = vec![
			(Check::pass("a", ""), true),
			(Check::pass("b", ""), true),
			(Check::warning("c", "", "w"), true),
			(Check::fail("d", "", "f"), true),
			(Check::skip("e", "", "s"), true),
			(Check::broken("g", "", "b"), true),
		];
		let c = census(&results);
		assert_eq!(c.passing, 2);
		assert_eq!(c.warning, 1);
		assert_eq!(c.failing, 1);
		assert_eq!(c.skipped, 1);
		assert_eq!(c.broken, 1);
		assert_eq!(c.total(), 6);
		// active = ran (everything but skipped)
		assert_eq!(c.active(), 5);
	}

	#[test]
	fn census_reflects_severity_capping() {
		// A fail capped to a warn ceiling must count as warning, not failing —
		// the census tracks what operators see after capping.
		let mut sweep = doctor::SweepResult {
			server_id: None,
			results: vec![(Check::fail("disk_free", "1% free", "out of space"), true)],
			overall: doctor::check::OverallResult::Failing,
			payload: json!({}),
			pg_version: None,
		};
		let mut severities = HashMap::new();
		severities.insert("disk_free".to_string(), CheckSeverity::Warn);
		sweep.apply_severities(&severities);

		let c = census(&sweep.results);
		assert_eq!(c.failing, 0);
		assert_eq!(c.warning, 1);
	}
}
