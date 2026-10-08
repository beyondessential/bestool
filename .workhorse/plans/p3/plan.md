# Report instanced checks to Canopy

Specs: CHK (Reporting to Canopy, Instances), CHK-SFS, CHK-FMA, and TLS (Healthchecks, covering the Caddy and collection checks).
Depends on a `bes-canopy-api` release carrying `HealthCheck.detail`, `instances` and `HealthCheckInstance`; 1.1.0 already has them.

## Core

- [ ] `Check` carries instances (key, result, optional label, detail) beside shared detail.
- [ ] An instanced check's status is derived from its instances: the most urgent not skipped, skipped when all are.
- [ ] `to_health_check` and `to_wire` put every check's fields in `detail`, including summary and reason; an instanced check sends neither summary nor reason and no flat fields.
- [ ] Check-level skip and broken carry no instances.
- [ ] Streaming JSON round-trips instances, so the doctor renders them.
- [ ] `cap_to` and the severity ceiling act on the derived status; decide whether the ceiling also applies per instance for local rendering.
- [ ] Tests: wire shape for plain, instanced, broken and empty-instance checks; an instance never serialises as broken.

## Checks

- [ ] `sync_facility_stale`: instance per active non-mobile device keyed by `deviceId`, labelled with facility names, sessions without a device id ignored.
- [ ] `fhir_materialisation`: instance per resource; disabled and upstream-absent skipped; errored warning; one `unmonitored` warning instance.
- [ ] `caddy_certs`: instance per certificate.
- [ ] `canopy_certificates`: instance per DNS name; undeclared and denied are skipped instances.
- [ ] `btrfs`, `inodes`, `disk_free`: instance per mount point. A mount that could not be inspected is a warning instance with the error in its detail. `btrfs` not installed stays a check-level skip.
- [ ] `sync_restart_loop`: instance per device with restart errors in the last hour, keyed by `deviceId`, labelled with facility names, graded per device by the existing 5 and 10 per hour thresholds.
- [ ] `sync_session_errors`: instance per non-mobile device with errors in the last minute keyed by `deviceId`, plus one `mobile` instance for mobile errors. An instance warns on any error and fails at ten or more. Counters unchanged.
- [ ] Not instanced: `version_drift`, `service_resources`, `held_captures`, `fhir_service_requests_unresolved`, `http_errors`, `tamanu_service`, `external_users`.
- [ ] `with_stat` untouched everywhere.

## Out of scope

An instance naming the target it concerns, which needs a facility's alertd to declare its `deviceId` as an alias on its application.
