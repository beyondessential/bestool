# Copied database check (TAM-7176)

Spec: [CHK-COPY](../specs/tamanu/copied-database.md).

Linux clones are scrubbed by ops (beyondessential/ops#341) and pgro upgrade replicas by beyondessential/postgres-restore-operator#165.
Windows clones have nothing automatic, and about half the clones in Canopy run Windows.
This check covers every platform and every way of copying, because it reads the result rather than hooking the restore.

## Next: clearing a copy

The check only reports today. Once a fleet run shows it fails on copies alone, add a self-heal that clears the copy, limited to machines Canopy ranks `clone`, so a production server with wrong Canopy records never clears its own identity.

- Canopy: tell an enrolled machine its rank. `GET /machines/self` returns ids and application types only; the rank is on the machine row.
- Clearing: stop the Tamanu services, run `forget_server_identity()`, on a central run Tamanu's upgrade (it recreates the settings key the function removes), start the services. alertd has no way yet to run Tamanu's upgrade on Windows.
- Read the rank at the time of the attempt, and set a long minimum interval.
- A Tamanu without the function is left alone.

## Open questions

- Is Tamanu's recorded server id (`metaServerId`) the machine id on every healthy server?
  A Tamanu whose status push Canopy rejects could hold anything, and the check would fail there. Look at every failure from the first fleet run before adding the heal.
- How are the Windows clones refreshed today: `bestool canopy restore`, a whole-machine Kopia restore, or by hand?
  A whole-machine restore brings the source's bestool registration across too. It is encrypted with a key derived from the machine id, so on a different machine it fails to decrypt and the check skips. A copied virtual machine keeps its machine id, so it reads, both ids are the source's, and the check passes. Worth confirming which happens.

## Also worth doing in Canopy

Canopy could flag a status push for a server that arrives from a machine the server is not attached to.
Tamanu signs that push with the device key from its database, which a copy shares with its source, so this only works if Canopy can tell the machine apart by another route, such as its tailnet identity. Unchecked.
It would need no client change and would cover machines without alertd, but it only alerts.
