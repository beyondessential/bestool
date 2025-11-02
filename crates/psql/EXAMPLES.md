# Examples

## Output

```sql
-- Normal output
database=> select * from settings where key like 'fhir.%';
      id      ┆  created_at  ┆  updated_at ┆ deleted_at ┆     key     ┆    value    ┆ facility_id ┆  scope ┆ updated_at_
              ┆              ┆             ┆            ┆             ┆             ┆             ┆        ┆  sync_tick
══════════════╪══════════════╪═════════════╪════════════╪═════════════╪═════════════╪═════════════╪════════╪═════════════
 37fa67a4-0c4 ┆ 2025-09-16T0 ┆ 2025-09-16T ┆ NULL       ┆ fhir.worker ┆ "1 minute"  ┆ NULL        ┆ global ┆ 0
 c-4cfe-9afe- ┆ 6:33:09.865Z ┆ 06:33:09.86 ┆            ┆ .heartbeat  ┆             ┆             ┆        ┆
 0c32046b6658 ┆              ┆ 5Z          ┆            ┆             ┆             ┆             ┆        ┆
╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌
 30410735-12f ┆ 2025-09-16T0 ┆ 2025-09-16T ┆ NULL       ┆ fhir.worker ┆ "10         ┆ NULL        ┆ global ┆ 0
 3-4733-a39f- ┆ 6:33:09.865Z ┆ 06:33:09.86 ┆            ┆ .assumeDrop ┆ minutes"    ┆             ┆        ┆
 dce7ea1b3635 ┆              ┆ 5Z          ┆            ┆ pedAfter    ┆             ┆             ┆        ┆
(2 rows, took 56.381ms)
-- all queries will print how long they took and a total count

-- \g alone is equivalent to ;
database=> select * from settings where key like 'fhir.%' \g
-- (output same as above)

-- Expanded output
database=> select * from settings where key like 'fhir.%' limit 1 \gx
-[ RECORD 1 ]-
 id                   ┆ 37fa67a4-0c4c-4cfe-9afe-0c32046b6658
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 created_at           ┆ 2025-09-16T06:33:09.865Z
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 updated_at           ┆ 2025-09-16T06:33:09.865Z
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 deleted_at           ┆ NULL
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 key                  ┆ fhir.worker.heartbeat
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 value                ┆ "1 minute"
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 facility_id          ┆ NULL
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 scope                ┆ global
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 updated_at_sync_tick ┆ 0
(1 row, took 57.375ms)
-- compared to native psql, you will notice that nulls are explicit here

-- JSON output prints one object per row
database=> select id, key, facility_id from settings where key like 'fhir.%' \gj
{"id":"37fa67a4-0c4c-4cfe-9afe-0c32046b6658","key":"fhir.worker.heartbeat","facility_id":null}
{"id":"30410735-12f3-4733-a39f-dce7ea1b3635","key":"fhir.worker.assumeDroppedAfter","facility_id":null}
(2 rows, took 57.717ms)

-- Expanded JSON output pretty-prints an array of objects
database=> select id, created_at, key, value from settings where key like 'fhir.%' \gjx
[
  {
    "id": "37fa67a4-0c4c-4cfe-9afe-0c32046b6658",
    "created_at": "2025-09-16T06:33:09.865Z",
    "key": "fhir.worker.heartbeat",
    "value": "1 minute"
  },
  {
    "id": "30410735-12f3-4733-a39f-dce7ea1b3635",
    "created_at": "2025-09-16T06:33:09.865Z",
    "key": "fhir.worker.assumeDroppedAfter",
    "value": "10 minutes"
  }
]
(2 rows, took 56.093ms)

-- Zero output (useful with \re)
database=> select * from patients \gz
(49 rows, took 56.927ms)

-- Output to file in one-object per line JSON format
database=> select id, created_at, key, value from settings where key like 'fhir.%' \gjo file.json
(2 rows, took 56.619ms)
-- note how this still prints the count and time taken here, and not in your file
```

## Write Mode

