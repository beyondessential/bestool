---
id: SUBJ
---

# Machines and applications

A healthcheck, and every fact reported alongside it, is about one **subject**: the machine a deployment runs on, or one application running on it.
This spec describes what those subjects are, how each is identified, and which checks and facts belong to each.
See [CHK](healthchecks.md) for the checks themselves, [SUB](substrate.md) for how a check obtains its readings for a subject, and [DOC](doctor.md) for the sweep that runs them.

A **machine** is a host: its filesystems, its clock, its memory and processors, its network identity.
An **application** is one product installed on a machine: its own services, its own version, its own database, its own HTTP traffic, its own certificates, and its own names on the network.

A machine hosts zero or more applications, and the two are reported separately rather than one standing in for the other.
A machine commonly hosts a Tamanu facility application alongside an mSupply application, which share nothing but the machine.
The same holds for two applications of one product, so a machine running both a central and a facility hosts two applications rather than one server of an ambiguous kind.

The Postgres installation a deployment runs on is an application in its own right, not a part of whatever uses it.
Whether a cluster is tuned, whether its pages carry checksums, what version it runs and whether it answers at all are questions about the server, and they hold whether one application uses it, several do, or none does.
A machine running a Tamanu on a local Postgres therefore reports two applications, and the health of the database is not confused with the health of what reads from it.

An application carries a **type**: the software it is and the role it plays, together, as a slug such as `tamanu-central`.
Which duties it can have, which facts describe it, and which checks apply to it all follow from its type.

## Identifying a subject

A machine is identified by the identity its agent enrolled with, which the agent mints once and keeps.
That identity belongs to the machine, so a machine hosting several applications has one of them rather than one per application.

An application is identified by its type together with a key the reporter chooses.
The key separates an application from the others on its own machine and carries no meaning beyond it, so an application is correlated from its machine and its key together rather than from the key alone.
Two applications on different machines may share a key without being confused for one another.

An agent installed on a machine uses a fixed key for each application it reports there.
A process driving sweeps for applications it observes from elsewhere supplies each key itself, because it is what knows how to tell those applications apart.

A Postgres cluster is identified by the port it answers on, never by its version.
A cluster upgraded in place keeps its port while its version changes, and reporting it under a version-bearing key would say that one application had stopped and another started when nothing had moved.
The port is also the one identifier every connection form carries, a Unix socket being named for the port it serves, so a cluster reached over TCP and the same cluster reached over its socket are recognised as one.
A cluster reached at an address that is not the reporting machine's is keyed apart from a local one, so a key never claims a machine hosts something it does not.

A key is stable across pushes: it names the same application every time the reporter pushes.
A key that appears under a different type reports that one application has stopped being reported and another has started, so a key is not reused for an application of another type.

Nothing mints an identifier per application, and a sweep never creates an identity for a subject it does not own.

## Splitting the catalogue

Every check in the catalogue reports for either a machine or an application, and its subject determines what it may read.

Machine checks are: filesystem capacity, inodes, btrfs device statistics, held filesystem captures, clock synchronisation, whole-machine memory, whole-machine load, machine uptime, unexpected local user accounts, the machine's addresses, whether munin-node is installed, the machine's billing tags, Tailscale presence and configuration, bestool's own Canopy enrolment, and the checks that grade the HTTP front-end software itself — its version, its resolvers, and the version marker in its configuration file.

Held captures are a machine concern because they are filesystem-level snapshots, and because what is captured is not confined to any one application's database.

Billing tags are read against the machine, not against any application on it, so a machine hosting several applications carries one set of tags rather than one per application.

Application checks are: everything that reads the application's own data, its HTTP reachability and error rates, its service inventory and version drift, its certificates, and its resource usage per service.

The checks that grade the database server itself — whether it is reachable, what version it runs, how it is tuned, and whether its pages carry checksums — report for the Postgres application rather than for whatever uses it.
A check reading an application's tables is about that application; a check grading the cluster is about the cluster.

A concern that genuinely exists on both sides is two checks rather than one check with a conditional subject, so neither has a mode in which it reports the wrong subject's reading.

Which checks apply to an application follows from its type, so a check written for one type is not run against another.
A check that does not apply to a subject is absent from that subject's report rather than reported for it as skipped.
Skipped is reserved for a check that does apply to the subject but could not be determined on this sweep, so the two are not confused for one another.

A check is named within its subject, so one name may belong to a machine check and to an application check without the two being the same check.
A name identifies a check only together with the subject it reports for.

## Reported facts

A sweep reports each subject the same way: that subject's checks, and its detail.
A machine and each application on it are described alike, so one shape serves both grains and a reader does not have to unpick which subject a check or a fact was really about.

A report names the agent that produced it, so several agents reporting on one machine are told apart and each accounts only for the checks it files.
The alertd daemon's sweep reports under the name `alertd`.

The facts reported alongside the checks split by subject on the same terms, so no fact is reported against a subject it is not about.

A machine reports: its hostname, its uptime, its operating system kind, name, version and kernel, its architecture, whether it is virtualised and by what, its processor count, its total memory, its filesystems, its addresses and its IPv4, IPv6 and NAT64 reachability, its clock timezone, its billing tags, and the version of bestool running on it.

An application reports: its product version, its type, its install root where it has one on disk, the version of the runtime it executes under, its canonical URL, its current sync tick, and its configured timezone.

A Postgres application reports its server version.
That version belongs to the server rather than to what connects to it, so an application using a database does not report the database's version as one of its own facts.

Because each subject reports only its own facts, a fact is absent when the subject genuinely lacks it rather than when the reading could not be attributed.
The version of bestool is a machine fact and an application has none, which answers correctly for an application no agent is installed alongside: there is no agent there to upgrade.

The machine's clock timezone and an application's configured timezone are separate facts reported against separate subjects.
Drift between them is still graded wherever one sweep holds both — a machine running an application it also reports for — and neither subject reports the other's zone as its own in order to make that comparison possible.
