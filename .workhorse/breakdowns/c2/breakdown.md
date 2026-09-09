# Audit database redesign follow-ups

Cards that fall out of the C2 design but are not part of delivering the new store itself.

## Publish audit chain heads to canopy from bestool-alertd · F2

The new audit store hash-chains every record a session writes, which makes the log self-verifying but proves nothing against someone who can rewrite the directory. An off-box witness closes that gap. bestool-alertd already runs on Tamanu servers, already holds a device key, and already posts doctor results to canopy, so it is the natural place to periodically read the latest chain head of each session on the box through the bestool-psql read API and post them to canopy. A later verification compares the stored log against the witnessed heads to show it was not rewritten before each witness point. Needs a canopy endpoint to receive the heads and a decision on which users' state directories alertd reads.

## Warn when the audit log is on a network filesystem · G2

The store must be on local storage: a network or synchronised filesystem gives no useful advisory locking, so the guarantee that only one process ever writes a segment stops holding, and a synchronised directory can duplicate or resurrect files under the reader. [AUD](../../specs/psql/audit/overview.md) says the session warns loudly at startup when it finds itself on one and continues anyway, since a degraded log is better than none. The store itself works there today; only the warning is missing. Detecting it needs the filesystem type of the store directory per platform: statfs on Linux and macOS, drive type on Windows. The warning is advisory only and never blocks a session.

## Stream compaction's fold instead of buffering a whole day · D2

Compaction reads a whole day's records into memory before writing anything, holding each one twice — parsed and as its full JSON text — plus a hash of every record purely to dedup. The sources are each already ordered, so a k-way merge would stream into the zstd encoder with a bounded window, and dedup can key on session and sequence number as [AUD-RET](../../specs/psql/audit/retention.md) already specifies. Correct as it stands and bounded by one day rather than by the size of the log, so this is a scaling improvement rather than a fix.
