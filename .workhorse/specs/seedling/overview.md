---
id: SEED
---

# Seedling hosts

Some hosts run the Seedling application orchestrator, which owns the applications on that host and exposes an operator interface for observing and controlling them.
On such a host the Tamanu commands act through the Seedling daemon rather than through the host's service manager and container runtime.

This spec holds what every Seedling-aware command shares: how a host is recognised as a Seedling host, how a command authenticates to the daemon, and how a command chooses where to act.
Individual commands are specified alongside: the operational commands in [SHC](host-commands.md).

## Recognising a Seedling host

A host counts as a Seedling host when the daemon's data directory is configured in the environment and the daemon's published operator-interface identity is present there.
The daemon writes its own interface identity into its data directory at startup, so a co-located command establishes which daemon it is talking to by reading that file rather than by probing the daemon or prompting the operator.

## Authenticating to the daemon

A command authenticates to the daemon as a client the daemon has authorised.
The daemon holds a set of authorised client identities, and admits an entry added to the authorisation file in its data directory.
Because the commands run on the same host as the daemon, an operator with write access to the data directory can authorise the tooling without needing a prior authenticated session.

A command never authorises itself.
When the daemon has not authorised it, the command reports that it is unauthorised and identifies the client identity an operator needs to authorise, rather than granting itself access to the daemon it is asking to control.

## Choosing where to act

A command resolves the host into one of three states before it acts.

When no Seedling is configured on the host, the command acts through the host service manager and container runtime, so the same invocation keeps working on a host that carries no Seedling.

When Seedling is configured and its daemon answers, the command acts through the daemon.

When Seedling is configured and its daemon cannot be reached, or has not authorised this client, the command reports why it cannot reach the daemon and does nothing.
It does not fall back to the host service manager in this state: on a Seedling host the services under the host manager are not the ones the operator means, so acting on them would report success while leaving the running system untouched.

## Targeting an application

A command acts on the Tamanu application the daemon manages.
When the daemon manages no Tamanu application, the command reports that rather than acting on an unrelated one.
When it manages more than one, the command requires the operator to name which application to act on.
