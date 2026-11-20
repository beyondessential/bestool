//! Config file values a Tamanu server still relies on, ahead of Tamanu reading
//! no config file at all.
//!
//! Each value either has no home but an environment variable, so an upgrade
//! cannot move it, or is moved into the database by an upgrade step. The first
//! warns whenever the deployment's own config sets it. The second warns only once
//! the server has run the release carrying its migration and the database still
//! lacks it, which means the migration left it behind; an older server passes,
//! since its upgrade will move the value.
//!
//! Only the deployment's own config files are read, never the defaults Tamanu
//! ships, and a server with none passes.
//!
//! spec: CHK-LCF

use std::collections::BTreeSet;

use node_semver::Version;
use serde_json::Value;
use tracing::warn;

use bestool_postgres::pool::PgConnection;

use super::{TamanuCx, is_unknown_version};
use crate::check::{Check, CheckStatus, Instance};
use crate::subject::ApplicationKind;

const NAME: &str = "legacy_config";

/// A config value whose only other home is an environment variable.
struct EnvOnly {
	path: &'static str,
	env: &'static str,
}

const ENV_ONLY: &[EnvOnly] = &[
	EnvOnly {
		path: "proxy.trusted",
		env: "PROXY_TRUSTED",
	},
	EnvOnly {
		path: "disk.diskPath",
		env: "DISK_PATH",
	},
	EnvOnly {
		path: "auth.saltRounds",
		env: "SALT_ROUNDS",
	},
	EnvOnly {
		path: "db.migrateOnStartup",
		env: "MIGRATE_ON_STARTUP",
	},
	EnvOnly {
		path: "db.skipVersionCompatibilityCheck",
		env: "DB_SKIP_VERSION_COMPATIBILITY_CHECK",
	},
	EnvOnly {
		path: "log.consoleLevel",
		env: "LOG_CONSOLE_LEVEL",
	},
	EnvOnly {
		path: "log.timeless",
		env: "LOG_TIMELESS",
	},
	EnvOnly {
		path: "log.path",
		env: "LOG_PATH",
	},
];

