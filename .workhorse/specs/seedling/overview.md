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
A command presents the identity of the operator who invoked it wherever that operator has one, the same identity that operator already uses to reach the daemon.
That keeps a command's authority equal to the authority of the person running it, and keeps the daemon's record of who acted naming them.

An operator who has no identity of their own is not asked to obtain one.
A Seedling installation authorises one identity for us on the host it is installed on, at `/etc/bestool/seedling.key`, and a command falls back to that.
This is what lets the operational commands work on a freshly provisioned host, where nothing has been configured for any operator yet.
Acting under it, a command names the invoking operator to the daemon anyway, so the record of who acted is a person rather than the host.

The host identity is readable only by root, so using it is confined to privileged invocations: it grants no authority that root on that host does not already hold, since root can authorise any key it likes.
The daemon's own state, including the identity it publishes, is likewise root-only.
An unprivileged invocation that needs either of them runs itself again elevated rather than failing, so an operator does not have to discover that these commands need privileges on a Seedling host.
Where the operator's own identity is refused and the host carries one that would be accepted, the refusal says so, rather than leaving the operator to work out that elevating is what is missing.

An operator with no identity of their own, on a host with none either, is told how to obtain access rather than silently acting as no-one.

A command verifies the daemon it connects to against the identity the daemon publishes in its data directory for processes on the same host.
Because a command runs beside the daemon, it can read that identity directly and needs neither a first-connection prompt nor a store of previously seen daemons.

## Advertising the host identity

A host's Seedling identity is unreadable to an unprivileged process, so a co-located tool cannot tell by itself whether elevating would gain it a working one.
The local alert daemon's API answers that: its `/seedling` endpoint reports whether the host carries an identity, as `host_identity` on a JSON object.
It reports presence only and never discloses the identity, so asking is safe from anywhere the API is reachable.

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
