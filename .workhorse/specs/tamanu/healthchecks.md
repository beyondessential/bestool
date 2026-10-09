---
id: CHK
---

# Healthchecks

The doctor and the alertd daemon run a shared registry of named healthchecks against a host and its Tamanu deployment. Each check resolves to one of the outcomes and is selected, ordered, and rendered as described in `tamanu/doctor.md`.

This spec is the parent for the healthcheck catalogue: the conventions common to every check, with each check that warrants its own acceptance criteria captured in a sibling spec.

Which subject a check reports for — a machine or an application on it — is described in [SUBJ](subjects.md), and how a check obtains its readings for that subject in [SUB](substrate.md).

## Assembling a suite

The catalogue is a registry to select from, not a fixed sweep to run.
Each check declares the subjects it reports for, and a consumer assembles the suite it needs by taking the checks the subjects in front of it admit.
The doctor command and the alertd daemon are two such consumers, and a process that observes applications it does not host is another.

Selecting this way rather than naming checks one by one means a check added to the catalogue reaches every consumer whose subjects admit it, and a consumer that should not run one says so by not presenting a subject it applies to.
No consumer holds its own list of check names to keep in step with the registry.

The checks and the machinery they are built from — the outcomes, the metrics a check declares, the storage it remembers readings in — are available to a consumer independently of the daemon that runs them on a schedule.
A check is therefore the unit that is shared between consumers, so two environments running the same check cannot grade it differently.

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

## Reporting to Canopy

A check's outcome travels to Canopy as one entry in its subject's health list.
Every entry carries the check's fields inside a `detail` object, and nothing beside the check's name and its result or instances.
The check's summary and, for a non-passing check, its reason are two of those fields.

## Instances

A check whose condition holds several times over reports each occurrence as an instance, rather than as an array or keyed object inside its detail.
Canopy grades, silences and presents an instance by itself, which a field inside the detail of a single result cannot be.
A check with nothing to distinguish its occurrences reports a single result.

An instanced check reports its instances in place of a result.
Each instance has:

- a key, chosen by the check, non-empty, unique within the check and the same for the same occurrence on every sweep.
  A key names the occurrence itself, never a value it currently has, so that Canopy's silences on it keep meaning the same thing.
- a result of passed, warning, failed or skipped.
- an optional label naming the occurrence to an operator.
- its own detail, to which the reason a non-passing instance carries is added.

The instances are the complete set.
Instances that pass are reported as well as the degraded ones, and a check with no occurrences reports an empty set, which recovers everything Canopy held for it.
A check that can only observe the occurrences that have something to report, such as errors inside a window, reports those, and an occurrence it stops reporting has recovered.

What the instances share goes in the check's detail, and an instanced check has no fields beside its instances.
An instanced check carries no summary or reason of its own on the wire, because Canopy writes the check's message from its graded instances.

Broken belongs to the whole check.
A check that cannot run because its own query is faulty reports broken with no instances, and Canopy keeps the instances it held without recovering any.
A check whose query fails for any other reason reports failed, and one that skips for want of a precondition, such as a database connection, reports skipped, each with no instances.
No instance reports broken.
A check that did run but could not read one occurrence reports that instance as a warning with the error in its detail, since one unreadable occurrence is not the whole check failing to run.

An instanced check's own status, for the doctor's rendering, the heal trigger and the severity ceiling, is that of its most urgent instance that is not skipped, and skipped when every instance is.
The check keeps a summary for local rendering, which is not sent.

The numeric telemetry a check declares ([MET](metrics.md)) is independent of how it reports its outcome.

## Services on this machine

A check that reaches a service on its own machine over HTTP, such as Tamanu's API or Caddy's admin interface, opens a new connection for every request.
It never sends a request on a connection left open by an earlier sweep.

> [!NOTE]
> Some hosts drop a connection that sits idle between sweeps without closing it, and a request sent on one waits out its timeout even though the service is answering.
> Opening a new connection each time means the result reflects whether the service answers now, so a healthy service does not read as failing on alternate sweeps.

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

The rate limit and the backoff belong to a check on one subject, since a name identifies a check only together with the subject it reports for ([SUBJ](subjects.md)).
Two applications running the same failing check each get their own attempts, so one application's repair does not consume another's allowance or defer it behind a backoff it played no part in.

Each check sets a minimum interval between its own heal attempts.
Most checks use a short default; a check whose repair is disruptive, or whose effect on the graded condition lands only slowly, sets a longer floor.
The minimum interval bounds every attempt, including one made straight after a successful repair, so a repair whose effect is not yet visible to the check does not trigger a second repair before the floor has elapsed.

At most one heal attempt for a given check on a given subject runs at a time.
Because attempts run in the background, one can take longer than the interval between sweeps; a sweep does not start a heal for a check whose previous attempt has not yet finished.
A heal runs against the context its check ran with, so a repair made for one application acts on that application rather than on whichever one a shared context happened to hold.
