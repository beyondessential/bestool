---
id: CHK-COPY
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

The database's server id is the Canopy server id Tamanu records in the application's database, which Tamanu reports its status against.
The machine's server id is the one in its Canopy registration ([CHK-REG](../canopy/registration.md)).

> [!NOTE]
> Canopy accepts a status push only against the machine the pushing device is enrolled for, so on a machine whose Tamanu reports to Canopy the two ids are the same.
> The database's id travels with the database, so a copy holds its source's, while the registration stays with the machine.

A database whose server id differs from the machine's was copied from another server.
A database with no server id has none to compare.

> [!NOTE]
> A database with no server id is a fresh install, or a copy whose identity is already cleared.

## Outcomes

- passes when the database has no server id, or its server id is the machine's, compared without regard to case.
- fails when the database's server id differs from the machine's. The reason says the database was copied from another server and directs the operator to Tamanu's restore-as-copy procedure, and the detail carries both server ids.
- skips when the machine's registration cannot be read or holds no server id, or when the database cannot be reached.

The check does not clear a copy itself.
