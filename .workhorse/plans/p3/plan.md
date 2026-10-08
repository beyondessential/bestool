# Report instanced checks to Canopy

Specs: CHK (Reporting to Canopy, Instances), CHK-SFS, CHK-FMA, and TLS (Healthchecks, covering the Caddy and collection checks).
Depends on a `bes-canopy-api` release carrying `HealthCheck.detail`, `instances` and `HealthCheckInstance`; 1.1.0 already has them.

## Core

- [x] `Check` carries instances (key, result, optional label, detail) beside shared detail.
- [x] An instanced check's status is derived from its instances: the most urgent not skipped, skipped when all are.
- [x] `to_health_check` and `to_wire` put every check's fields in `detail`, including summary and reason; an instanced check sends neither summary nor reason and no flat fields.
- [x] Check-level skip and broken carry no instances.
- [x] Streaming JSON round-trips instances, so the doctor renders them; the doctor's cached-payload reader reads instanced entries and the older flat ones.
- [x] `cap_to` and the severity ceiling act on the derived status only. Canopy grades each instance itself, so instances are left as reported.
- [x] Tests: wire shape for plain, instanced, broken and empty-instance checks; an instance never serialises as broken.

## Checks

- [x] `sync_facility_stale`: instance per active non-mobile device keyed by `deviceId`, labelled with facility names, sessions without a device id ignored.
- [x] `fhir_materialisation`: instance per resource; disabled and upstream-absent skipped; errored warning; one `unmonitored` warning instance.
- [x] `caddy_certs`: instance per certificate.
- [x] `canopy_certificates`: instance per DNS name; undeclared and denied are skipped instances.
- [x] `btrfs`, `inodes`, `disk_free`: instance per mount point. A mount that could not be inspected is a warning instance with the error in its detail. `btrfs` not installed stays a check-level skip.
- [x] `sync_restart_loop`: instance per device with restart errors in the last hour, keyed by `deviceId`, labelled with facility names, graded per device by the existing 5 and 10 per hour thresholds.
- [x] `sync_session_errors`: instance per non-mobile device with errors in the last minute keyed by `deviceId`, plus one `mobile` instance for mobile errors. An instance warns on any error and fails at ten or more. Counters unchanged.
- [x] Not instanced: `version_drift`, `service_resources`, `held_captures`, `fhir_service_requests_unresolved`, `http_errors`, `tamanu_service`, `external_users`.
- [x] `with_stat` untouched everywhere.

## Out of scope

An instance naming the target it concerns, which needs a facility's alertd to declare its `deviceId` as an alias on its application.
