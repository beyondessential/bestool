# Plan: FHIR materialisation gap check

## Why

Every existing FHIR check in alertd measures the **queue machinery**, not the **outcome**.
`fhir_jobs` measures jobs waiting and the age of the oldest queued job, `fhir_job_errors` measures jobs that failed, `fhir_workers` measures worker liveness, and `fhir_service_requests_unresolved` measures the resolution state of rows that have *already* been materialised.

None of them can see an upstream record that was never materialised at all.

That matters because it is a real and silent failure mode.
If a materialisation job is never enqueued — trigger missed, `fhir.worker.resourceMaterialisationEnabled.<Resource>` turned off, or `fhir.jobs` truncated without the follow-up re-materialisation that the Tamanu support pack warns is required — then queue depth is zero, oldest-job age is zero, error count is zero, workers are alive, and unresolved count is zero.
All five checks read green while the record is invisible to any integration that consumes the FHIR API.

This was observed on a live central server during a support investigation into delayed lab results reaching an external LIS.
Five FHIR checks were clean for the entire observable window; the only anomaly was in the queue machinery, and no metric could confirm whether lab requests had actually been materialised.

Tamanu already computes exactly the number needed.
`FhirMissingResources.countQueue()` in `packages/shared/src/tasks/fhir/FhirMissingResources.js:49-75` counts materialisable upstream rows with no corresponding FHIR row, as a pure read.
But it runs once daily at 01:48 on central, is disabled by default on facility, and `ScheduledTask.runImmediatelyImplementation` (`packages/shared/src/tasks/ScheduledTask.js:65-76`) logs the result to a span attribute and then discards it.

Doing this in alertd ships now.
Doing it in Tamanu ships in two to three months on the deployments we want to measure.
This plan covers the alertd version, and records what the Tamanu version should later absorb.

## What to measure

Check name `fhir_materialisation`, producing two per-resource gauges:

- `gap` — the number of upstream rows with no FHIR row, per resource.
- `lag_seconds` — the age of the **oldest** such row, per resource.

The lag is the metric that carries the alerting.
Count alone is inherently noisy: there is always a transient nonzero count in the seconds between an upstream insert and its materialisation, so any count threshold either alerts constantly or is set so high it misses real gaps.
Age separates "five records, oldest three seconds old, working normally" from "five records, oldest six hours old, broken", and it needs no per-deployment calibration because it is an absolute duration.

Thresholds on `lag_seconds`, matching the shape of the existing unresolved check: warn past 15 minutes, fail past 60 minutes.
Report the worst resource in the summary and put the per-resource breakdown in details.

Grouping the stats as `gap` and `lag_seconds` under check name `fhir_materialisation` yields the metric names `bes_alertd_fhir_materialisation_gap` and `bes_alertd_fhir_materialisation_lag_seconds`.

## Discovering what to measure

The FHIR side is fully self-discovering from the live schema.

```sql
SELECT table_name
  FROM information_schema.columns
 WHERE table_schema = 'fhir' AND column_name = 'upstream_id'
 ORDER BY table_name;
```

This returns exactly the materialised resources and nothing else.
Verified against the checked-in schema in the Tamanu repo (`database/model/fhir/`): `upstream_id` appears on nine tables — `encounters`, `immunizations`, `medication_requests`, `non_fhir_medici_report`, `organizations`, `patients`, `practitioners`, `service_requests`, `specimens` — which is precisely the set of nine resources declaring `FHIR_INTERACTIONS.INTERNAL.MATERIALISE`.
`jobs` and `job_workers` do not have the column.
`DiagnosticReport`, `Observation` and `ImagingStudy` declare upstream models in code but are computed on read and have no tables at all, so discovery does not over-collect them.

Discovery is preferable to deriving table names from resource names.
Tamanu derives them as `snakeCase(pluralize(fhirName))` (`packages/database/src/models/fhir/Resource.ts:80`), but there is at least one override that breaks the rule — `MediciReport` uses `tableName: 'non_fhir_medici_report'`, singular and unprefixed (`packages/database/src/models/fhir/MediciReport.ts:100`) — and the upstream side contains an irregular plural (`Facility` → `facilities`).
Reading the table names from the schema sidesteps the inflection problem entirely.

## What cannot be discovered

The resource-to-upstream relationship is an arbitrary declaration in Tamanu and is not recoverable from the schema.
Nothing about the string `service_requests` implies it materialises from both `lab_requests` and `imaging_requests`, and there is no foreign key to follow because `upstream_id` is polymorphic across those two tables.

This is the only hardcoded data the check needs.
Nine entries, keyed by the discovered FHIR table name, sourced from the `UpstreamModels` assignments in `packages/database/src/models/fhir/`:

