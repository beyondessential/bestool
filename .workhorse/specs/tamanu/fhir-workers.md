---
id: CHK-FWK
---

# FHIR job worker healthcheck

The Tamanu doctor runs a check that verifies a central's FHIR materialisation workers are alive and heartbeating. Tamanu's FHIR workers register a row in `fhir.job_workers` and update its `updated_at` on a periodic heartbeat; job grabbing, completion, and stuck-job reclamation all gate on that heartbeat being recent, so a worker that has silently stopped heartbeating stalls FHIR materialisation even while its service process looks up. This check surfaces that condition, which neither the service-up check nor the jobs-backlog check catches on its own.

It is one of the healthchecks described by `tamanu/healthchecks.md` and follows the shared outcome model in `tamanu/doctor.md` (pass, skip, warning, fail, broken).
The numeric telemetry it declares is specified in `tamanu/metrics.md`.

## Applicability

The check only applies to a central server whose FHIR worker is enabled.

- It skips on a facility server.
- It skips when the FHIR worker is not enabled in the deployment's configuration.
- It skips when `fhir.job_workers` is absent, as on a Tamanu version that predates the table.

A skip carries a reason naming which precondition was not met.

## Liveness model

A worker's liveness is read the same way Tamanu itself reads it: a worker is live when its heartbeat is recent, where "recent" is the deployment's `fhir.worker.assumeDroppedAfter` setting, defaulting to 10 minutes when the setting is absent.

- A row is a **live** worker when it is not soft-deleted (`deleted_at` is null) and its `updated_at` is within the window.
- A row is a **dropped** worker when it is not soft-deleted but its `updated_at` is older than the window — a worker that crashed or was killed without deregistering, so its row lingers with a frozen heartbeat.
- A soft-deleted row (`deleted_at` set) is a worker that shut down gracefully and is not counted as either live or dropped.

The window is read from the setting so it tracks the deployment rather than a value baked into the check.

## Outcomes

For a central server with the FHIR worker enabled:

- [ ] The check fails when no worker is live.
- [ ] The check warns when at least one worker is live but one or more dropped workers are present.
- [ ] The check passes when at least one worker is live and no dropped workers are present.

A count of live workers cannot be asserted against a fixed expectation: the number of worker rows is emergent from deployment topology (a running server registers a `refresh` and a `resolver` worker, multiplied by however many server processes run), so the check grades the presence of liveness, not a specific worker count.
