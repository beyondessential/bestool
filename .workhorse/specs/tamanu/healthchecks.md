---
id: CHK
---

# Healthchecks

The doctor and the alertd daemon run a shared registry of named healthchecks against a host and its Tamanu deployment. Each check resolves to one of the outcomes and is selected, ordered, and rendered as described in `tamanu/doctor.md`.

This spec is the parent for the healthcheck catalogue: the conventions common to every check, with each check that warrants its own acceptance criteria captured in a sibling spec.

## Spec identifiers

Every spec describing an individual healthcheck carries a frontmatter `id` of the form `CHK-<id>`, where `<id>` is a short identifier for that check (for example `CHK-CFV` for the Caddyfile version check). The shared `CHK-` prefix distinguishes healthcheck specs from other specs at a glance and groups them for code-to-spec traceability.

## Concurrent execution

The checks in a sweep run concurrently and independently of one another.
A check that is slow, that waits on an unresponsive host, or that occupies its thread for an extended period delays only its own result: the other checks in the sweep continue to make progress and complete on their own schedule.
This holds for every check regardless of what it does internally, so adding a check that consults a slow external tool cannot degrade the rest of the sweep.

A duration a check reports — a connect latency, a response time — measures only the work that check performed.
Time the sweep spends running other checks is never counted against it, so a reported duration is a usable signal about the thing being graded rather than an artefact of what else the sweep was doing.
The same check run alone and run as part of a full sweep reports durations in the same range.

A check that errors so severely that it produces no result at all is reported as broken, for that check alone, and the rest of the sweep completes and reports normally.

## Self-healing

A check may declare a self-heal action: a repair the daemon attempts, while the check is failing, to recover the condition the check grades without operator action.

Self-healing is a responsibility of the long-running alertd daemon.
The interactive doctor command reports check outcomes but never attempts repairs, so running it by hand has no side effects on the host.

The daemon attempts a check's heal action only when that check's latest outcome is a failure — not a warning, a skip, or a check that errored — and always in the background.
A heal attempt never delays the sweep or the status report to Canopy, so a slow or stuck repair cannot hold up alerting.

A heal attempt never changes the outcome reported for the sweep that triggered it.
A successful repair takes effect in a later sweep, once the healed condition is observed afresh; the daemon does not have to be restarted for a repair to take effect.

Heal attempts for a given check are rate-limited and back off on repeated failure, so a check that cannot yet be healed — because a dependency is unreachable, say — does not retry its repair on every sweep.
A heal attempt that fails or cannot proceed is logged and retried later under the backoff schedule.

Each check sets a minimum interval between its own heal attempts.
Most checks use a short default; a check whose repair is disruptive, or whose effect on the graded condition lands only slowly, sets a longer floor.
The minimum interval bounds every attempt, including one made straight after a successful repair, so a repair whose effect is not yet visible to the check does not trigger a second repair before the floor has elapsed.

At most one heal attempt for a given check runs at a time.
Because attempts run in the background, one can take longer than the interval between sweeps; a sweep does not start a heal for a check whose previous attempt has not yet finished.