| FHIR table | upstream table(s) | upstream filter |
| --- | --- | --- |
| `service_requests` | `lab_requests`, `imaging_requests` | none |
| `patients` | `patients` | none |
| `practitioners` | `users` | none |
| `organizations` | `facilities` | none |
| `immunizations` | `administered_vaccines` | none |
| `medication_requests` | `pharmacy_order_prescriptions` | none |
| `specimens` | `lab_requests` | `specimen_attached = true` |
| `encounters` | `encounters` | `encounter_type <> 'surveyResponse'` |
| `non_fhir_medici_report` | `encounters` | `encounter_type <> 'surveyResponse'` |

The upstream filters come from `Resource.queryToFilterUpstream`, which returns `null` for all but three resources (`packages/database/src/models/fhir/Resource.ts:283-285`).
The three overrides are trivially expressible in SQL: `filterFromLabRequests` is `specimenAttached: true` (`packages/database/src/utils/fhir/Specimen/getQueryToFilterUpstream.ts:8-12`) and `filterFromEncounters` is `encounterType != ENCOUNTER_TYPES.SURVEY_RESPONSE`, whose value is the string `surveyResponse` (`packages/database/src/utils/fhir/Encounter/getQueryToFilterUpstream.ts:9-13`, `packages/constants/src/encounters.ts:8`).

Because all nine filters are expressible, the alertd check has **full coverage** — it is not a reduced-scope approximation of the Tamanu version.

### Fail loudly when the map goes stale

Cross-reference the discovered tables against the hardcoded map.
When the schema contains a `fhir.*` table with `upstream_id` that the map has no entry for, emit it rather than silently ignoring it — a `unmonitored` gauge, or a warning listing the unmapped resources.

Without this, a future Tamanu version that adds a materialised resource silently reduces coverage.
With it, the check reports that it has gone out of date, which is a very different risk profile for a stopgap.

## Preconditions readable from config

Two gates come from the install's config and should be applied before any query runs, using the typed reader in `crates/tamanu/src/config/structure.rs`.

**Gate on `fhir_worker_enabled()`.**
When `integrations.fhir.worker.enabled` is false the materialisation worker is not running at all, so every upstream row is legitimately unmaterialised and the check would report a total gap on every resource.
Skip the check entirely in that case rather than reporting a fault.
`fhir_workers.rs:63` and `fhir_jobs.rs:158` already apply exactly this gate and are the precedent to follow.

**Server kind** comes from `is_facility()` / `ApiServerKind`, as the existing FHIR checks already do.

Two further config values are worth reading, though neither gates the check:

- `primary_time_zone()` (`structure.rs:73-78`) already mirrors Tamanu's `getPrimaryTimeZone()`.
  This check does not need it, because the columns it reads are timezone-aware — see the query notes — but it exists, so don't reimplement it if a later change needs wall-clock reasoning.
- The `fhirMissingResources` schedule and enabled flag are in config (`packages/central-server/config/default.json5:231-234`).
  That task is what repairs gaps, nightly, so whether it is enabled changes how a gap should be read: with it on, a gap should clear overnight and one that persists across a night is a harder fault; with it off, gaps never self-heal.
  Worth surfacing as a check detail if the config struct is extended to expose it.

The config reader is a typed `serde` structure rather than a generic key-path lookup, so exposing any additional key is a small additive change to `structure.rs` — not a reason to reach for the database.

## Respecting per-resource enablement

Per-resource enablement, unlike the gates above, is **not available from config** and must come from the database.
`integrations.fhir` in the config file carries only `enabled` and `worker.enabled` (`packages/central-server/config/default.json5:343-352`); there is no materialisation-per-resource key there in any vintage.

It lives only as the setting `fhir.worker.resourceMaterialisationEnabled.<Resource>` (`packages/settings/src/schema/definitions/fhir.ts`), and **most resources default to `false`** — `Patient` defaults true, `Encounter`, `Immunization` and `MediciReport` default false.

A check that ignores this will report a 100% phantom gap for every disabled resource on every deployment.
This is the single most important correctness detail in the plan.

Settings live in the `settings` table with columns `key`, `value`, `facility_id`, `scope`, `deleted_at`, so the stored overrides are readable directly:

```sql
SELECT key, value
  FROM settings
 WHERE key LIKE 'fhir.worker.resourceMaterialisationEnabled.%'
   AND deleted_at IS NULL;
```

The complication is that an unset setting resolves to the schema `defaultValue`, which lives in Tamanu's code and not in the database.
Absence of a row therefore does not mean disabled.

Suggested resolution, in order of preference:

1. Use the settings row when one exists.
2. When no row exists, fall back to inferring enablement from observed data: a resource with at least one row in its FHIR table has been materialising, and one with zero rows has not.
   This is self-calibrating and needs no knowledge of Tamanu's defaults.
   Its failure mode is informative rather than noisy — a resource disabled *after* accumulating rows will start reporting a growing gap, which is arguably a legitimate warning that materialisation was switched off with data already present.
3. Avoid embedding Tamanu's per-resource defaults in alertd if possible; they are a drift risk for exactly the reason this whole check exists.

