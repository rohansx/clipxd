# Shared Postgres on Dokploy

Stood up 2026-08-03. One cluster, one database per project, replacing the pattern where every
project brought its own Postgres container.

## What exists

Dokploy project **shared-infra** → service **postgres-shared** (`pgvector/pgvector:pg16`).

The pgvector image rather than plain `postgres:16-alpine` on purpose: leadecho's current database
runs `pgvector/pgvector:pg16`, so choosing the extension-bearing image now is what makes migrating
it later a restore rather than a re-platform. Confirmed present:
`SELECT count(*) FROM pg_available_extensions WHERE name='vector'` → 1.

| database | owner | status |
|---|---|---|
| `clipxd` | `app_clipxd` | for the enrichment worker (queue + job state) |
| `crdt` | `app_crdt` | for the Phase-5 sync server |
| `cloakpipe` | `app_cloakpipe` | empty landing pad |
| `leadecho` | `app_leadecho` | empty landing pad |
| `umami` | `app_umami` | empty landing pad |

The three landing pads are deliberately empty. **Nothing has been migrated.** The live cloakpipe,
leadecho and umami databases are still running in their own containers, untouched. The pads exist
so that a future migration is a `pg_restore` into a database that already has the right owner and
privileges, rather than a schema decision made under pressure inside a maintenance window.

## Isolation

Database-per-project, not schema-per-project. Postgres cannot query across databases, which here
is the feature: a hard boundary, and `pg_dump`/restore works per project.

Each role can connect only to its own database — `REVOKE CONNECT ON DATABASE x FROM PUBLIC`
followed by a grant to that project's role. Without the revoke, every role on the cluster can open
every database, which would make the boundary decorative. Verified:

```
app_clipxd → clipxd : write + read ok
app_clipxd → umami  : FATAL: permission denied for database "umami"
                      DETAIL: User does not have CONNECT privilege.
```

## Connecting

From another Dokploy service, over the internal network:

```
postgres://app_<project>:<password>@postgres-shared-rfjeae:5432/<database>
```

Check both services share `dokploy-network` if the hostname doesn't resolve.

**No external port.** One was opened temporarily (55432) to run the setup SQL and closed again
immediately after — verified shut from the internet. Re-open it the same way if you need psql
access from outside, and close it when you're done: this host has no firewall in front of the
database, so an always-open Postgres port is an always-open Postgres port.

## Credentials

The superuser password is stored in Dokploy (Settings → the postgres-shared service). Per-role
passwords were generated at creation time and are **not** recorded in this repo. Retrieve them
from wherever you store secrets, or rotate:

```sql
ALTER ROLE app_clipxd WITH PASSWORD '<new>';
```

## What has NOT been done

Consolidating the three live databases. That needs a dump/restore per project, because umami is on
**PG 15** while the shared cluster is 16 — a version jump, not a pointer change — and each stack's
`DATABASE_URL` has to move with it. Three of the four stacks are git-sourced, so their compose
files change in their own repos rather than in Dokploy.

The migration, when you want it, per database:

1. `pg_dump -Fc` from the live container, verify the dump restores into a scratch database first.
2. Stop the app (not the old database), restore into the pad, run the app's own migration check.
3. Point `DATABASE_URL` at the shared cluster, redeploy, verify.
4. Leave the old container stopped but its **volume intact** for a week before deleting anything.

Umami and leadecho will each be down for the few minutes of step 2–3.
