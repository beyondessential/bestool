---
id: SUB
---

# Check substrates

A substrate is what a healthcheck asks for the readings it grades.
It identifies what the checking process has access to and what it can find out about a deployment, so one check runs unchanged whether bestool is installed on the server it reports for or is observing a deployment from elsewhere.
See [CHK](healthchecks.md) for the checks themselves and [DOC](doctor.md) for the sweep that runs them.

A substrate is not a proxy that every check routes through.
It answers who the checking process is speaking for and what it can obtain on that deployment's behalf; a check that needs nothing from it does not consult it.

## Graded logic stays in the check

A check keeps its own graded logic: its thresholds, its outcomes, and the wording of its summaries and reasons.
Only the acquisition of a reading varies between substrates.
Two deployments running the same check therefore reach their verdicts by the same rules whatever their substrate, and cannot diverge into subtly different checks.

Where a substrate cannot serve a reading a check needs, the check reports skipped with the reason it could not be taken.
A skip is always for a stated reason rather than by accident of what a given substrate happens to expose.

## Systems and deployments

A **system** is a machine. A **deployment** is one product installed on it: its own services, its own version, its own database, its own HTTP traffic, its own certificates, and its own names on the network.

A system hosts zero or more deployments, and a substrate speaks for exactly one of them.
A machine running two is served by two substrates, one per deployment, and their readings are separate throughout — a Linux server commonly hosts a Tamanu facility deployment alongside an mSupply deployment, which share nothing but the machine.
The same holds for two deployments of one product, so a machine running both a central and a facility is two deployments rather than one server of an ambiguous kind.

A deployment carries the product it is: which duties it can have, which facts describe it, and which checks apply to it all follow from that.

## What a substrate answers for

A substrate is described by what a check can ask it about, grouped by subject.

The **system**: whether the checking process is running on the machine hosting the deployment it reports for.
This is the only thing checks in this group need from a substrate, because they read the machine directly rather than through it.

The **workload**: which services make up this deployment, and the facts that hang off each of them.

**Tamanu**: the deployment's configuration, its version, whether it is a central or a facility, and a connection to its application database.

**A database**: either a means of establishing a connection, or an established connection to run queries against.

## Checks that read the system directly

Some checks are about the machine and nothing else: filesystem capacity and inodes, btrfs device statistics, clock synchronisation, Tailscale presence and configuration, unexpected local user accounts, held captures, whether munin-node is installed, the host's addresses, whole-system memory and load, and host uptime.

These read the machine directly rather than through any abstraction, because there is no useful reading to abstract: the concerns they grade are properties of a machine, and a deployment observed from elsewhere has no machine of its own to grade.
Each skips when the substrate reports that the checking process is not the system it is reporting for, with that as the stated reason.

Whole-system memory and load stay in this group.
They measure the machine rather than the deployment, which is a signal worth keeping wherever a server is monolithic and bestool runs on it directly, and one that has no meaning at all when it is not.

## The workload

A deployment is served by a set of **services**, each carrying an identifier and a **duty**.
A service is one container, one supervised process, or one pod, and several services commonly share a duty: an API duty usually runs more than one, and a Kubernetes Postgres duty runs a primary alongside a replica where a Linux or Windows one runs a single service.
This holds on every substrate — a Linux server runs Tamanu as separate containers and cgroup-confined units, a Windows server runs it as separate supervised processes, and a Kubernetes deployment runs it as separate pods.

A substrate answers with the list of services making up the deployment.
Each entry's identifier is what a check passes back to ask for that service's details, so listing the workload and reading a service's facts are separate questions.

### The duty vocabulary

A duty names a product and, within it, the job that service does for that product.
Duties are drawn from a shared vocabulary so that every substrate names the same duty the same way, and a check reads a duty rather than a unit name, a process name, or a pod name.

Tamanu's duties are: API, tasks, sync, frontend, FHIR resolve, FHIR refresh, patient portal, and Postgres.
They carry no central-or-facility distinction, because a duty's job is the same whichever kind of server runs it and which kinds run which duties changes over time.
Whether a deployment is a central or a facility is a separate fact, answered once for the deployment rather than encoded into each duty's name.

The vocabulary is organised by product, so products whose duties have nothing in common never share a set of names: an mSupply deployment's duties do not map onto Tamanu's, and neither has to accommodate the other.
Tamanu is the only product the vocabulary covers, and it is shaped to admit others without that changing.

A service whose duty is outside the vocabulary is carried under its own name rather than dropped, so a deployment running something the vocabulary does not cover is still reported in full.
This is also how a deployment shape that should no longer exist stays visible: a substrate reports the service it found under the name it found it by, and a check that grades such a service as forbidden finds it there without the shared vocabulary having to carry a duty nothing should be running.

### Service facts

For a service, a substrate answers what version or image it is running, whether it is currently up, and its memory and CPU usage.
The service-expectation logic that decides what a deployment ought to be running grades against these readings, so a shortfall in running services is found the same way on every substrate.

## Resource usage per service

Each service's memory and CPU usage are reported as metrics wherever a substrate can read them, dimensioned by duty and service.
These are telemetry rather than a verdict: a deployment's resource usage is reported whether or not anything grades it.

A service is graded against a ceiling only where one is declared for it — a container memory limit, a supervised unit's configured maximum, or a Kubernetes container limit.
Where a service declares no ceiling there is no denominator to take a percentage of, so its usage is reported as a metric and the grading skips for that service.
Usage is never graded against the machine's total, because the machine's capacity is shared with everything else on it and says nothing about whether a service is near its own limit.

## HTTP traffic and certificates

A check asks the substrate for HTTP traffic statistics for its own workload rather than for a particular server's.
Where a deployment fronts its own traffic, this reads that front end's statistics for the whole machine.
Where traffic is served by shared infrastructure, this reads that infrastructure's statistics and filters them to the workload being reported for, so one reading serves each deployment behind it separately.

TLS certificates are likewise asked for as the certificates in force for the workload, whatever issues and serves them.

A check that grades the front-end software itself, rather than the traffic through it, reads it directly and skips when the substrate is not the system, on the same terms as the other system checks.
