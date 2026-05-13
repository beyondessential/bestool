use std::{collections::HashMap, path::PathBuf, sync::Arc};

use bestool_alertd::{
	InternalContext,
	scheduler::Scheduler,
	state_file::{PersistedAlertState, PersistedState},
};
use tempfile::TempDir;

async fn make_ctx() -> Arc<InternalContext> {
	let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
	let pg_pool = bestool_postgres::pool::create_pool(&db_url, "bestool-alertd-test")
		.await
		.unwrap();
	Arc::new(InternalContext {
		pg_pool,
		http_client: reqwest::Client::new(),
		canopy_client: None,
	})
}

fn write_alert(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
	let path = dir.join(name);
	std::fs::write(&path, body).unwrap();
	path
}

#[tokio::test]
async fn hydration_seeds_triggered_at_for_matched_alert() {
	let tmp = TempDir::new().unwrap();
	let alert_path = write_alert(
		tmp.path(),
		"disk.yml",
		"sql: \"SELECT 1\"\nsend:\n  - id: ops\n    subject: x\n    template: y\n",
	);

	let triggered_at: jiff::Timestamp = "2026-05-13T14:55:00Z".parse().unwrap();
	let mut alerts = HashMap::new();
	alerts.insert(
		alert_path.clone(),
		PersistedAlertState {
			triggered_at: Some(triggered_at),
			..Default::default()
		},
	);
	let persisted = PersistedState {
		saved_at: None,
		alerts,
	};

	let ctx = make_ctx().await;
	let scheduler = Scheduler::new(
		vec![tmp.path().to_string_lossy().into_owned()],
		ctx,
		None,
		true, // dry_run keeps task wakeups inert for the test
	);

	scheduler.set_pending_hydration(persisted).await;
	scheduler.load_and_schedule_alerts().await.unwrap();

	let states = scheduler.get_alert_states().await;
	let state = states
		.get(&alert_path)
		.expect("alert should be loaded under its canonical path");
	assert_eq!(
		state.triggered_at,
		Some(triggered_at),
		"triggered_at should be hydrated from the persisted state"
	);
}

#[tokio::test]
async fn hydration_ignores_entries_for_unknown_alerts() {
	let tmp = TempDir::new().unwrap();
	let alert_path = write_alert(
		tmp.path(),
		"present.yml",
		"sql: \"SELECT 1\"\nsend:\n  - id: ops\n    subject: x\n    template: y\n",
	);

	let mut alerts = HashMap::new();
	// Entry for an alert file that doesn't exist on disk.
	alerts.insert(
		PathBuf::from("/no/such/path.yml"),
		PersistedAlertState {
			triggered_at: Some("2026-05-13T14:55:00Z".parse().unwrap()),
			..Default::default()
		},
	);
	let persisted = PersistedState {
		saved_at: None,
		alerts,
	};

	let ctx = make_ctx().await;
	let scheduler = Scheduler::new(
		vec![tmp.path().to_string_lossy().into_owned()],
		ctx,
		None,
		true,
	);

	scheduler.set_pending_hydration(persisted).await;
	scheduler.load_and_schedule_alerts().await.unwrap();

	let states = scheduler.get_alert_states().await;
	let state = states.get(&alert_path).unwrap();
	assert!(
		state.triggered_at.is_none(),
		"orphan hydration entries must not seed unrelated alerts"
	);
}

#[tokio::test]
async fn snapshot_round_trips_through_persistence() {
	let tmp = TempDir::new().unwrap();
	let alert_path = write_alert(
		tmp.path(),
		"disk.yml",
		"sql: \"SELECT 1\"\nsend:\n  - id: ops\n    subject: x\n    template: y\n",
	);

	let triggered_at: jiff::Timestamp = "2026-05-13T14:55:00Z".parse().unwrap();
	let mut alerts = HashMap::new();
	alerts.insert(
		alert_path.clone(),
		PersistedAlertState {
			triggered_at: Some(triggered_at),
			last_output: Some("rows=...".into()),
			..Default::default()
		},
	);
	let persisted = PersistedState {
		saved_at: None,
		alerts,
	};

	let ctx = make_ctx().await;
	let scheduler = Scheduler::new(
		vec![tmp.path().to_string_lossy().into_owned()],
		ctx,
		None,
		true,
	);
	scheduler.set_pending_hydration(persisted).await;
	scheduler.load_and_schedule_alerts().await.unwrap();

	let snapshot = scheduler.snapshot_for_persistence().await;
	let entry = snapshot
		.alerts
		.get(&alert_path)
		.expect("snapshot should include the loaded alert");
	assert_eq!(entry.triggered_at, Some(triggered_at));
	assert_eq!(entry.last_output.as_deref(), Some("rows=..."));
}
