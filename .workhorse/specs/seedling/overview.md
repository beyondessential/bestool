---
id: SEED
---

# Seedling hosts

Some hosts run the Seedling application orchestrator, which owns the applications on that host and exposes an operator interface for observing and controlling them.
On such a host the Tamanu commands act through the Seedling daemon rather than through the host's service manager and container runtime.

This spec holds what every Seedling-aware command shares: how a host is recognised as a Seedling host, how a command authenticates to the daemon, and how a command chooses where to act.
Individual commands are specified alongside: the operational commands in [SHC](host-commands.md).

## Recognising a Seedling host

A host counts as a Seedling host when the Seedling daemon's data directory is configured in the environment.
That signal describes the host, so it is independent of whether the daemon is currently running or reachable.

## Acting through the operator's CLI

A command reaches the daemon by driving `seedling-ctl`, the Seedling operator CLI, rather than by speaking the operator interface itself.

The operator interface authenticates both ends by pinned public key, and the CLI already holds the operator's own identity and its store of known daemon identities.
Driving the CLI therefore carries the identity of the operator who invoked the command, and needs no separate identity of its own: an operator who can already operate the host can run these commands, and one who cannot is refused by the daemon rather than by us.
This also keeps a command's authority equal to the authority of the person running it, so a command cannot reach a daemon its operator could not reach directly.

## Choosing where to act

A command resolves the host into one of three states before it acts.

When no Seedling is configured on the host, the command acts through the host service manager and container runtime, so the same invocation keeps working on a host that carries no Seedling.

When Seedling is configured and its daemon answers, the command acts through the daemon.

When Seedling is configured but the daemon cannot be reached, the command reports why and does nothing.
This covers a daemon that is down, an operator the daemon refuses, and an operator CLI that is absent from the host.

It does not fall back to the host service manager in any of those states: on a Seedling host the services under the host manager are not the ones the operator means, so acting on them would report success while leaving the running system untouched.
A Seedling host that cannot currently be reached is still a Seedling host, so the answer is an error the operator can act on rather than a quiet change of target.

## Targeting an application

A command acts on the Tamanu application the daemon manages.
When the daemon manages no Tamanu application, the command reports that rather than acting on an unrelated one.
When it manages more than one, the command requires the operator to name which application to act on.
