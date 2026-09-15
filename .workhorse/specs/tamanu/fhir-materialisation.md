---
id: CHK-FMA
---

# FHIR materialisation gap

The `fhir_materialisation` healthcheck grades whether upstream Tamanu records are reaching their materialised FHIR resources, so an operator sees a record that was never materialised at all.
It is one of the doctor's healthchecks; see [CHK](healthchecks.md) for the framework it runs in and [DOC](doctor.md) for how checks are selected and rendered.
The numeric telemetry it declares is specified in [MET](metrics.md).

Every other FHIR check measures the queue machinery rather than its outcome: [CHK-FHJ](fhir-jobs.md) measures pending work, the job-errors check measures work that failed, [CHK-FWK](fhir-workers.md) measures worker liveness, and the unresolved-service-requests check measures the resolution state of rows that have already been materialised.
None of them can see an upstream record for which no materialisation was ever queued, whether because a trigger was missed, materialisation for that resource was switched off, or the job queue was truncated without the re-materialisation that recovering from that requires.
In that state every one of those checks reads green while the record is invisible to any integration consuming the FHIR API.

## What it measures

For each materialised FHIR resource, the check measures the number of upstream records that have no corresponding FHIR row, and the age of the oldest such record.

The age carries the grading.
A count alone is inherently noisy, because there is always a transient nonzero count in the moments between an upstream record being written and its materialisation, so any threshold on the count either alerts constantly or is set high enough to miss a real gap.
An age distinguishes a handful of records seconds old and materialising normally from the same handful hours old and stuck, and needs no per-deployment calibration because it is an absolute duration.

Both are reported per resource, and the resource with the oldest gap is named in the check's summary, except where its pace keeps it off that headline.

The check considers only upstream records written recently: within the last two days for a resource expected to materialise promptly, and within the last week for a deferred one.
An older gap is a backfill concern rather than an incident, and bounding the measurement keeps it cheap enough to run on every sweep.
A resource's window always outlasts the age at which it fails, so a gap can age into failing before it leaves the measurement.

Presence of the FHIR row is the whole test, and its resolution state is not consulted: a materialised but unresolved row is graded by the unresolved-service-requests check and must not be counted as a gap as well.
Soft-deleted upstream records are excluded, so a cancelled clinical record does not read as a gap.

## Which resources it measures

The materialised resources are read from the deployment's live database schema: a table in the `fhir` schema that carries an `upstream_id` column is a materialised resource, and one that does not is not.
Reading them from the schema rather than deriving table names from resource names avoids the deployment's inflection rules, which have at least one exception, and means a resource that is computed on read rather than materialised is never collected.

The relationship between a materialised resource and the upstream records it materialises from cannot be read from the schema.
It is an arbitrary declaration in Tamanu, and `upstream_id` is polymorphic where a resource has more than one upstream, so there is no key to follow.
The check therefore carries the relationship as known data:

| FHIR resource | FHIR table | upstream table(s) | upstream records considered | pace |
| --- | --- | --- | --- | --- |
| `ServiceRequest` | `service_requests` | `lab_requests`, `imaging_requests` | all | prompt |
| `Patient` | `patients` | `patients` | all | prompt |
| `Practitioner` | `practitioners` | `users` | all | prompt |
| `Organization` | `organizations` | `facilities` | all | prompt |
| `Immunization` | `immunizations` | `administered_vaccines` | all | prompt |
| `MedicationRequest` | `medication_requests` | `pharmacy_order_prescriptions` | all | prompt |
| `Specimen` | `specimens` | `lab_requests` | those with a specimen attached | prompt |
| `Encounter` | `encounters` | `encounters` | those that are not a survey response | prompt |
| `MediciReport` | `non_fhir_medici_report` | `encounters` | those that are not a survey response | deferred |

A resource with more than one upstream table reports one gap and one age across all of them, not one per upstream.

Because the three narrowed upstreams are the only ones Tamanu narrows, the check measures the same records Tamanu's own materialisation does, rather than an approximation of them.

Known data goes stale, so the check cross-references what it discovered against what it knows.
A materialised resource present in the schema that the check has no relationship for is reported by name as unmonitored, and the check warns.
Without that, a future Tamanu version adding a materialised resource would silently narrow the check's coverage; with it, the check reports that it has gone out of date.
The cross-reference runs in that direction only: a resource the check knows about but that has no table in this deployment is absent by design on that version, not a fault.

## Applicability

The check only applies to a central server whose FHIR materialisation worker is enabled.

