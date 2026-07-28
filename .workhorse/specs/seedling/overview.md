---
id: SEED
---

# Seedling hosts

Some hosts run the Seedling application orchestrator, which owns the applications on that host and exposes an operator interface for observing and controlling them.
On such a host the Tamanu commands act through the Seedling daemon rather than through the host's service manager and container runtime.

This spec holds what every Seedling-aware command shares: how a host is recognised as a Seedling host, how a command reaches the daemon, and how a command chooses where to act.
Individual commands are specified alongside: the operational commands in [SHC](host-commands.md).

## Recognising a Seedling host

A host counts as a Seedling host when the Seedling daemon is installed on it as a service.
Installing Seedling registers the daemon as a service and keeps its state in a fixed directory, so the presence of that service is a property of the host rather than of the daemon's current condition: a host whose daemon is stopped, broken, or mid-upgrade is still a Seedling host.
Recognition therefore never depends on reaching the daemon, which is what lets an unreachable daemon be reported as a fault instead of being mistaken for a host that runs no Seedling.

## Speaking the operator interface

A command reaches the daemon over its operator interface, rather than by driving the operator CLI.
The interface is a versioned contract with typed requests and responses; a CLI's arguments and rendered output are a human-facing surface that can be rearranged without notice, so depending on them would make a command's correctness rest on how another tool chooses to print things.

Both ends authenticate by public key.
A command presents the identity of the operator who invoked it, the same identity that operator already uses to reach the daemon, so it needs no identity of its own and nothing has to be authorised for it: an operator who can already operate the host can run these commands, and one who cannot is refused by the daemon rather than by us.
That keeps a command's authority equal to the authority of the person running it, so a command cannot reach a daemon its operator could not reach directly.

A command verifies the daemon it connects to against the identity the daemon publishes in its data directory for processes on the same host.
Because a command runs beside the daemon, it can read that identity directly and needs neither a first-connection prompt nor a store of previously seen daemons.

## Choosing where to act

A command resolves the host into one of three states before it acts.

When no Seedling is configured on the host, the command acts through the host service manager and container runtime, so the same invocation keeps working on a host that carries no Seedling.

When Seedling is configured and its daemon answers, the command acts through the daemon.

When Seedling is configured but the daemon cannot be reached, the command reports why and does nothing.
This covers a daemon that is down, one whose published identity is unreadable, and an operator whose identity the daemon refuses.

It does not fall back to the host service manager in any of those states: on a Seedling host the services under the host manager are not the ones the operator means, so acting on them would report success while leaving the running system untouched.
A Seedling host that cannot currently be reached is still a Seedling host, so the answer is an error the operator can act on rather than a quiet change of target.

## Targeting an application

A command acts on the Tamanu application the daemon manages.
When the daemon manages no Tamanu application, the command reports that rather than acting on an unrelated one.
When it manages more than one, the command requires the operator to name which application to act on.
