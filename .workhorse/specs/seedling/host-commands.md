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

The stream follows the same output shape and the same follow, filtering, and range behaviour the command offers on a host without Seedling, so an operator's habits and any scripts around it carry across unchanged.

## Interactive database access

The interactive database command opens a shell session through the daemon into the Tamanu application, and connects to the database from inside that session using the connection details the application itself holds.
Running inside the application means the session reaches the database over the same path the application uses, so the command needs no database credentials of its own and the database is not exposed beyond the host.

The session is interactive: input, output, and terminal resizing are carried for the life of the session, and the session ends when the operator exits.
The command surfaces the session's exit status as its own.
