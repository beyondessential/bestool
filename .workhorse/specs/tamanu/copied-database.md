---
id: CHK-CPY
---

# Copied database

The `copied_database` healthcheck grades whether a Tamanu application's database was copied from another server and still carries that server's identity.
It is one of the doctor's healthchecks; see [CHK](healthchecks.md) for the framework it runs in.

> [!NOTE]
> A Tamanu database restored onto another machine keeps the identity of the server it was taken from, whatever made the copy: a clone refresh, a whole-machine restore, a copied virtual machine, a dump.
> That identity is its Canopy server id, the device key it signs with, its sync credentials and its integration credentials.
> Until it is cleared, the copy reports to Canopy as its source, and can sync, report and send messages as it.
> Tamanu clears it with its `forget_server_identity()` database function, which the Linux clone restore runs; on other platforms nothing runs it unless an operator does.

## What it compares

The check reports for each Tamanu application on the machine ([SUBJ](subjects.md)).

The database's server id is the Canopy server id Tamanu records in the application's database when it first reports to Canopy.

The application's server id is the one Canopy holds for the Tamanu server attached to this machine with the application's type.
It is resolved from Canopy and never from the database.

> [!NOTE]
> The database's server id travels with the database, so a copy holds its source's, while a restore cannot carry the application's server id across.

A database whose server id differs from the application's was copied from another server.
A database with no server id has none to compare.

> [!NOTE]
> A database with no server id is a fresh install, or a copy whose identity is already cleared.

## Outcomes

- passes when the database has no server id, or its server id is the application's.
- fails when the database's server id differs from the application's. The reason says the database was copied from another server, and the detail carries both server ids.
- skips when the database cannot be reached, when Canopy cannot be reached, or when Canopy attaches no Tamanu server of the application's type to this machine.

## Clearing a copy

When the check fails, the daemon clears the copied identity as a self-heal action ([CHK](healthchecks.md#self-healing)), but only when Canopy ranks the application's server as `clone`.
On any other rank the check stays failed and nothing is cleared.
The rank is read from Canopy at the time of the attempt, not carried from an earlier sweep.

> [!NOTE]
> Clearing is limited to clones because a production server whose Canopy records are wrong would otherwise clear its own identity.

Clearing stops the application's Tamanu services, runs `forget_server_identity()` against its database, and starts the services again.
On a central server it runs Tamanu's upgrade before starting the services, because the function removes the settings key and the upgrade recreates it.

> [!NOTE]
> A cleared facility server comes up with no central to sync with, which is the expected state of a cleared copy: an operator sets it up against the clone's central.

A database whose Tamanu version does not provide `forget_server_identity()` is not cleared, and the check's reason directs the operator to Tamanu's restore-as-copy procedure.

Clearing sets a long minimum interval between attempts.
