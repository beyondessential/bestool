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
The defaults Tamanu ships are never graded, since a server cannot rely on what it does not set.
They are read only to compare a deployment's value against.
Only files on the server's own disk are read, never ones inside a container image.
A server with neither file reports no instances and passes.
A deployment known only through its database, with no install files on the machine, is skipped.
A value set to null counts as unset.

## Values with no home but the environment

These config values have no home an upgrade can write, only an environment variable:

| Config value | Environment variable | Read from |
| --- | --- | --- |
| `proxy.trusted` | `PROXY_TRUSTED` | 2.62 |
| `disk.diskPath` | `DISK_PATH` | 2.62 |
| `auth.saltRounds` | `SALT_ROUNDS` | 2.62 |
| `db.migrateOnStartup` | `MIGRATE_ON_STARTUP` | 2.62 |
| `db.skipVersionCompatibilityCheck` | `DB_SKIP_VERSION_COMPATIBILITY_CHECK` | 2.62 |
| `log.consoleLevel` | `LOG_CONSOLE_LEVEL` | 2.62 |
| `log.timeless` | `LOG_TIMELESS` | 2.62 |
| `log.path` | `LOG_PATH` | 2.62 |
| `primaryTimeZone`, `countryTimeZone` | `TZ` | 2.62 |
| `auth.secret` | `AUTH_SECRET` | 2.62 |
| `auth.refreshToken.secret` | `AUTH_REFRESH_TOKEN_SECRET` | 2.62 |
| `canonicalHostName` | `CANONICAL_HOST_NAME` | 2.62 |
| `s3.accessKeyId` | `AWS_ACCESS_KEY_ID` | 2.62 |
| `s3.secretAccessKey` | `AWS_SECRET_ACCESS_KEY` | 2.62 |
| `sync.enabled` | `SYNC_ENABLED` | 2.70 |
| `sync.syncApiConnection.host` | `SYNC_API_HOST` | 2.70 |
| `sync.syncApiConnection.port` | `SYNC_API_PORT` | 2.70 |
| `integrations.fhir.worker.enabled` | `TAMANU_FHIR_WORKER_ENABLED` | 2.70 |

Each one set by config is a warning on a server at or past the release that reads its environment variable, or of unknown version, naming the variable to set.
On an older server, which reads only config, it is passed.

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
| `mail.transport.auth.pass` | central | setting `mail.transportPassword` | 2.62 |
| `sync.numberConcurrentPullSnapshots` | central | setting `sync.numberConcurrentPullSnapshots` | 2.70 |
| `sync.schedule`, `sync.jitterTime`, `sync.timeout`, `sync.backoff`, `sync.dynamicLimiter`, `sync.assertIfPulledRecordsUpdatedAfterPushSnapshot` | facility | facility setting of the same path, by way of central | 2.70 |

A value set by config on the server it applies to is one instance, graded:

- passed when the server is older than the release, since its upgrade will move the value.
- passed when the database already holds the value, the config value being unused.
- passed when the value only arrives from central and the server has not completed a sync since the migration that sends it: the local fact `lastSuccessfulSyncPull` is absent, or was last written before that migration's fact.
- passed when the value is headed for a setting and every part of it equals the default Tamanu ships, since the migration moves nothing that matches the default.
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