- It skips on a facility server.
- It skips when the FHIR worker is not enabled for the deployment, because then no upstream record is expected to be materialised and every resource would report a total gap.
- It skips when the deployment has no materialised resources at all, as on a version predating the `fhir` schema.
- It skips when no resource has materialisation enabled, because then nothing is expected to materialise and there is no gap to measure.
- It skips when the database cannot be reached, so that a database outage remains something the daemon can alert on rather than something that stops it.

A skip carries a reason naming which precondition was not met.

## Per-resource enablement

Materialisation is enabled or disabled per resource, and on a stock deployment every resource except `Patient` is disabled.
A check that ignored this would report a total phantom gap for every disabled resource on every deployment, so resolving enablement correctly is what makes the check usable at all.

Enablement for a resource is resolved from the first of these that is available:

1. The deployment's stored setting for that resource, under `fhir.worker.resourceMaterialisationEnabled`. A resource enabled for any one facility is enabled server-wide, matching how the deployment merges the setting across facilities, so a per-facility reading is not needed.
2. The deployment's configuration, under `integrations.fhir.worker.resourceMaterialisationEnabled`, which is where versions before the setting existed keep the same flags.
3. Whether the resource has ever materialised anything: a resource with at least one FHIR row is treated as enabled, and one with none as disabled.

The last of these is inference rather than a reading, and it exists so the check still functions where neither of the declared sources carries the resource.
Its failure mode is to under-report a resource that is enabled but has yet to materialise anything, and to report a growing gap for one disabled after rows had accumulated — which is itself worth surfacing, as materialisation having been switched off with data already present.
The check reports, per resource, which of the three answered, so an operator can see when a verdict rests on inference.

A resource whose materialisation is disabled is omitted from the measurement entirely and declares no metrics, rather than reporting a gap of zero: a zero would be indistinguishable from a resource that is enabled and up to date.

A resource whose upstream table does not exist on the deployment's version is likewise omitted, without being treated as a fault: it is absent by design on that version, as with a resource that has no FHIR table.
A resource the check could not measure for any other reason is named, and the check warns, so lost coverage is visible rather than reading as an absence of gaps.

## Materialisation pace

Not every materialised resource is expected to keep pace with its upstream, and grading them all on one clock would mean permanently failing a deployment that is working as designed.
Each resource therefore carries a pace, which sets the thresholds its gap is graded against, the window its measurement is bounded to, and whether its backlog joins the check's headline.

A prompt resource is materialised off the upstream write and is expected to keep pace with it, so a gap persisting beyond minutes means something has gone wrong.
Every resource but `MediciReport` is prompt.

`MediciReport` is deferred.
It is not a FHIR resource served to integrations but a single-purpose report, and it is materialised behind every prompt resource, so it runs slowly and carries a large standing backlog as a matter of course.

A deferred resource differs from a prompt one in three ways.

- It fails only once its oldest gap is older than five days, which is long enough to mean materialisation has stopped rather than merely fallen behind.
- It has no warning threshold, because a backlog that is expected has no degraded state between working and stopped.
- Its backlog is kept out of the check's headline count and the oldest gap that headline names, because that backlog is routinely the largest number the check holds and folding it in would bury the gaps that do mean something.
  It is reported in its own right on the headline instead, and declares the same per-resource numbers as any other resource.

## Outcomes

For a central server with the FHIR worker enabled and at least one resource enabled, every enabled resource is graded against the thresholds of its own pace, and the check takes the worst grade any of them reaches:

- [ ] The check fails when a prompt resource's oldest gap is older than an hour.
- [ ] The check warns when a prompt resource's oldest gap is older than a quarter of an hour but no older than an hour.
- [ ] The check fails when a deferred resource's oldest gap is older than five days, and is unaffected by the size of that resource's backlog.
- [ ] The check warns when the schema contains a materialised resource the check has no relationship for, whatever the measured gaps.
- [ ] The check passes when no enabled resource has crossed a threshold of its own pace.
- [ ] A non-passing grade names the resource that drove it and the threshold that resource crossed.
  Where more than one resource has crossed a threshold, the most severe grade is named, and between equal grades the resource furthest past its own threshold in proportion to it.

## Retirement

The deployment already computes this number, as part of the nightly task that re-queues missing resources, and discards it: the task sums the gap across all resources, logs the total, and keeps neither the per-resource split nor any age.
This check exists because reading it from the deployment would take longer to reach the deployments that need measuring than measuring it directly.

It is superseded once the deployment reports the gap and its age per resource itself.
At that point the check reads the deployment's numbers, and the relationship data it carries is deleted rather than kept alongside them, so the definition of a materialisation gap does not exist twice and drift.