/// Where an upgrade step puts a config value.
#[derive(Clone, Copy)]
enum Home {
	Fact(&'static str),
	Secret(&'static str),
	Setting(&'static str),
}

/// A config value an upgrade step moves into the database.
struct Migrated {
	path: &'static str,
	home: Home,
	/// The first release carrying the step.
	since: (u64, u64),
	role: Role,
	/// The value only arrives once the server has synced with central.
	after_first_sync: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
	Central,
	Facility,
}

const MIGRATED: &[Migrated] = &[
	Migrated {
		path: "sync.host",
		home: Home::Fact("syncHost"),
		since: (2, 61),
		role: Role::Facility,
		after_first_sync: false,
	},
	Migrated {
		path: "sync.email",
		home: Home::Fact("syncEmail"),
		since: (2, 61),
		role: Role::Facility,
		after_first_sync: false,
	},
	Migrated {
		path: "sync.password",
		home: Home::Secret("syncPassword"),
		since: (2, 61),
		role: Role::Facility,
		after_first_sync: false,
	},
	Migrated {
		path: "crypto.settingsPsk",
		home: Home::Secret("settingsPsk"),
		since: (2, 62),
		role: Role::Central,
		after_first_sync: false,
	},
	Migrated {
		path: "crypto.settingsPsk",
		home: Home::Secret("settingsPsk"),
		since: (2, 62),
		role: Role::Facility,
		after_first_sync: true,
	},
	Migrated {
		path: "integrations.dhis2.password",
		home: Home::Setting("integrations.dhis2.password"),
		since: (2, 62),
		role: Role::Central,
		after_first_sync: false,
	},
	Migrated {
		path: "telegramBot.apiToken",
		home: Home::Setting("integrations.telegram.apiToken"),
		since: (2, 62),
		role: Role::Central,
		after_first_sync: false,
	},
	Migrated {
		path: "telegramBot.webhook.secret",
		home: Home::Setting("integrations.telegram.webhook.secret"),
		since: (2, 62),
		role: Role::Central,
		after_first_sync: false,
	},
	Migrated {
		path: "mailgun.apiKey",
		home: Home::Setting("mail.mailgun.apiKey"),
		since: (2, 62),
		role: Role::Central,
		after_first_sync: false,
	},
	Migrated {
		path: "sync.numberConcurrentPullSnapshots",
		home: Home::Setting("sync.numberConcurrentPullSnapshots"),
		since: (2, 70),
		role: Role::Central,
		after_first_sync: false,
	},
	late_facility_sync("sync.schedule"),
	late_facility_sync("sync.jitterTime"),
	late_facility_sync("sync.timeout"),
	late_facility_sync("sync.backoff"),
	late_facility_sync("sync.dynamicLimiter"),
	late_facility_sync("sync.assertIfPulledRecordsUpdatedAfterPushSnapshot"),
];

/// Facility sync timing reaches the facility's settings by way of central, so
/// only after a sync has carried it there and back.
const fn late_facility_sync(path: &'static str) -> Migrated {
	Migrated {
		path,
		home: Home::Setting(path),
		since: (2, 70),
		role: Role::Facility,
		after_first_sync: true,
	}
}

/// A fact an upgrade step sets once it has moved the config values into
/// settings, for the server role it runs on.
struct SettingsMigration {
	fact: &'static str,
	since: (u64, u64),
	role: Role,
}

const SETTINGS_MIGRATIONS: &[SettingsMigration] = &[
	SettingsMigration {
		fact: "centralConfigMigratedToSettings",
		since: (2, 62),
		role: Role::Central,
	},
	SettingsMigration {
		fact: "facilityConfigMigratedToSettings",
		since: (2, 62),
		role: Role::Facility,
	},
	SettingsMigration {
		fact: "centralLateConfigMigratedToSettings",
		since: (2, 70),
		role: Role::Central,
	},
	SettingsMigration {
		fact: "facilityLateConfigMigratedToSettings",
		since: (2, 70),
		role: Role::Facility,
	},
];

const LAST_SYNC_FACT: &str = "lastSuccessfulSyncPull";

/// What the database holds, as far as this check asks.
#[derive(Debug, Default)]
struct Recorded {
	facts: BTreeSet<String>,
	secrets: BTreeSet<String>,
	settings: BTreeSet<String>,
}

impl Recorded {
	fn has(&self, home: Home) -> bool {
		match home {
			Home::Fact(key) => self.facts.contains(key),
			Home::Secret(key) => self.secrets.contains(key),
			Home::Setting(key) => self.settings.contains(key),
		}
	}
}

fn lookup<'a>(config: &'a Value, path: &str) -> Option<&'a Value> {
	path.split('.')
		.try_fold(config, |node, segment| node.get(segment))
		.filter(|value| !value.is_null())
}

fn applies(role: Role, kind: ApplicationKind) -> bool {
	match role {
		Role::Central => kind == ApplicationKind::TamanuCentral,
		Role::Facility => kind == ApplicationKind::TamanuFacility,
	}
}

fn reached(version: &Version, (major, minor): (u64, u64)) -> bool {
	(version.major, version.minor) >= (major, minor)
}

fn evaluate(
	config: &Value,
	kind: ApplicationKind,
	version: Option<&Version>,
	recorded: Option<&Recorded>,
) -> Check {
	let mut instances = Vec::new();

	for key in ENV_ONLY {
		if lookup(config, key.path).is_some() {
			instances.push(
				Instance::warning(
					key.path,
					format!(
						"set by config, which Tamanu stops reading: set the {} environment variable and remove it from config",
						key.env
					),
				)
				.with_detail("env", key.env),
			);
		}
	}

	let synced = recorded.is_some_and(|recorded| recorded.facts.contains(LAST_SYNC_FACT));
	for key in MIGRATED.iter().filter(|key| applies(key.role, kind)) {
		if lookup(config, key.path).is_none() {
			continue;
		}
		let Some(recorded) = recorded else {
			instances.push(Instance::skip(key.path, "no DB connection"));
			continue;
		};
		let instance = match version {
			None => Instance::skip(key.path, "server version unknown"),
			Some(version) if !reached(version, key.since) => Instance::pass(key.path).with_detail(
				"note",
				format!(
					"moved into the database on upgrade to {}.{}",
					key.since.0, key.since.1
				),
			),
			Some(_) if recorded.has(key.home) => Instance::pass(key.path).with_detail(
				"note",
				"already in the database; the config value is unused",
			),
			Some(_) if key.after_first_sync && !synced => Instance::pass(key.path).with_detail(
				"note",
				"arrives from central with the first successful sync",
			),
			Some(_) => Instance::warning(
				key.path,
				"still read from config: its upgrade migration left the database without it",
			),
		};
		instances.push(instance);
	}

	let has_config = config.as_object().is_some_and(|object| !object.is_empty());
	for step in SETTINGS_MIGRATIONS
		.iter()
		.filter(|step| has_config && applies(step.role, kind))
	{
		let Some(recorded) = recorded else {
			instances.push(Instance::skip(step.fact, "no DB connection"));
			continue;
		};
		let instance = match version {
			None => Instance::skip(step.fact, "server version unknown"),
			Some(version) if !reached(version, step.since) => Instance::pass(step.fact)
				.with_detail(
					"note",
					format!("runs on upgrade to {}.{}", step.since.0, step.since.1),
				),
			Some(_) if recorded.facts.contains(step.fact) => Instance::pass(step.fact),
			Some(_) => Instance::warning(
				step.fact,
				"config values not yet moved into settings: the upgrade step retries on the next upgrade",
			),
		};
		instances.push(instance);
	}

	let relying = instances
		.iter()
		.filter(|instance| matches!(instance.status, CheckStatus::Warning(_)))
		.count();
	let summary = if instances.is_empty() {
		"no legacy config values".to_string()
	} else if relying == 0 {
		format!("{} legacy config value(s), none relied on", instances.len())
	} else {
		format!("{relying} config value(s) still relied on")
	};

	Check::instanced(NAME, summary, instances)
}

/// The deployment's own config, or `None` where it is known only through its
/// database and there are no files to read.
fn deployment_config(ctx: &TamanuCx) -> Option<miette::Result<Value>> {
	let root = ctx.install_root.as_deref()?;
	let package = ctx.server_kind().package_name();
	let mut config = Value::Object(Default::default());
	for file in ["production.json5", "local.json5"] {
		match bestool_tamanu::config::package_config_on_disk(root, Some(package), file) {
			Some(Ok(layer)) => config = bestool_tamanu::config::merge_json(config, layer),
			Some(Err(err)) => return Some(Err(err)),
			None => {}
		}
	}
	Some(Ok(config))
}

const FACTS_SQL: &str = "SELECT key FROM local_system_facts WHERE key = ANY($1)";
const SECRETS_SQL: &str = "SELECT key FROM local_system_secrets WHERE key = ANY($1)";
const SECRETS_TABLE_SQL: &str = "SELECT to_regclass('local_system_secrets') IS NOT NULL";
// An object setting is stored as one row per leaf, so a path counts as set
// when any row sits at or under it.
const SETTINGS_SQL: &str = "SELECT path FROM unnest($1::text[]) AS path WHERE EXISTS ( \
		SELECT 1 FROM settings s WHERE s.deleted_at IS NULL \
			AND (s.key = path OR s.key LIKE path || '.%'))";

pub async fn run(ctx: TamanuCx) -> Check {
	let config = match deployment_config(&ctx) {
		None => {
			return Check::skip(
				NAME,
				"no install files",
				"this deployment is known only through its database, so there is no config to read",
			);
		}
		Some(Ok(config)) => config,
		Some(Err(err)) => {
			// The parse error quotes the offending line, which can hold a credential.
			warn!(?err, "legacy_config: could not read the config files");
			let file = err.to_string();
			return Check::broken(
				NAME,
				"could not read the config files",
				format!("could not read {file}; see the alertd log on this host"),
			);
		}
	};

	let version = (!is_unknown_version(&ctx.version)).then_some(&ctx.version);
	let has_config = config.as_object().is_some_and(|object| !object.is_empty());
	if !has_config {
		return evaluate(&config, ctx.app.kind, version, None);
	}

	let Some(client) = ctx.db().await else {
		return evaluate(&config, ctx.app.kind, version, None);
	};
	match read_recorded(&client).await {
		Ok(recorded) => evaluate(&config, ctx.app.kind, version, Some(&recorded)),
		Err(err) => {
			warn!(?err, "legacy_config: could not read the database");
			evaluate(&config, ctx.app.kind, version, None)
		}
	}
}

fn wanted(pick: fn(Home) -> Option<&'static str>) -> Vec<String> {
	MIGRATED
		.iter()
		.filter_map(|key| pick(key.home))
		.map(str::to_string)
		.collect()
}

async fn read_recorded(client: &PgConnection) -> Result<Recorded, tokio_postgres::Error> {
	let mut recorded = Recorded::default();

	let mut facts = wanted(|home| match home {
		Home::Fact(key) => Some(key),
		_ => None,
	});
	facts.push(LAST_SYNC_FACT.to_string());
	facts.extend(SETTINGS_MIGRATIONS.iter().map(|step| step.fact.to_string()));
	recorded.facts = client
		.query(FACTS_SQL, &[&facts])
		.await?
		.iter()
		.map(|row| row.get(0))
		.collect();

	let has_secrets_table: bool = client.query_one(SECRETS_TABLE_SQL, &[]).await?.get(0);
	if has_secrets_table {
		let secrets = wanted(|home| match home {
			Home::Secret(key) => Some(key),
			_ => None,
		});
		recorded.secrets = client
			.query(SECRETS_SQL, &[&secrets])
			.await?
			.iter()
			.map(|row| row.get(0))
			.collect();
	}

	let settings = wanted(|home| match home {
		Home::Setting(key) => Some(key),
		_ => None,
	});
	recorded.settings = client
		.query(SETTINGS_SQL, &[&settings])
		.await?
		.iter()
		.map(|row| row.get(0))
		.collect();

	Ok(recorded)
}

#[cfg(test)]
mod tests {
	use serde_json::json;

