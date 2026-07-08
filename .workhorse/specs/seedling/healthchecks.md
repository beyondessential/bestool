---
id: SDH
---

# Seedling healthchecks

On a host that runs the Seedling application orchestrator, `bestool tamanu doctor` gathers Seedling-specific health checks from the local Seedling daemon and folds them into the sweep alongside the host and Tamanu checks.
The Seedling checks share the registry, outcomes, grouping, rendering, and status wire format of every other check (see [DOC](../tamanu/doctor.md)); this spec covers only what is specific to Seedling: when they apply, where their data comes from, and what each reports.

## Seedling host context

The Seedling checks apply when the host runs Seedling: the daemon's data directory is configured in the environment and the daemon's control interface is present there.
When no Seedling is configured, every Seedling check skips, so the same doctor invocation runs safely on a host that carries none.

## Obtaining checks from the daemon

The doctor reads Seedling health from the daemon's local control interface, authenticating as a client the daemon trusts.
The daemon is the single source of truth for its own health: the doctor reports what the daemon returns rather than inspecting Seedling's containers or database itself.
When the daemon is reachable but cannot answer a given check, that check is reported as broken rather than failing, so a daemon-side fault is not mistaken for an unhealthy system.

## Checks

The doctor derives one check per Seedling subsystem the daemon reports on.

A reverse-proxy check reports whether the proxy that fronts application traffic is running, and fails when it is not.
A resolver check reports whether the Seedling DNS resolver is running, and fails when it is not.
An applications check reports the health of the Seedling-managed applications: it passes when every app is running (or none are deployed), and warns when some are not running.

Each check carries a one-line summary, and a reason whenever it does not pass, in line with [DOC](../tamanu/doctor.md).