```sql
-- By default, the session is in read-only mode. SELECTs will work:
database=> select id, display_name, role from users limit 1;
                  id                  ┆ display_name ┆  role
══════════════════════════════════════╪══════════════╪════════
 00000000-0000-0000-0000-000000000000 ┆ System       ┆ system
(1 row, took 56.338ms)

-- But INSERTS and other write operations will be refused:
database=> insert into users (email, role) values ('admin@bes.au', 'admin');
2025-11-01T01:35:43.708916Z ERROR bestool_psql::repl::execute:   db error
  ╰─▶ ERROR: cannot execute INSERT in a read-only transaction

-- You instead need to switch into write mode, either
-- by providing --write/-W on the CLI, or interactively:
database=> \W
OTS?
-- At this point, it will prompt you for an "OTS", and refuse to proceed without.
-- You can hit "up arrow" to go through the OTS history, or type your own.
-- This OTS ("Over The Shoulder") will be recorded in the audit log for every
-- query done in this write mode session; it's meant to record who is supervising
-- you as you're working in a database potentially destructively.

-- Let's provide "demo" for now:
database=> \W
OTS? demo
AUTOCOMMIT IS OFF -- REMEMBER TO `COMMIT;` YOUR WRITES
database=>
-- In the actual output, the prompt will now be bold green. Legend:
-- normal white -- read-only mode
-- bold green   -- write mode, idle transaction (initially or after committing)
-- bold blue    -- write mode, active transaction (queries have been executed)
-- bold red     -- write mode, failed transaction (you should issue a ROLLBACK)

database=> insert into users (email, role, display_name) values ('admin@bes.au', 'admin', 'Admin');
(no rows)
database=*>
-- Along with being bold blue, in an active transaction the prompt will have a *

database=*> commit;
(no rows)
database=>
-- COMMIT or ROLLBACK to return to a bold green idle state
-- note that a new idle transaction has been automatically opened

-- Once you're done writing, return to read-only mode
-- This will refuse when you're in an active transaction to avoid losing work
-- you should either continue your work, or COMMIT/ROLLBACK to allow the action
-- Similarly, exiting will also be refused while in an active transaction.
database=> \W
SESSION IS NOW READ ONLY
```

## Snippets

```sql
-- Run a query
database=> select id, created_at, key, value from settings where key like 'fhir.%';
-- (output omitted)

-- Save the last query (so the one above here) to a snippet
database=> \snip save fhir_settings
Snippet saved to /home/.local/share/snippets/fhir_settings.sql
-- snippets are saved as SQL files but contain the exact query run,
-- so if you use query modifiers, they will be included too

-- Run a snippet
database=> \run fhir_settings
(2 rows, took 52.163ms)

-- In your history (and in the audit log) you will now have two things:
-- 1. The snippet invocation
-- 2. The actual snippet contents
-- This means you can hit "up arrow" to edit the snippet contents,
-- and it also prohibits "smuggling" queries past the audit log.
```

## Results

Have you ever done a query and then immediately after that, run it again
in a different display mode because the original was unreadable? With the
`\re` commands, you can re-print any of the last queries' results without
have to re-run them, in the same or a different format, and even apply
simple transformations.

