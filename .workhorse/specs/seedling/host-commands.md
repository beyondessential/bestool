---
id: SHC
---

# Tamanu commands on a Seedling host

On a Seedling host, `bestool tamanu start`, `stop`, `restart`, `status`, `logs`, and `psql` act through the Seedling daemon.
An operator runs the same commands with the same arguments on every host; the command discovers where Tamanu actually lives and acts there.

Host recognition, how a command reaches the daemon, path selection, and application targeting are shared by every Seedling-aware command and specified in [SEED](overview.md).
This spec covers what each of these commands does once it has resolved to the Seedling path.
`bestool tamanu doctor` gathers Seedling health checks through the same daemon, covered separately.

## Lifecycle

Starting Tamanu brings the application out of the stopped state through the daemon, and stopping it returns the application to that state.

Restarting rolls the application's deployments through the daemon, following the update strategy each deployment declares, rather than stopping the application and starting it again.
A deployment that can keep an instance serving while another is replaced does so, matching the rolling behaviour an operator gets on a host without Seedling; stopping and restarting would drop every instance at once and lose that property.

A lifecycle command reports what the daemon reports: the state the application reached, and the reason when the daemon declines or fails to reach it.
When the application is already in the requested state, the command succeeds without change rather than treating it as an error.

## Status

The status command reports the application state the daemon holds, so an operator sees the state of the runtime the lifecycle commands act on rather than the state of unrelated host services.
It reports the state of each of the application's constituent parts where the daemon distinguishes them.

## Logs

The log command streams Tamanu's logs from the daemon's log stream.
The daemon is the source of the log data, so the command does not read container or journal files itself.

Following, pattern filtering, and the count of trailing lines behave as they do on a host without Seedling, so an operator's habits and any scripts around them carry across.
The daemon composes each entry it streams from the entry's time, its unit, and the message, and those lines reach the operator as the daemon rendered them.
Pattern filtering is applied to the stream as it arrives, because the daemon matches no pattern of its own.

## Interactive database access

The interactive database command keeps its own client, and with it the read-only session, redaction, audit trail, and supervision prompt before writes that it applies on every other host.
Handing the operator a shell into the database instead would replace all four with a bare client, on precisely the hosts where they matter most.

Postgres runs as its own application and exports the directory holding its unix socket, so the command connects over that socket rather than over the network.
A local socket connection is trusted, which is what lets the command work without holding the database's password, and the database never needs to be reachable beyond the host.

Every part of that connection is discovered from the daemon: the socket's location from the exported volumes it reports, and the database and role from the Tamanu application's own parameters.
Nothing is inferred from where the daemon happens to keep its files.
When no application exports a socket, the command says so rather than falling back to a route that would need a password it does not have.
