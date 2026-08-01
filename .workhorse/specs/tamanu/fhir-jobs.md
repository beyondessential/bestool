---
id: CHK-FHJ
---

# FHIR jobs backlog

The `fhir_jobs` healthcheck grades the backlog of pending FHIR materialise jobs on a central server, so an operator sees a queue that is growing or stalling before it falls far behind.
It is one of the doctor's healthchecks; see [CHK](healthchecks.md) for the framework it runs in and [DOC](doctor.md) for how checks are selected and rendered.

## Grading

The check measures the pending queue: the number of FHIR jobs not yet worked through, and the age of the oldest such job.
Jobs that have errored are excluded from both measures — they are a record of past failures, not pending work, and are graded by the separate FHIR job-errors check.

The check fails when the pending queue is deep or its oldest job is old, warns at lower thresholds, and passes otherwise.
It skips on a facility server, which runs no FHIR workers, and skips when the deployment has no FHIR jobs table.
It fails when the database cannot be reached.

## Self-healing

When the check fails, its self-heal action restarts the host's FHIR worker services — the resolve worker and the refresh worker — to recover a worker that has stopped or wedged (see [CHK](healthchecks.md#self-healing)).

The restart is attempted only on a central server whose configuration has the FHIR worker enabled.
When the worker is disabled in configuration, no restart is attempted: the workers are meant to be down, and the misconfiguration is graded by the FHIR config check instead.

A restart does not clear a large backlog at once, so the check keeps failing until the queue drains.
To avoid restarting the workers repeatedly while they work through a backlog, the heal action is capped at one attempt an hour.
