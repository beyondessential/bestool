# bestool-alertd

An alert daemon that watches a set of YAML alert definitions, runs them on a
schedule, and dispatches the results to one or more targets (email, HTTP
endpoints, etc.).

This crate is part of [BES tooling][repo], and is in particular what powers
the `tamanu alerts` workflow. It is published as both a library and a
standalone binary; the `bestool` umbrella tool also embeds it.

[repo]: https://github.com/beyondessential/bestool

## Install

```console
$ cargo install bestool-alertd
```

Pre-built binaries are attached to each `bestool-alertd-v*` GitHub release.

## Use

```console
$ bestool-alertd run \
    --database-url postgresql://localhost/mydb \
    --glob '/etc/myapp/alerts/**/*.yml'
```

Common flags:

- `--glob PATTERN` (repeatable): where to find alert definition files. Patterns
  may match a directory (read recursively) or individual files. Globs are
  watched for changes and re-evaluated periodically.
- `--database-url URL` / `DATABASE_URL`: PostgreSQL connection for SQL alerts.
- `--email-from`, `--mailgun-api-key`, `--mailgun-domain`: enable email
  targets via Mailgun.
- `--device-key-file PATH` / `DEVICE_KEY_FILE`: PEM identity used when posting
  to canopy `/events` targets.
- `--dry-run`: execute every alert once and exit; useful in CI.
- `--server-addr`: where the local HTTP control API listens
  (default `[::1]:8271` and `127.0.0.1:8271`).

`bestool-alertd` exposes additional subcommands that talk to a running daemon
via its HTTP API: `status`, `reload`, `loaded-alerts`, `pause-alert`,
`validate`. SIGHUP also triggers a reload on Unix.

On Windows, `bestool-alertd install` registers a native service named
`bestool-alertd`; `uninstall` and `configure-recovery` are also provided.

## Defining alerts and targets

- Alert files: see [ALERTS.md](./ALERTS.md).
- Target files: see [TARGETS.md](./TARGETS.md).

## Library

The crate also exposes a library API (`bestool_alertd::run`,
`DaemonConfig`, …) so other tools can embed the daemon without going through
the binary.

## License

GPL-3.0-or-later.
