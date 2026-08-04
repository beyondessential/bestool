---
id: LOG
---

# Tamanu log tailing

`bestool tamanu logs` prints recent log lines for a Tamanu deployment's services and the infrastructure they run behind, optionally following new lines as they arrive.
It resolves where each service's logs actually live, so an operator runs the same command on every host regardless of how the deployment is supervised.

On a Seedling host the command streams from the daemon instead, as described in [SHC](../seedling/host-commands.md).

## Selecting what to tail

Each name argument is matched as a substring against the deployment's expected-running services, so a partial name selects every service whose name contains it.
Multiple names combine, selecting the union of what they match.
A name matching no expected-running service is an error that reports the available names, rather than a silent empty tail.

With no names at all, the command selects every expected-running Tamanu service together with caddy.

Two names select infrastructure rather than a Tamanu service.
`caddy` selects the reverse proxy.
`postgres` selects the database, and is also accepted spelled `postgresql`, `postgre`, `pg`, `psql`, or `pgsql`, in any case.
A name that selects infrastructure and nothing else leaves the Tamanu services unselected, so naming only `caddy` tails only caddy.

## Log sources

Caddy is read from two places at once, because its output is split across them.
Its runtime events — configuration reloads, certificate renewals, upstream failures — are read from the system journal for the `caddy.service` unit.
Its access entries are read from the `*.log` files in `/var/log/caddy`.
Both are tailed together, so an operator sees request traffic alongside the events that explain it.

Where a deployment's proxy writes access entries to its standard output, they appear in the journal and no such files exist; the command reads whatever of the two is present.
The presence of the directory alone does not imply access entries are written to it.

On Windows, caddy is read from the `.log` files under `C:\Caddy\logs`, or under `C:\Caddy` when the former holds none.

Postgres is read from the system journal for units matching `postgresql*` together with the `*.log` files in `/var/log/postgresql`.
On Windows it is read from the `log` or `pg_log` directory within the Postgres data directory.

Tamanu's own services are read from the system journal for their units, or from the supervising process manager's log files where a process manager runs them.

The file read in each location is the live one; files that have already been rotated away, compressed or not, are never opened.

Reading these sources requires privileges an operator may not hold, and a source that cannot be read yields no entries rather than an error.
The command therefore acquires the privileges it needs before reading, so that an unreadable source cannot be mistaken for a quiet one.

## Output

Lines from several sources are interleaved in order of arrival.
A line read from a log file is prefixed with its containing directory and file name so it stays attributable; a line read from the journal carries no prefix.

### Timestamps

A line that is a JSON object whose `ts` field is a number holding a Unix epoch timestamp in seconds is rewritten to carry that instant as an RFC 3339 timestamp in UTC with microsecond precision.
Every other field, and the order of all fields, is preserved.

The rewrite is unconditional and has no opt-out.
It applies only to numbers within the range of plausible epoch timestamps, so a `ts` field carrying an unrelated number is left alone, as is one already carrying a formatted timestamp.

## Options

The trailing line count applies per source, so a tail spanning several sources prints that many lines from each of them.

A regular expression filter restricts output to matching lines, and inverts to restrict it to non-matching lines instead.

Following keeps printing lines as they arrive, and survives the file it is following being replaced underneath it.
A file rotated away and succeeded by a fresh one at the same path continues to be followed there: the entries written to it before it was rotated away are printed first, and following then continues from its replacement.
This holds however the rotation is performed, whether the file is replaced by a new one or emptied in place.
