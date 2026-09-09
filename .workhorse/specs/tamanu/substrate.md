---
id: SUB
---

# Check substrates

A substrate is how a check reads the runtime of the application it reports for: what is running that application, what traffic reaches it, and what certificates stand in front of it.
It exists so that one check works whether bestool is installed alongside an application or is observing it from elsewhere.
See [SUBJ](subjects.md) for the subjects a check reports for, [CHK](healthchecks.md) for the checks themselves, and [DOC](doctor.md) for the sweep that runs them.

A substrate is not a layer every check routes through.
It covers only what genuinely differs between environments: an application's services are found through a supervisor on one machine, a container runtime on another, and a cluster's API on a third, and the same reading has to come out of all three.

What a check needs that does not differ is supplied to it rather than asked for.
A connection to a database, an application's configuration, its version and its type are parameters: they are the same kind of thing wherever the application runs, and wrapping them in an abstraction would only restate what the sweep already knows.

Machine checks use no substrate at all.
There is only ever one host to read — the one the checking process runs on — and a process with no machine to report for has no machine subject, so those checks are absent from what it reports rather than reading anything.

## Graded logic stays in the check

A check keeps its own graded logic: its thresholds, its outcomes, and the wording of its summaries and reasons.
Only the acquisition of a reading varies between substrates.
Two applications running the same check therefore reach their verdicts by the same rules whatever their substrate, and cannot diverge into subtly different checks.

This is why a reading that feeds a threshold is asked for rather than handed in.
A consumer that supplied the numbers itself could grade against a different denominator than another consumer running the same check, which is the divergence the abstraction exists to prevent.

Where a substrate cannot serve a reading a check that applies to its subject needs, the check reports skipped with the reason it could not be taken.
A check that does not apply to the subject at all is absent from its report rather than skipped, as [SUBJ](subjects.md) describes, so a substrate's inability to serve a reading is never confused with a check having no business there.

## The workload

An application is served by a set of **services**, each carrying an identifier and a **duty**.
A service is one container, one supervised process, or one pod, and several services commonly share a duty: an API duty usually runs more than one, and a frontend duty runs a named instance per slot.
This holds on every substrate — a Linux machine runs Tamanu as separate containers and cgroup-confined units, a Windows machine runs it as separate supervised processes, and a Kubernetes application runs it as separate pods.

A substrate answers with the list of services making up the application.
Each entry's identifier is what a check passes back to ask for that service's details, so listing the workload and reading a service's facts are separate questions.

### The duty vocabulary

A duty names a product and, within it, the job that service does for that product.
Duties are drawn from a shared vocabulary so that every substrate names the same duty the same way, and a check reads a duty rather than a unit name, a process name, or a pod name.

Tamanu's duties are: API, tasks, sync, frontend, FHIR resolve, FHIR refresh, and patient portal.
They carry no central-or-facility distinction, because a duty's job is the same whichever kind of server runs it and which kinds run which duties changes over time.
The role an application plays is carried by its type, so it is stated once for the application rather than encoded into each duty's name.

A Postgres installation is an application in its own right rather than a duty of whatever uses it, so its services are its own — a single server on a machine, or a primary alongside its replicas on a substrate that runs it that way.

The vocabulary is organised by product, so products whose duties have nothing in common never share a set of names: an mSupply application's duties do not map onto Tamanu's, and neither has to accommodate the other.
Tamanu is the only product the vocabulary covers, and it is shaped to admit others without that changing.

A service whose duty is outside the vocabulary is carried under its own name rather than dropped, so an application running something the vocabulary does not cover is still reported in full.
This is also how a deployment shape that should no longer exist stays visible: a substrate reports the service it found under the name it found it by, and a check that grades such a service as forbidden finds it there without the shared vocabulary having to carry a duty nothing should be running.

### Service facts

For a service, a substrate answers what version or image it is running, whether it is currently up, and its memory and processor usage.
The service-expectation logic that decides what an application ought to be running grades against these readings, so a shortfall in running services is found the same way on every substrate.

## Applications with their compute switched off

An application can exist and hold its data while all of its compute is switched off, and a substrate reports that state for the application it speaks for.

An application in that state has no running services and no reachable database, which is the intended condition rather than a fault.
Checks that need a running service or a live database skip, giving that state as the reason, so an application that is deliberately switched off does not alert.
Facts that come from the application's declared configuration rather than from anything running are reported as usual, so it remains identifiable while it sleeps.

## Resource usage per service

Each service's memory and processor usage are reported as metrics wherever a substrate can read them, dimensioned by duty and service.
These are telemetry rather than a verdict: an application's resource usage is reported whether or not anything grades it.

A service is graded against a ceiling only where one is declared for it.
A declared ceiling is whatever bounds that service specifically: a container's memory limit, a Kubernetes container limit, or the memory bounds configured on a supervised unit.
Where a service declares no ceiling there is no denominator to take a percentage of, so its usage is reported as a metric and the grading skips for that service.
Usage is never graded against the machine's total, because the machine's capacity is shared with everything else on it and says nothing about whether a service is near its own limit.

## Postgres tuning

The tuning check reports for a Postgres application, and grades its settings against the memory that server may actually use.
That figure is the declared ceiling of the service running it wherever one exists, on the same terms as any other service's ceiling.
Only where no ceiling is declared does the check fall back to the memory of the machine hosting it — the reading that is right for a machine running one unconfined server and wrong for everything else.
With neither a ceiling nor a hosting machine to read, there is nothing to tune against and the check skips.

## HTTP traffic and certificates

A check asks the substrate for HTTP traffic statistics for its own application rather than for a particular machine's.
Where an application fronts its own traffic, this reads that front end's statistics for the whole machine.
Where traffic is served by shared infrastructure, this reads that infrastructure's statistics and filters them to the application being reported for, so one reading serves each application behind it separately.

TLS certificates are likewise asked for as the certificates in force for the application, whatever issues and serves them.

A check that grades the front-end software itself, rather than the traffic through it, is a machine check and reads that software directly.

## Check state

A check that compares a reading against an earlier one keeps that history in check storage, which is a separate abstraction from the substrate: what a check can find out and where it may remember things are independent questions, and a process observing applications remotely supplies its own storage without having to be the thing that reads them.

Check storage is scoped to the subject the check is reporting for.
Several applications driven from one process therefore never read or write each other's history, and a check's baseline is always a baseline for the application it is grading.

Each check declares whether its stored state survives its application's compute being switched off.
A check whose readings are cumulative counters kept by processes that stop when the compute does discards its state, because those counters restart from zero on waking and a retained baseline would read the fresh counters as a reset or, worse, as a plausible delta.
A check whose readings measure something that persists in the application's own data keeps its state, so a quantity that moved while the application slept is still visible as having moved when it wakes.

A reading whose source is no longer present is an absence rather than a decrease.
Where a substrate's readings come from several sources that come and go, history is kept per source and a source that has vanished is dropped, rather than its disappearance being graded as the quantity having fallen.