	use super::*;

	fn warns(status: &CheckStatus) -> bool {
		matches!(status, CheckStatus::Warning(_))
	}

	fn v(s: &str) -> Version {
		Version::parse(s).unwrap()
	}

	fn status_of<'a>(check: &'a Check, key: &str) -> &'a CheckStatus {
		&check
			.instances
			.as_ref()
			.unwrap()
			.iter()
			.find(|instance| instance.key == key)
			.unwrap_or_else(|| panic!("no instance {key}"))
			.status
	}

	#[test]
	fn no_config_passes_with_no_instances() {
		let check = evaluate(
			&json!({}),
			ApplicationKind::TamanuFacility,
			Some(&v("2.69.0")),
			Some(&Recorded::default()),
		);
		assert!(matches!(check.status, CheckStatus::Pass));
		assert!(check.instances.unwrap().is_empty());
	}

	#[test]
	fn env_only_keys_warn_at_any_version() {
		let config = json!({ "proxy": { "trusted": "loopback" }, "disk": { "diskPath": "/var" } });
		for version in ["2.55.0", "2.70.0"] {
			let check = evaluate(
				&config,
				ApplicationKind::TamanuCentral,
				Some(&v(version)),
				Some(&Recorded::default()),
			);
			assert!(warns(status_of(&check, "proxy.trusted")));
			assert!(warns(status_of(&check, "disk.diskPath")));
		}
	}

	#[test]
	fn migrated_key_passes_before_its_release() {
		let config = json!({ "sync": { "password": "pw" } });
		let check = evaluate(
			&config,
			ApplicationKind::TamanuFacility,
			Some(&v("2.60.3")),
			Some(&Recorded::default()),
		);
		assert!(matches!(
			status_of(&check, "sync.password"),
			CheckStatus::Pass
		));
	}

	#[test]
	fn migrated_key_warns_when_left_behind() {
		let config = json!({ "sync": { "password": "pw", "email": "f@x" } });
		let recorded = Recorded {
			facts: ["syncEmail".to_string()].into(),
			..Default::default()
		};
		let check = evaluate(
			&config,
			ApplicationKind::TamanuFacility,
			Some(&v("2.63.0")),
			Some(&recorded),
		);
		assert!(warns(status_of(&check, "sync.password")));
		assert!(matches!(status_of(&check, "sync.email"), CheckStatus::Pass));
	}

	#[test]
	fn facility_psk_waits_for_the_first_sync() {
		let config = json!({ "crypto": { "settingsPsk": "abc" } });
		let unsynced = evaluate(
			&config,
			ApplicationKind::TamanuFacility,
			Some(&v("2.65.0")),
			Some(&Recorded::default()),
		);
		assert!(matches!(
			status_of(&unsynced, "crypto.settingsPsk"),
			CheckStatus::Pass
		));

		let synced = evaluate(
			&config,
			ApplicationKind::TamanuFacility,
			Some(&v("2.65.0")),
			Some(&Recorded {
				facts: [LAST_SYNC_FACT.to_string()].into(),
				..Default::default()
			}),
		);
		assert!(warns(status_of(&synced, "crypto.settingsPsk")));
	}

	#[test]
	fn role_scoped_keys_ignore_the_other_role() {
		let config = json!({ "sync": { "host": "https://central" }, "mailgun": { "apiKey": "k" } });
		let check = evaluate(
			&config,
			ApplicationKind::TamanuCentral,
			Some(&v("2.65.0")),
			Some(&Recorded::default()),
		);
		let keys: Vec<_> = check
			.instances
			.unwrap()
			.iter()
			.map(|instance| instance.key.clone())
			.collect();
		assert!(keys.contains(&"mailgun.apiKey".to_string()));
		assert!(!keys.contains(&"sync.host".to_string()));
		assert!(!keys.contains(&"facilityConfigMigratedToSettings".to_string()));
	}

	#[test]
	fn unknown_version_skips_migrated_keys() {
		let config = json!({ "mailgun": { "apiKey": "k" } });
		let check = evaluate(
			&config,
			ApplicationKind::TamanuCentral,
			None,
			Some(&Recorded::default()),
		);
		assert!(status_of(&check, "mailgun.apiKey").is_skip());
	}

	#[test]
	fn null_config_value_counts_as_unset() {
		let config = json!({ "proxy": { "trusted": null } });
		let check = evaluate(
			&config,
			ApplicationKind::TamanuCentral,
			Some(&v("2.70.0")),
			Some(&Recorded::default()),
		);
		assert!(
			!check
				.instances
				.unwrap()
				.iter()
				.any(|instance| instance.key == "proxy.trusted")
		);
	}

	#[test]
	fn settings_migration_warns_once_due_and_unfinished() {
		let config = json!({ "db": { "name": "app" } });
		let due = evaluate(
			&config,
			ApplicationKind::TamanuFacility,
			Some(&v("2.70.1")),
			Some(&Recorded {
				facts: ["facilityConfigMigratedToSettings".to_string()].into(),
				..Default::default()
			}),
		);
		assert!(matches!(
			status_of(&due, "facilityConfigMigratedToSettings"),
			CheckStatus::Pass
		));
		assert!(warns(status_of(
			&due,
			"facilityLateConfigMigratedToSettings"
		)));

		let older = evaluate(
			&config,
			ApplicationKind::TamanuFacility,
			Some(&v("2.66.0")),
			Some(&Recorded::default()),
		);
		assert!(warns(status_of(&older, "facilityConfigMigratedToSettings")));
		assert!(matches!(
			status_of(&older, "facilityLateConfigMigratedToSettings"),
			CheckStatus::Pass
		));
	}

	#[test]
	fn late_facility_sync_key_counts_leaf_rows_and_waits_for_sync() {
		let config = json!({ "sync": { "backoff": { "maxAttempts": 20 } } });
		let unsynced = evaluate(
			&config,
			ApplicationKind::TamanuFacility,
			Some(&v("2.70.0")),
			Some(&Recorded::default()),
		);
		assert!(matches!(
			status_of(&unsynced, "sync.backoff"),
			CheckStatus::Pass
		));

		let left_behind = evaluate(
			&config,
			ApplicationKind::TamanuFacility,
			Some(&v("2.70.0")),
			Some(&Recorded {
				facts: [LAST_SYNC_FACT.to_string()].into(),
				..Default::default()
			}),
		);
		assert!(warns(status_of(&left_behind, "sync.backoff")));
	}

	#[test]
	fn parse_error_names_the_file_without_its_contents() {
		let root = tempfile::tempdir().unwrap();
		std::fs::write(
			root.path().join("local.json5"),
			"{ db: { password: \"hunter2\" ",
		)
		.unwrap();
		let err = bestool_tamanu::config::package_config_on_disk(
			root.path(),
			Some("central-server"),
			"local.json5",
		)
		.unwrap()
		.unwrap_err();
		let reported = err.to_string();
		assert!(reported.ends_with("local.json5"));
		assert!(!reported.contains("hunter2"));
	}
}