```sql
-- Let's get a query's results in the buffer
database=> SELECT * FROM usersselect * from users;
    id   ┆ created ┆ updated ┆ deleted ┆  email  ┆ passwo ┆ displa ┆  role  ┆ displa ┆ visibi ┆ phone_ ┆ update ┆ device
         ┆   _at   ┆   _at   ┆   _at   ┆         ┆   rd   ┆ y_name ┆        ┆  y_id  ┆ lity_s ┆ number ┆ d_at_s ┆ _regis
         ┆         ┆         ┆         ┆         ┆        ┆        ┆        ┆        ┆  tatus ┆        ┆ ync_ti ┆ tratio
         ┆         ┆         ┆         ┆         ┆        ┆        ┆        ┆        ┆        ┆        ┆   ck   ┆ n_quot
         ┆         ┆         ┆         ┆         ┆        ┆        ┆        ┆        ┆        ┆        ┆        ┆    a
═════════╪═════════╪═════════╪═════════╪═════════╪════════╪════════╪════════╪════════╪════════╪════════╪════════╪════════
 0000000 ┆ 2025-09 ┆ 2025-09 ┆ NULL    ┆ system  ┆ NULL   ┆ System ┆ system ┆ NULL   ┆ curren ┆ NULL   ┆ 0      ┆ 0
 0-0000- ┆ -16T06: ┆ -16T06: ┆         ┆         ┆        ┆        ┆        ┆        ┆ t      ┆        ┆        ┆
 0000-00 ┆ 33:12.9 ┆ 33:12.9 ┆         ┆         ┆        ┆        ┆        ┆        ┆        ┆        ┆        ┆
 00-0000 ┆ 13596Z  ┆ 13596Z  ┆         ┆         ┆        ┆        ┆        ┆        ┆        ┆        ┆        ┆
 0000000 ┆         ┆         ┆         ┆         ┆        ┆        ┆        ┆        ┆        ┆        ┆        ┆
 0       ┆         ┆         ┆         ┆         ┆        ┆        ┆        ┆        ┆        ┆        ┆        ┆
╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌
 88085a0 ┆ 2025-09 ┆ 2025-09 ┆ NULL    ┆ admin@t ┆ $2b$12 ┆ Initia ┆ admin  ┆ NULL   ┆ curren ┆ NULL   ┆ 3      ┆ 0
 2-6f4d- ┆ -16T06: ┆ -16T06: ┆         ┆ amanu.i ┆ $JRZEj ┆ l      ┆        ┆        ┆ t      ┆        ┆        ┆
 43ae-ae ┆ 36:33.3 ┆ 36:33.3 ┆         ┆ o       ┆ eHQcqq ┆ Admin  ┆        ┆        ┆        ┆        ┆        ┆
 cb-6cd8 ┆ Z       ┆ Z       ┆         ┆         ┆ qg9bO0 ┆        ┆        ┆        ┆        ┆        ┆        ┆
 fd603f1 ┆         ┆         ┆         ┆         ┆ xucR.z ┆        ┆        ┆        ┆        ┆        ┆        ┆
 c       ┆         ┆         ┆         ┆         ┆ xQZKOM ┆        ┆        ┆        ┆        ┆        ┆        ┆
         ┆         ┆         ┆         ┆         ┆ QntRrp ┆        ┆        ┆        ┆        ┆        ┆        ┆
         ┆         ┆         ┆         ┆         ┆ cFvMMy ┆        ┆        ┆        ┆        ┆        ┆        ┆
         ┆         ┆         ┆         ┆         ┆ l/NetX ┆        ┆        ┆        ┆        ┆        ┆        ┆
         ┆         ┆         ┆         ┆         ┆ 6K83EO ┆        ┆        ┆        ┆        ┆        ┆        ┆
╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌
(4 rows, took 84.806ms)
-- (not all rows shown)
-- That's pretty cramped. I should have used \gx...

-- List out the available results
database=> \re list
Past query results (1 of 1):

 N ┆         When        ┆   Took   ┆   Size  ┆ Rows ┆ Cols
═══╪═════════════════════╪══════════╪═════════╪══════╪══════
 0 ┆ 2025-10-30 10:45:52 ┆ 84.81 ms ┆ 1.68 KB ┆ 4    ┆ 13

Total memory used: 1.68 KB
Memory limit: 1.00 GB

-- Now lets print the users again, but in JSON format:
database=> \re show format=json
{"created_at":"2025-09-16T06:33:12.913596Z","deleted_at":null,"device_registration_quota":0,"display_id":null,"display_name":"System","email":"system","id":"00000000-0000-0000-0000-000000000000","password":null,"phone_number":null,"role":"system","updated_at":"2025-09-16T06:33:12.913596Z","updated_at_sync_tick":0,"visibility_status":"current"}
{"created_at":"2025-09-16T06:36:33.3Z","deleted_at":null,"device_registration_quota":0,"display_id":null,"display_name":"Initial Admin","email":"admin@tamanu.io","id":"88085a02-6f4d-43ae-aecb-6cd8fd603f1c","password":"$2b$12$JRZEjeHQcqqqg9bO0xucR.zxQZKOMQntRrpcFvMMyl/NetX6K83EO","phone_number":null,"role":"admin","updated_at":"2025-09-16T06:36:33.3Z","updated_at_sync_tick":3,"visibility_status":"current"}
{"created_at":"2025-09-16T06:36:33.495Z","deleted_at":null,"device_registration_quota":0,"display_id":null,"display_name":"System: facility-a sync","email":"facility-a@tamanu.io","id":"4345bd44-d6e4-48fd-85c3-c006bbc786e3","password":"$2b$12$ehgYV1nSi.1UePwtS7T5pObCcjtP35rL/MVDnc/u9YwmjU2oFNGwG","phone_number":null,"role":"admin","updated_at":"2025-09-16T06:36:33.495Z","updated_at_sync_tick":3,"visibility_status":"current"}
{"created_at":"2025-11-01T11:45:21.392383Z","deleted_at":null,"device_registration_quota":0,"display_id":null,"display_name":"Admin","email":"admin@bes.au","id":"4bbfb6fd-a1e6-4093-9afd-acbeca759e09","password":null,"phone_number":null,"role":"admin","updated_at":"2025-11-01T01:45:21.392383Z","updated_at_sync_tick":31,"visibility_status":"current"}

-- Let's page through the results one at a time, in expanded format
database=> \re show format=expanded limit=1 offset=0
-[ RECORD 1 ]-
 id                        ┆ 00000000-0000-0000-0000-000000000000
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 created_at                ┆ 2025-09-16T06:33:12.913596Z
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 updated_at                ┆ 2025-09-16T06:33:12.913596Z
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 deleted_at                ┆ NULL
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 email                     ┆ system
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 password                  ┆ NULL
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 display_name              ┆ System
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 role                      ┆ system
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 display_id                ┆ NULL
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 visibility_status         ┆ current
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 phone_number              ┆ NULL
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 updated_at_sync_tick      ┆ 0
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 device_registration_quota ┆ 0

database=> \re show format=expanded limit=1 offset=1
-[ RECORD 1 ]-
 id                        ┆ 88085a02-6f4d-43ae-aecb-6cd8fd603f1c
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 created_at                ┆ 2025-09-16T06:36:33.3Z
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 updated_at                ┆ 2025-09-16T06:36:33.3Z
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 deleted_at                ┆ NULL
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 email                     ┆ admin@tamanu.io
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 password                  ┆ $2b$12$JRZEjeHQcqqqg9bO0xucR.zxQZKOMQntRrpcFvMMyl/NetX6K83EO
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 display_name              ┆ Initial Admin
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 role                      ┆ admin
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 display_id                ┆ NULL
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 visibility_status         ┆ current
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 phone_number              ┆ NULL
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 updated_at_sync_tick      ┆ 3
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 device_registration_quota ┆ 0

-- Lets only show the columns we're interested in
database=> \re show cols=id,email,role,display_name
                  id                  ┆         email        ┆  role  ┆       display_name
══════════════════════════════════════╪══════════════════════╪════════╪═════════════════════════
 00000000-0000-0000-0000-000000000000 ┆ system               ┆ system ┆ System
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 88085a02-6f4d-43ae-aecb-6cd8fd603f1c ┆ admin@tamanu.io      ┆ admin  ┆ Initial Admin
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 4345bd44-d6e4-48fd-85c3-c006bbc786e3 ┆ facility-a@tamanu.io ┆ admin  ┆ System: facility-a sync
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 4bbfb6fd-a1e6-4093-9afd-acbeca759e09 ┆ admin@bes.au         ┆ admin  ┆ Admin

-- Let's make another query (and for demo's sake, hide its output)
database=> select * from patients \gz
(49 rows, took 56.354ms)

database=> \re list
Past query results (2 of 2):

 N ┆         When        ┆   Took   ┆   Size   ┆ Rows ┆ Cols
═══╪═════════════════════╪══════════╪══════════╪══════╪══════
 0 ┆ 2025-10-30 10:45:52 ┆ 84.81 ms ┆ 1.68 KB  ┆ 4    ┆ 13
╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌┼╌╌╌╌╌╌
 1 ┆ 2025-10-30 10:58:39 ┆ 66.08 ms ┆ 34.59 KB ┆ 49   ┆ 20

Total memory used: 36.27 KB
Memory limit: 1.00 GB

-- But how do we tell which query was which? With +
database=> \re list+
Past query results (2 of 2):

 N ┆         When        ┆   Took   ┆   Size   ┆ Rows ┆                Columns                ┆           Query
═══╪═════════════════════╪══════════╪══════════╪══════╪═══════════════════════════════════════╪══════════════════════════
 0 ┆ 2025-10-30 10:45:52 ┆ 84.81 ms ┆ 1.68 KB  ┆ 4    ┆ id, created_at, updated_at,           ┆ select * from users
   ┆                     ┆          ┆          ┆      ┆ deleted_at, email, password,          ┆
   ┆                     ┆          ┆          ┆      ┆ display_name, role, display_id,       ┆
   ┆                     ┆          ┆          ┆      ┆ visibility_status, phone_number,      ┆
   ┆                     ┆          ┆          ┆      ┆ updated_at_sync_tick,                 ┆
   ┆                     ┆          ┆          ┆      ┆ device_registration_quota             ┆
╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 1 ┆ 2025-10-30 10:58:39 ┆ 66.08 ms ┆ 34.59 KB ┆ 49   ┆ id, created_at, updated_at,           ┆ select * from patients
   ┆                     ┆          ┆          ┆      ┆ deleted_at, display_id, first_name,   ┆
   ┆                     ┆          ┆          ┆      ┆ middle_name, last_name,               ┆
   ┆                     ┆          ┆          ┆      ┆ cultural_name, email, date_of_birth,  ┆
   ┆                     ┆          ┆          ┆      ┆ sex, village_id, additional_details,  ┆
   ┆                     ┆          ┆          ┆      ┆ date_of_death, merged_into_id,        ┆
   ┆                     ┆          ┆          ┆      ┆ visibility_status,                    ┆
   ┆                     ┆          ┆          ┆      ┆ date_of_birth_legacy,                 ┆
   ┆                     ┆          ┆          ┆      ┆ date_of_death_legacy,                 ┆
   ┆                     ┆          ┆          ┆      ┆ updated_at_sync_tick                  ┆

Total memory used: 36.27 KB
Memory limit: 1.00 GB

-- Now we can reformat the output of that second query:
database=> \re show limit=1 cols=id,last_name,first_name
                  id                  ┆ last_name ┆ first_name
══════════════════════════════════════╪═══════════╪════════════
 3aa63a3c-bb1e-4f3c-a185-ad18b5218256 ┆ Milani    ┆ Esther

-- And if we want, we can still re-display the users...
database=> \re show n=0 format=csv cols=id,created_at
id,created_at
00000000-0000-0000-0000-000000000000,2025-09-16T06:33:12.913596Z
88085a02-6f4d-43ae-aecb-6cd8fd603f1c,2025-09-16T06:36:33.3Z
4345bd44-d6e4-48fd-85c3-c006bbc786e3,2025-09-16T06:36:33.495Z
4bbfb6fd-a1e6-4093-9afd-acbeca759e09,2025-11-01T11:45:21.392383Z
```

