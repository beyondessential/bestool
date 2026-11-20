---
id: CHK-LCF
---

# Legacy config healthcheck

The `legacy_config` healthcheck grades whether a Tamanu server still relies on a value from its own config files.
It is one of the doctor's healthchecks; see [CHK](healthchecks.md) for the framework it runs in and for how a check reports instances.

> [!NOTE]
> Tamanu is moving every value out of its config files and into settings, local facts and secrets, or environment variables, and will stop reading the files.
> This check finds the servers that would lose a value when it does, while there is still time to prepare them.

## Applicability

The check runs for central and facility servers.

Only the deployment's own config files are read: `production.json5` and `local.json5`.
The defaults Tamanu ships are never read, since a server cannot rely on what it does not set.
Only files on the server's own disk are read, never ones inside a container image.
A server with neither file reports no instances and passes.
A deployment known only through its database, with no install files on the machine, is skipped.
A value set to null counts as unset.

## Values with no home but the environment

These config values have no home an upgrade can write, only an environment variable:

| Config value | Environment variable |
| --- | --- |
| `proxy.trusted` | `PROXY_TRUSTED` |
| `disk.diskPath` | `DISK_PATH` |
| `auth.saltRounds` | `SALT_ROUNDS` |
| `db.migrateOnStartup` | `MIGRATE_ON_STARTUP` |
| `db.skipVersionCompatibilityCheck` | `DB_SKIP_VERSION_COMPATIBILITY_CHECK` |
| `log.consoleLevel` | `LOG_CONSOLE_LEVEL` |
| `log.timeless` | `LOG_TIMELESS` |
| `log.path` | `LOG_PATH` |

Each one set by config is a warning at any server version, naming the environment variable to set.

## Values an upgrade moves into the database

These config values are moved into the database by an upgrade step, first shipped in the release given:

| Config value | Server | Moved into | From |
| --- | --- | --- | --- |
| `sync.host` | facility | local fact `syncHost` | 2.61 |
| `sync.email` | facility | local fact `syncEmail` | 2.61 |
| `sync.password` | facility | local secret `syncPassword` | 2.61 |
| `crypto.settingsPsk` | central | local secret `settingsPsk` | 2.62 |
| `crypto.settingsPsk` | facility | local secret `settingsPsk`, pulled from central | 2.62 |
| `integrations.dhis2.password` | central | setting `integrations.dhis2.password` | 2.62 |
| `telegramBot.apiToken` | central | setting `integrations.telegram.apiToken` | 2.62 |
| `telegramBot.webhook.secret` | central | setting `integrations.telegram.webhook.secret` | 2.62 |
| `mailgun.apiKey` | central | setting `mail.mailgun.apiKey` | 2.62 |
| `sync.numberConcurrentPullSnapshots` | central | setting `sync.numberConcurrentPullSnapshots` | 2.70 |
| `sync.schedule`, `sync.jitterTime`, `sync.timeout`, `sync.backoff`, `sync.dynamicLimiter`, `sync.assertIfPulledRecordsUpdatedAfterPushSnapshot` | facility | facility setting of the same path, by way of central | 2.70 |

A value set by config on the server it applies to is one instance, graded:

- passed when the server is older than the release, since its upgrade will move the value.
- passed when the database already holds the value, the config value being unused.
- passed when the value only arrives from central and the server has not yet completed a sync.
- warning when the server has run the release and the database still lacks the value.
- skipped when the server's version is unknown, or its database cannot be reached.

A setting counts as held when any setting row sits at its path or beneath it, whatever its scope.

## Config moved into settings

Upgrade steps record a local fact once they have moved a server's config values into settings:

| Fact | Server | From |
| --- | --- | --- |
| `centralConfigMigratedToSettings` | central | 2.62 |
| `facilityConfigMigratedToSettings` | facility | 2.62 |
| `centralLateConfigMigratedToSettings` | central | 2.70 |
| `facilityLateConfigMigratedToSettings` | facility | 2.70 |

On a server that has a config file, each fact for its role is one instance, graded passed before the release, passed when the fact is set, warning when the server has run the release and the fact is unset, and skipped when the server's version is unknown or its database cannot be reached.

> [!NOTE]
> A facility step leaves its fact unset while a facility it serves has not yet synced down, and retries on the next upgrade.
