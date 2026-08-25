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

## What a substrate answers for

A substrate is described by what a check can ask it about, grouped by subject.

The **system**: whether the checking process is running on the machine it reports for.
This is the only thing checks in this group need from a substrate, because they read the machine directly rather than through it.

The **workload**: which services make up this deployment, and the facts that hang off each running instance of them.

**Tamanu**: the deployment's configuration, its version, and a connection to its application database.

**A database**: either a means of establishing a connection, or an established connection to run queries against.

## Checks that read the system directly

Some checks are about the machine and nothing else: filesystem capacity and inodes, btrfs device statistics, clock synchronisation, Tailscale presence and configuration, unexpected local user accounts, held captures, whether munin-node is installed, the host's addresses, whole-system memory and load, and host uptime.

These read the machine directly rather than through any abstraction, because there is no useful reading to abstract: the concerns they grade are properties of a machine, and a deployment observed from elsewhere has no machine of its own to grade.
Each skips when the substrate reports that the checking process is not the system it is reporting for, with that as the stated reason.

Whole-system memory and load stay in this group.
They measure the machine rather than the deployment, which is a signal worth keeping wherever a server is monolithic and bestool runs on it directly, and one that has no meaning at all when it is not.

## The workload

A deployment is a set of **duties**, each with zero or more running **instances**.
This holds on every substrate: a Linux server runs Tamanu as separate containers and cgroup-confined units, a Windows server runs it as separate supervised processes, and a Kubernetes deployment runs it as separate pods.
An instance is one container, one supervised process, or one pod, and a duty may have several — an API duty commonly runs more than one, and a Kubernetes Postgres duty runs a primary alongside a replica where a Linux or Windows one runs a single instance.

Duties are drawn from a shared vocabulary so that every substrate names the same duty the same way: central API, facility API, central tasks, facility tasks, facility sync, frontend, central FHIR resolve, central FHIR refresh, patient portal, Postgres, and mSupply.
A duty a substrate reports that is outside the vocabulary is carried under its own name rather than dropped, so a deployment running something the vocabulary does not yet cover is still reported in full.
The vocabulary grows and shrinks as recognised duties change, and a substrate names duties from it rather than from the naming scheme of the supervisor underneath — a check reads a duty, never a unit name or a process name.

A substrate answers which duties exist, how many instances each has, and what each instance is called.
For each instance it answers what version or image it is running, and whether it is currently up.
The service-expectation logic that decides what a deployment of a given kind ought to be running grades against these readings, so a shortfall in running instances is found the same way on every substrate.

## Resource usage per instance

Each instance's memory and CPU usage are reported as metrics on every substrate that can read them, dimensioned by duty and instance.
These are telemetry rather than a verdict: a deployment's resource usage is reported whether or not anything grades it.

An instance is graded against a ceiling only where one is declared for it — a container memory limit, a supervised unit's configured maximum, or a Kubernetes container limit.
Where an instance declares no ceiling there is no denominator to take a percentage of, so its usage is reported as a metric and the grading skips for that instance.
Usage is never graded against the machine's total, because the machine's capacity is shared with everything else on it and says nothing about whether this instance is near its own limit.

## HTTP traffic and certificates

A check asks the substrate for HTTP traffic statistics for its own workload rather than for a particular server's.
Where a deployment fronts its own traffic, this reads that front end's statistics for the whole machine.
Where traffic is served by shared infrastructure, this reads that infrastructure's statistics and filters them to the workload being reported for, so one reading serves each deployment behind it separately.

TLS certificates are likewise asked for as the certificates in force for the workload, whatever issues and serves them.

A check that grades the front-end software itself, rather than the traffic through it, reads it directly and skips when the substrate is not the system, on the same terms as the other system checks.
