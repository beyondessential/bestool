# bestool-alertd

The Tamanu healthchecks: the check registry, the sweep that runs them, the
self-heal machinery, and the stats each check declares.

This crate is part of [BES tooling][repo]. It carries the checks and the
machinery they're built from, and nothing that schedules them — so a consumer
can harvest checks without pulling in an HTTP server or a service registration.
The daemon that runs them on a schedule, and the `bestool tamanu doctor` command
that runs them interactively, both live in the `bestool` binary.

[repo]: https://github.com/beyondessential/bestool

## Use

`checks::all()` is the registry. `perform_sweep` runs a selection of it against
a host and its Tamanu deployment, resolving each check to an outcome and
reporting progress as it goes. `resolve_sweep_tamanu` and
`discover_sweep_tamanu` work out what deployment, if any, is on the host.

A check may declare a self-heal action, which the caller attempts while the
check is failing; the interactive doctor never does, so running it by hand has
no side effects.

## License

GPL-3.0-or-later.