### File-only formats

With `\re show` you can also export results as file-only formats.
These will error if you try using them without specifying `to=`.

```sql
-- Write an Excel spreadsheet (in XLSX format)
database=> \re show n=0 format=excel to=data.xlsx
Output written to /home/myself/data.xlsx

-- Write a SQLite database (data is in the table `results`)
database=> \re show n=0 format=sqlite to=/data/export.db
Output written to /data/export.db
```

## Variables

Variables have a different syntax and command set than in native psql.
Also unlike native psql, there are no variables that affect the tool itself,
nor variables that are set by the tool: it's all your content.

```sql
-- Set a variable manually
database=> \set my_var Hello

-- Print a variable
database=> \get my_var
Hello

-- List variables
database=> \vars
  Name  ┆ Value
════════╪═══════
 my_var ┆ Hello

-- Variables are set literally, with the variable name being a single word
database=> \set my_var Hello, World
database=> \set my_var2 'Hello, World'
database=> \vars
   Name  ┆      Value
═════════╪═════════════════
 my_var  ┆ Hello, World!
╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 my_var2 ┆ 'Hello, World!'

-- Use variables in queries using ${name}
database=> select '${my_var}' as message;
    message
═══════════════
 Hello, World!
(1 row, took 56.483ms)

-- Note again how variables are literal, and substitution also is literal
-- There's no type conversion or anything. Remember what my_var2 is set to?
database=> select ${my_var2} as message;
    message
═══════════════
 Hello, World!
(1 row, took 56.483ms)

database=> select 123 as "${my_var}";
 Hello, World!
═══════════════
 123
(1 row, took 56.032ms)

-- You can even have variables that contain arbitrary parts of queries
database=> \set count select count(*) from
database=> ${count} settings;
 count
═══════
 7
(1 row, took 59.619ms)

-- To use a literal ${foo} string, double the braces:
database=> select '${{foo}}';
 ?column?
══════════
 ${foo}
(1 row, took 55.336ms)
-- (using three braces outputs two, and so on)

-- Alternatively, use verbatim mode:
database=> select '${foo}' \gv
 ?column?
══════════
 ${foo}
(1 row, took 55.336ms)

-- You can use \gset to extract variables from the output of a query
database=> select id from settings limit 1 \gset
                  id
══════════════════════════════════════
 37fa67a4-0c4c-4cfe-9afe-0c32046b6658
(1 row, took 55.586ms)
-- unlike native psql, the output is still printed to screen (or wherever)
-- you can use \gzset if you want to hide the output

-- You can add a prefix to the variable names generated
database=> select id, key from settings limit 1 \gjset set_
{"id":"37fa67a4-0c4c-4cfe-9afe-0c32046b6658","key":"fhir.worker.heartbeat"}
(1 row, took 55.653ms)
-- notice how you can still use JSON output for the printout

-- If a snippet uses a variable, it will be taken from the environment, or
-- you can provide a value when calling the snippet (\i works the same):
database=> \snip run my_snip my_var=123
    message
═══════════════
      123
(1 row, took 52.812ms)

-- And finally, you can filter the \vars output:
database=> \vars set_*
   Name  ┆                 Value
═════════╪══════════════════════════════════════
 set_id  ┆ 37fa67a4-0c4c-4cfe-9afe-0c32046b6658
╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 set_key ┆ fhir.worker.heartbeat
```

