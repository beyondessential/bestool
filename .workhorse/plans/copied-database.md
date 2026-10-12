# Copied database check (TAM-7176)

Spec: [CHK-CPY](../specs/tamanu/copied-database.md).

Linux clones are scrubbed by ops (beyondessential/ops#341) and pgro upgrade replicas by beyondessential/postgres-restore-operator#165.
Windows clones have nothing automatic, and about half the clones in Canopy run Windows.
This check covers every platform and every way of copying, because it reads the result rather than hooking the restore.

## Depends on Canopy

The device cannot read either fact it needs today.

- `GET /servers/self` returns one server id, and cannot answer for a machine running more than one application.
- `GET /machines/self` names its applications by type only, with no server ids.
- Neither returns a rank.

Canopy needs to tell an enrolled machine, for each server attached to it: the server id, the application type, and the rank.

## Open questions

- Are Tamanu's recorded server id (`metaServerId`, set when Tamanu first POSTs `/servers`) and the server Canopy attaches to the machine always the same id on a healthy server?
  If any healthy server has two, the check fails there. Before enabling the heal, run the check report-only across the fleet and look at every failure.
- How are the Windows clones refreshed today: `bestool canopy restore`, a whole-machine Kopia restore, or by hand?
  A whole-machine restore would also bring the source's bestool registration across; the check then compares two copied ids and passes. Worth confirming which.
- A Tamanu deployment that sets the global `metaServer.serverId` setting keeps reporting as that id after the function runs, because the function leaves settings that are not secret alone.
  Check whether any deployment sets it.

## Also worth doing in Canopy

Canopy could flag a status push for a server that arrives from a machine the server is not attached to.
Tamanu signs that push with the device key from its database, which a copy shares with its source, so this only works if Canopy can tell the machine apart by another route, such as its tailnet identity. Unchecked.
It would need no client change and would cover machines without alertd, but it only alerts.