Note that facility settings for this key are merged server-wide — enabling a resource for one facility enables it for the whole server (`packages/settings/src/schema/facility.ts:461-465`) — so a per-facility reading is not needed.

## The query

Per resource, with the upstream filter substituted and a rolling window:

```sql
SELECT count(*)                                                          AS gap_count,
       coalesce(max(extract(epoch FROM now() - u.created_at)), 0)::bigint AS lag_seconds
  FROM <upstream_table> u
  LEFT JOIN fhir.<fhir_table> r ON r.upstream_id = u.id
 WHERE r.id IS NULL
   AND u.deleted_at IS NULL
   AND u.created_at > now() - interval '48 hours'
   AND <upstream_filter>;
```

Union the per-upstream results for resources with more than one upstream table (only `service_requests`) before aggregating, so the resource reports one gap and one lag across both.

Notes on the predicate:

- `now()` is correct and `LOCALTIMESTAMP` would be wrong.
  Tamanu stores clinical datetimes as naive strings in the deployment's primary timezone, which is a real trap for other queries, but `created_at`, `updated_at` and `deleted_at` are Sequelize audit columns typed `timestamp with time zone` (confirmed in `database/model/public/lab_requests.yml`).
  No timezone handling is needed for this check.
- `deleted_at IS NULL` excludes soft-deleted upstream rows so clinically cancelled records are not counted as gaps.
  Tamanu's own version inherits this from Sequelize's paranoid handling; confirm the behaviour matches before trusting absolute counts.
- Presence of the FHIR row is the test, not its resolution state.
  A materialised-but-unresolved row is already covered by `fhir_service_requests_unresolved` and must not also count as a gap here.
- The 48-hour window keeps the query cheap and bounds it to what is clinically live.
  Anything older is a backfill concern, not an incident.

## Cost and cadence

There is no index on `fhir.<resource>.upstream_id` — none is declared in `Resource.ts` and no migration defines one.
Postgres will therefore hash-join, scanning the whole FHIR table on each run.
That is cheap for `fhir.service_requests` and potentially expensive for `fhir.patients`.

`EXPLAIN ANALYZE` the query on a representative central database before choosing a cadence, and start at 15 minutes rather than 5.
Adding the index requires a Tamanu migration, so it belongs to the follow-up work below.

## Server kinds

The existing FHIR checks are central-only (`ctx.kind != ApiServerKind::Central` → skip).
Materialisation does also run on facility servers, and `FhirMissingResources` is present but disabled by default there.

Recommendation: implement central-first to match the existing pattern, but note that nothing in the query is central-specific — the schema discovery naturally no-ops where the `fhir` schema is absent, so extending to facility later costs nothing beyond deciding it is wanted.

## Implementation notes

Follow the shape of `crates/alertd/src/doctor/checks/fhir_service_requests_unresolved.rs`, which is the closest existing analogue: skip when not central, skip when `ctx.db` is `None`, `query_error_check` on query failure, `Stat::gauge(...).group(...).help(...)` for the metrics, `with_detail` for the per-resource breakdown.

Register in `crates/alertd/src/doctor/checks.rs` alongside the other `fhir_*` entries.

The check must not be required for the daemon to start, per the standing alertd rule that a database outage has to remain alertable.
The `ctx.db.as_ref()` skip path covers this.

Tests should follow the existing convention of `central_ctx()` / `facility_ctx()` from `test_support`, asserting the check runs and skips appropriately.
The mapping-vs-discovery cross-reference and the threshold tiering are both pure logic and should be unit-tested without a database.

## Open questions

- Enablement resolution: is the settings-row-then-infer-from-data approach above acceptable, or is embedding Tamanu's defaults preferable despite the drift risk?
  Config is not a third option — the per-resource key does not exist there, only the worker-level gate does.
- Window length: 48 hours is a guess balancing cost against catching a gap that opened yesterday evening. Should it be configurable?
- Should `lag_seconds` be reported for disabled resources as zero, or omitted entirely? Omitting keeps the metric honest; reporting zero keeps the series continuous across an enablement change.

## Follow-up in Tamanu, and retiring this

This check is a stopgap with a defined retirement.
The Tamanu-side work should:

- Break `countQueue()` out per resource instead of summing across all of them (`FhirMissingResources.js:70`).
- Add the lag alongside the count.
- Add a stale-not-missing predicate (`r.last_updated < u.updated_at`), which this check cannot do usefully and which closes the documented hole where truncating `fhir.jobs` drops outstanding refresh state and nothing verifies the follow-up re-materialisation happened.
- Add an index on `fhir.*.upstream_id` so the query is cheap enough to run frequently.
- Expose the numbers so alertd reads them instead of running its own SQL.

When that ships, delete the hardcoded relationship map and the SQL from this check and have it read Tamanu's values.
The important discipline is not leaving two implementations of the materialisation predicate in two languages to drift apart.
