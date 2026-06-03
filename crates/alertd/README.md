# bestool-alertd

A healthcheck daemon: it runs background tasks (the Tamanu doctor sweep) on a
schedule, posts the results to canopy, and serves task/status/health/metrics
over a small HTTP API.

This crate is part of [BES tooling][repo]. It is a library embedded by the
`bestool` umbrella tool, which drives it via `bestool tamanu alertd`.

[repo]: https://github.com/beyondessential/bestool

## Use

The daemon is configured and run through `bestool tamanu alertd run`, which
reads the database and device-key configuration from Tamanu's config files,
registers the doctor sweep, and starts the daemon.

The HTTP control API exposes:

- `GET /` — list of endpoints.
- `GET /status` — daemon name, version, uptime, pid.
- `GET /health` — watchdog health (200 if healthy, 530 if stalled).
- `GET /metrics` — Prometheus metrics.
- `GET /tasks/{task}/{endpoint}` — endpoints exposed by registered tasks (e.g.
  the doctor's `latest` and `recompute`).

On Windows, `bestool tamanu alertd install` registers a native service named
`bestool-alertd`; `uninstall` and `configure-recovery` are also provided.

## Library

The crate exposes a library API (`bestool_alertd::run`, `DaemonConfig`,
`BackgroundTask`, the `doctor` module, …) so other tools can embed the daemon.

## License

GPL-3.0-or-later.