## Database Exploration

```sql
-- List all tables in the default schema
database=> \list table
 Schema ┆                   Name                  ┆   Size
════════╪═════════════════════════════════════════╪═════════
 public ┆ SequelizeMeta                           ┆ 120 kB
╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌
 public ┆ administered_vaccines                   ┆ 48 kB
╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌
 public ┆ appointment_schedules                   ┆ 32 kB
╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌
 public ┆ appointments                            ┆ 48 kB
╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌
 public ┆ assets                                  ┆ 32 kB
-- etc

-- Search for tables by pattern with the alias
database=> \dt sync*
 Schema ┆         Name        ┆  Size
════════╪═════════════════════╪════════
 public ┆ sync_device_ticks   ┆ 24 kB
╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌
 public ┆ sync_lookup         ┆ 31 MB
╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌
 public ┆ sync_lookup_ticks   ┆ 24 kB
╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌
 public ┆ sync_queued_devices ┆ 80 kB
╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌
 public ┆ sync_sessions       ┆ 48 kB

-- Search within other schemas and show more detail
database=> \dt+ pg_catalog.pg_ts*
Schema   ┆       Name       ┆  Size ┆   Owner  ┆ Persistence ┆             ACL
════════════╪══════════════════╪═══════╪══════════╪═════════════╪════════════════════════════
 pg_catalog ┆ pg_ts_config     ┆ 72 kB ┆ postgres ┆ permanent   ┆ postgres=arwdDxtm/postgres
            ┆                  ┆       ┆          ┆             ┆ =r/postgres
╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 pg_catalog ┆ pg_ts_config_map ┆ 88 kB ┆ postgres ┆ permanent   ┆ postgres=arwdDxtm/postgres
            ┆                  ┆       ┆          ┆             ┆ =r/postgres
╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 pg_catalog ┆ pg_ts_dict       ┆ 80 kB ┆ postgres ┆ permanent   ┆ postgres=arwdDxtm/postgres
            ┆                  ┆       ┆          ┆             ┆ =r/postgres
╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 pg_catalog ┆ pg_ts_parser     ┆ 72 kB ┆ postgres ┆ permanent   ┆ postgres=arwdDxtm/postgres
            ┆                  ┆       ┆          ┆             ┆ =r/postgres
╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 pg_catalog ┆ pg_ts_template   ┆ 72 kB ┆ postgres ┆ permanent   ┆ postgres=arwdDxtm/postgres
            ┆                  ┆       ┆          ┆             ┆ =r/postgres

-- Listing views shows both regular and materialised views
database=> \list view
 Schema ┆                Name                ┆        Type       ┆   Size
════════╪════════════════════════════════════╪═══════════════════╪═════════
 public ┆ materialized_upcoming_vaccinations ┆ materialized view ┆ 16 kB
╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌
 public ┆ upcoming_vaccinations              ┆ view              ┆ 0 bytes

-- Describing a view shows the columns in the view output, and the + includes its definition
database=> \describe+ upcoming_vaccinations
View "public.upcoming_vaccinations"
        Column        ┆          Type          ┆  Storage
══════════════════════╪════════════════════════╪══════════
 patient_id           ┆ character varying(255) ┆ extended
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌
 scheduled_vaccine_id ┆ character varying(255) ┆ extended
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌
 vaccine_category     ┆ character varying(255) ┆ extended
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌
 vaccine_id           ┆ character varying(255) ┆ extended
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌
 due_date             ┆ date                   ┆ plain
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌
 days_till_due        ┆ integer                ┆ plain
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌
 status               ┆ text                   ┆ extended

View definition:
 WITH vaccine_settings AS (
         SELECT s.value AS thresholds,
            1 AS priority
           FROM settings s
          WHERE s.deleted_at IS NULL AND s.key = 'upcomingVaccinations.thresholds'::text
        UNION
-- snip

-- Listing functions
database=> \df setting*
 Schema ┆     Name    ┆ Result data type ┆              Argument data types              ┆   Type
════════╪═════════════╪══════════════════╪═══════════════════════════════════════════════╪══════════
 public ┆ setting_get ┆ jsonb            ┆ path text, facility character varying DEFAULT ┆ function
        ┆             ┆                  ┆ NULL::character varying                       ┆
╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌
 public ┆ setting_on  ┆ boolean          ┆ path text, facility character varying DEFAULT ┆ function
        ┆             ┆                  ┆ NULL::character varying                       ┆

-- Using \d+ with functions shows their source (where available)
database=> \d+ setting_get
Function "public.setting_get"
   sql, stable, parallel safe
Returns: jsonb

 Argument name ┆        Type
═══════════════╪═══════════════════
 path          ┆ text
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┼╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌
 facility      ┆ character varying

Definition:
CREATE OR REPLACE FUNCTION public.setting_get(path text, facility character varying DEFAULT NULL::character varying)
RETURNS jsonb
LANGUAGE sql
STABLE PARALLEL SAFE
AS $function$
     SELECT value
     FROM settings
     WHERE true
       AND key = path
       AND deleted_at IS NULL
       AND (facility_id IS NULL OR facility_id = facility)
     ORDER BY facility_id DESC LIMIT 1 -- prefer facility-specific setting when both matched
   $function$
```
