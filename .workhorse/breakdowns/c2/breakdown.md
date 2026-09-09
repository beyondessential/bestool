# Audit database redesign follow-ups

Cards that fall out of the C2 design but are not part of delivering the new store itself.

## Publish audit chain heads to canopy from bestool-alertd

The new audit store hash-chains every record a session writes, which makes the log self-verifying but proves nothing against someone who can rewrite the directory. An off-box witness closes that gap. bestool-alertd already runs on Tamanu servers, already holds a device key, and already posts doctor results to canopy, so it is the natural place to periodically read the latest chain head of each session on the box through the bestool-psql read API and post them to canopy. A later verification compares the stored log against the witnessed heads to show it was not rewritten before each witness point. Needs a canopy endpoint to receive the heads and a decision on which users' state directories alertd reads.
