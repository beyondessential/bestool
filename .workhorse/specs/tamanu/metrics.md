---
id: MET
---

# Doctor metrics

The alertd daemon exposes the numeric telemetry its healthchecks gather as metrics on its `/metrics` endpoint, in either prometheus or munin format.
See [CHK](healthchecks.md) for the checks themselves and [DOC](doctor.md) for the sweep that produces them.

Metrics are additive telemetry: what a check declares as metrics never changes its verdict, its summary, or the facts it reports to Canopy.

## The endpoint

`/metrics` serves prometheus text by default, and for a request whose `Accept` offers `*/*` or a prometheus content type, so existing scrapers are unaffected.
It serves munin text when the request's `Accept` asks for the munin content type.

Munin scrapes in two calls: a config request returns field and graph metadata, and a bare request returns values.
Both are rendered from the same sweep snapshot within a scrape, so a graph's declared fields and its values always agree.

A thin munin plugin ships in the bestool deb and is wired active on hosts where munin-node is present.
The daemon also reports whether munin-node is present on the host as a top-level fact to Canopy.

## Daemon liveness

The daemon always reports how long ago it was last active, and — once it has completed a sweep — how long ago that sweep finished, each as an age in seconds rather than an absolute timestamp, so a scrape can alert on staleness without computing the difference itself.

## Check census

Every sweep yields a count of checks by outcome — passing, warning, failing, skipped, broken — and a total, capped to Canopy's effective severities so the census matches what operators see elsewhere.
In munin the census is a single stacked-area graph, so the make-up of the host's check outcomes reads at a glance.

## Declared metrics

A check declares typed metrics. Each metric carries:

- a name in `snake_case` that is a valid prometheus name and munin field;
- a value;
- a kind — a gauge that rises and falls, or a counter that only increases;
- optional labels: dimensions with a fixed key and a per-series value, such as a mount point, an HTTP status code, or a percentile;
- a human description;
- an optional group (see below);
- an optional namespace.

A metric's unit lives in its name by convention — `_seconds`, `_bytes`, `_ms` — and a metric that measures a duration or a size always names its unit, so no metric reads as a bare unitless number when it is really a quantity.
In munin a graph labels its axis with the unit read from its metrics' names, and a byte graph scales in powers of 1024, so an operator reads seconds, bytes, or a percentage off the axis directly.

Metrics are named under their check by default, but a check whose name would misrepresent its telemetry may declare a namespace that replaces the check name in the metric's name.
The error-rate `http_errors` check publishes its request telemetry — which counts successful responses too — under `http`, so a reader isn't misled into thinking a request count is an error count.

## Grouping into munin graphs

Prometheus models dimensioned metrics with labels natively: one metric family per name, one series per label combination.

Munin has no labels, so a check's metrics are grouped into graphs.
There is one graph per group; a metric with no explicit group forms its own graph, named for the metric.
Metrics that share a unit and a comparable scale and are read together — a warn tier beside its fail tier, used beside total — declare a shared group and share a graph.
Metrics of different units are never placed in the same graph, so no graph mixes scales on one axis; and a total that dwarfs the components it sums stays on its own graph, so the components' variation isn't flattened against it.
The label-dimensioned series of a single metric — one per mount, per status code, per percentile — are the fields of that metric's graph.

## Sync session durations

The sync-sessions check reports the number of currently active sync sessions, and, for the most recently completed session, the duration in seconds of each phase of the sync: the snapshot phase, the persist phase, and the session overall.
The phase durations share one graph.

## Sync snapshot table sizes

The sync-snapshot-tables check reports the p50 and p99 of the leftover snapshot tables' sizes in bytes, as two series of one percentile graph, alongside the count of tables and recent sessions.
The total size of all snapshot tables is a separate graph: it dwarfs the percentiles, so sharing an axis would flatten their variation.

## FHIR workers

The FHIR-workers check ([CHK-FWK](fhir-workers.md)) reports the health of a central's FHIR materialisation workers, read from their heartbeats.

It reports the count of live workers beside the count of dropped workers — those that stopped heartbeating without deregistering — as the two components of one graph.
It reports the age in seconds of the oldest live worker's heartbeat, so an operator sees how close the least-fresh worker is to the drop window, on its own graph.
It reports the number of jobs the live workers have completed successfully and the number they have failed, summed across the live workers as one metric dimensioned by outcome; because a worker's identity is fresh each time its process starts, these are aggregate throughput rather than a per-worker or monotonic series.
