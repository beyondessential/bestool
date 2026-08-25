---
id: SUB
---

# Check substrates

A substrate is what a healthcheck asks for the readings it grades.
It identifies what the checking process has access to and what it can find out, so one check runs unchanged whether bestool is installed on the machine it reports for or is observing an application from elsewhere.
See [CHK](healthchecks.md) for the checks themselves and [DOC](doctor.md) for the sweep that runs them.

A substrate is not a proxy that every check routes through.
It answers who the checking process is speaking for and what it can obtain on that subject's behalf; a check that needs nothing from it does not consult it.

## Machines and applications

A **machine** is a host: its filesystems, its clock, its memory and processors, its network identity.
An **application** is one product installed on a machine: its own services, its own version, its own database, its own HTTP traffic, its own certificates, and its own names on the network.

A machine hosts zero or more applications, and the two are reported separately rather than one standing in for the other.
A machine commonly hosts a Tamanu facility application alongside an mSupply application, which share nothing but the machine.
The same holds for two applications of one product, so a machine running both a central and a facility hosts two applications rather than one server of an ambiguous kind.

An application carries the product it is: which duties it can have, which facts describe it, and which checks apply to it all follow from that.

A substrate speaks for exactly one subject — a machine, or a single application.
A machine hosting two applications is covered by three subjects: the machine, and each application separately.

## Graded logic stays in the check

A check keeps its own graded logic: its thresholds, its outcomes, and the wording of its summaries and reasons.
Only the acquisition of a reading varies between substrates.
Two applications running the same check therefore reach their verdicts by the same rules whatever their substrate, and cannot diverge into subtly different checks.

Where a substrate cannot serve a reading a check needs, the check reports skipped with the reason it could not be taken.
A skip is always for a stated reason rather than by accident of what a given substrate happens to expose.

## What a substrate answers for

A substrate is described by what a check can ask it about, grouped by subject.

The **machine**: whether the checking process is running on the machine it reports for.
This is the only thing machine checks need from a substrate, because they read the machine directly rather than through it.

The **workload**: which services make up this application, and the facts that hang off each of them.

**Tamanu**: the application's configuration, its version, whether it is a central or a facility, and a connection to its database.

**A database**: either a means of establishing a connection, or an established connection to run queries against.

## Machine checks read the machine directly

A machine check reads the machine directly rather than through any abstraction, because there is no useful reading to abstract: the concerns it grades are properties of a host, and an application observed from elsewhere has no host of its own to grade.
Each skips when the substrate reports that the checking process is not running on the machine it is reporting for, with that as the stated reason.

Whole-machine memory and load are machine checks.
They measure the host rather than any application on it, which is a signal worth keeping wherever a machine runs one application and bestool runs on it directly, and one that has no meaning when the checking process is elsewhere.

## The workload

An application is served by a set of **services**, each carrying an identifier and a **duty**.
A service is one container, one supervised process, or one pod, and several services commonly share a duty: an API duty usually runs more than one, and a Kubernetes Postgres duty runs a primary alongside a replica where a Linux or Windows one runs a single service.
This holds on every substrate — a Linux machine runs Tamanu as separate containers and cgroup-confined units, a Windows machine runs it as separate supervised processes, and a Kubernetes application runs it as separate pods.

A substrate answers with the list of services making up the application.
Each entry's identifier is what a check passes back to ask for that service's details, so listing the workload and reading a service's facts are separate questions.

### The duty vocabulary

A duty names a product and, within it, the job that service does for that product.
Duties are drawn from a shared vocabulary so that every substrate names the same duty the same way, and a check reads a duty rather than a unit name, a process name, or a pod name.

Tamanu's duties are: API, tasks, sync, frontend, FHIR resolve, FHIR refresh, patient portal, and Postgres.
They carry no central-or-facility distinction, because a duty's job is the same whichever kind of server runs it and which kinds run which duties changes over time.
Whether an application is a central or a facility is a separate fact, answered once for the application rather than encoded into each duty's name.

The vocabulary is organised by product, so products whose duties have nothing in common never share a set of names: an mSupply application's duties do not map onto Tamanu's, and neither has to accommodate the other.
Tamanu is the only product the vocabulary covers, and it is shaped to admit others without that changing.

A service whose duty is outside the vocabulary is carried under its own name rather than dropped, so an application running something the vocabulary does not cover is still reported in full.
This is also how a deployment shape that should no longer exist stays visible: a substrate reports the service it found under the name it found it by, and a check that grades such a service as forbidden finds it there without the shared vocabulary having to carry a duty nothing should be running.

### Service facts

For a service, a substrate answers what version or image it is running, whether it is currently up, and its memory and processor usage.
The service-expectation logic that decides what an application ought to be running grades against these readings, so a shortfall in running services is found the same way on every substrate.

## Resource usage per service

Each service's memory and processor usage are reported as metrics wherever a substrate can read them, dimensioned by duty and service.
These are telemetry rather than a verdict: an application's resource usage is reported whether or not anything grades it.

A service is graded against a ceiling only where one is declared for it.
A declared ceiling is whatever bounds that service specifically: a container's memory limit, a Kubernetes container limit, or the memory bounds configured on a supervised unit.
Where a service declares no ceiling there is no denominator to take a percentage of, so its usage is reported as a metric and the grading skips for that service.
Usage is never graded against the machine's total, because the machine's capacity is shared with everything else on it and says nothing about whether a service is near its own limit.

## Postgres tuning

The tuning check grades an application's Postgres settings against the memory that Postgres may actually use.
That figure is the Postgres service's declared ceiling wherever one exists, on the same terms as any other service's ceiling.
Only where no ceiling is declared, and the substrate is the machine, does the check fall back to the machine's total memory — the reading that is right for a machine running one unconfined Postgres and wrong for everything else.
With neither a ceiling nor a machine to read, there is nothing to tune against and the check skips.

## HTTP traffic and certificates

A check asks the substrate for HTTP traffic statistics for its own application rather than for a particular machine's.
Where an application fronts its own traffic, this reads that front end's statistics for the whole machine.
Where traffic is served by shared infrastructure, this reads that infrastructure's statistics and filters them to the application being reported for, so one reading serves each application behind it separately.

TLS certificates are likewise asked for as the certificates in force for the application, whatever issues and serves them.

A check that grades the front-end software itself, rather than the traffic through it, is a machine check: it reads that software directly and skips when the checking process is not on the machine.

## Splitting the catalogue

Every check in the catalogue reports for either a machine or an application, and its subject determines what it may read.

Machine checks are: filesystem capacity, inodes, btrfs device statistics, clock synchronisation, whole-machine memory, whole-machine load, machine uptime, unexpected local user accounts, the machine's addresses, whether munin-node is installed, the machine's cloud instance tags, Tailscale presence and configuration, bestool's own Canopy enrolment, and the checks that grade the HTTP front-end software itself.

Application checks are: everything that reads the application's database, the application's own HTTP reachability and error rates, its service inventory and version drift, its certificates, and its resource usage per service.

A concern that exists on both sides is two checks rather than one shared check with a conditional subject.
Filesystem capacity is a machine check reading the host's mounts, and volume capacity is an application check reading the size declared for the application's own storage; on a substrate with no machine to read, the first skips and the second still runs.
Uptime is likewise a machine check reading how long the host has been up, and a service fact reading how long each of the application's services has been running.
