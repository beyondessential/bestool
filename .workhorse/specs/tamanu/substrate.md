---
id: SUB
---

# Check substrates

A substrate is what a healthcheck asks for the readings it grades.
It identifies what the checking process has access to and what it can find out, so one check runs unchanged whether bestool is installed on the machine it reports for or is observing an application from elsewhere.
See [SUBJ](subjects.md) for the machines and applications a check reports for, [CHK](healthchecks.md) for the checks themselves, and [DOC](doctor.md) for the sweep that runs them.

A substrate is not a proxy that every check routes through.
It answers who the checking process is speaking for and what it can obtain on that subject's behalf; a check that needs nothing from it does not consult it.

A substrate speaks for exactly one subject — a machine, or a single application.
A machine hosting two applications is covered by three subjects: the machine, and each application separately.

## Graded logic stays in the check

A check keeps its own graded logic: its thresholds, its outcomes, and the wording of its summaries and reasons.
Only the acquisition of a reading varies between substrates.
Two applications running the same check therefore reach their verdicts by the same rules whatever their substrate, and cannot diverge into subtly different checks.

Where a substrate cannot serve a reading a check that applies to its subject needs, the check reports skipped with the reason it could not be taken.
A skip is always for a stated reason rather than by accident of what a given substrate happens to expose.
A check that does not apply to the subject at all is absent from its report rather than skipped, as [SUBJ](subjects.md) describes, so a substrate's inability to serve a reading is never confused with a check having no business there.

## What a substrate answers for

A substrate is described by what a check can ask it about, grouped by subject.

The **machine**: nothing. Machine checks read the host directly rather than through a substrate.

The **workload**: which services make up this application, and the facts that hang off each of them.

**Tamanu**: the application's configuration, its version, its type, and a connection to its database.

**A database**: either a means of establishing a connection, or an established connection to run queries against.

## Machine checks read the machine directly

A machine check reads the host directly rather than through any abstraction, because there is no useful reading to abstract: the concerns it grades are properties of a host, and there is only ever one host to read — the one the checking process is running on.

A machine check therefore needs no substrate, and no guard against being run somewhere it would report the wrong host's facts.
A process that observes applications from elsewhere has no machine subject to report for, so machine checks are absent from what it reports rather than running and skipping.

Whole-machine memory and load are machine checks.
They measure the host rather than any application on it, which is a signal worth keeping wherever a machine runs one application and bestool runs on it directly.

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

## Check state

A check that compares a reading against an earlier one keeps that history in check storage, which is a separate abstraction from the substrate: what a check can find out and where it may remember things are independent questions, and a process observing applications remotely supplies its own storage without having to be the thing that reads them.

Check storage is scoped to the subject the check is reporting for.
Several applications driven from one process therefore never read or write each other's history, and a check's baseline is always a baseline for the application it is grading.

Each check declares whether its stored state survives its application's compute being switched off.
A check whose readings are cumulative counters kept by processes that stop when the compute does discards its state, because those counters restart from zero on waking and a retained baseline would read the fresh counters as a reset or, worse, as a plausible delta.
A check whose readings measure something that persists in the application's own data keeps its state, so a quantity that moved while the application slept is still visible as having moved when it wakes.

A reading whose source is no longer present is an absence rather than a decrease.
Where a substrate's readings come from several sources that come and go, history is kept per source and a source that has vanished is dropped, rather than its disappearance being graded as the quantity having fallen.

## HTTP traffic and certificates

A check asks the substrate for HTTP traffic statistics for its own application rather than for a particular machine's.
Where an application fronts its own traffic, this reads that front end's statistics for the whole machine.
Where traffic is served by shared infrastructure, this reads that infrastructure's statistics and filters them to the application being reported for, so one reading serves each application behind it separately.

TLS certificates are likewise asked for as the certificates in force for the application, whatever issues and serves them.

A check that grades the front-end software itself, rather than the traffic through it, is a machine check: it reads that software directly and skips when the checking process is not on the machine.
