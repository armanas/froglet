# Mounts

Mounts give a published service access to data sources the operator has
pre-configured. They are capability-gated: a workload only receives a mount
if its `requested_access` capability list was granted. This keeps the
data-source surface small, declarative, and auditable.

## Model

An `ExecutionMount` carries:

- `kind` — data-source family (`postgres`, `filesystem`, …)
- `handle` — operator-chosen name (e.g. `analytics`)
- `read_only` — whether the workload may write
- `binding` — optional source-specific binding override (legacy; most modern
  kinds resolve binding from operator config instead)

The capability string encoding is:

```
mount.{kind}.{read|write}.{handle}
```

When a workload is admitted, the deal's `capabilities_granted` list
determines which mounts are visible to the runtime. A declared mount whose
capability is not granted is silently dropped from the context supplied to
the workload.

## Postgres mount

The `postgres` mount kind exposes an operator-configured DSN to the workload
as an environment variable. The workload opens its own connection; Froglet
does not proxy queries or parse SQL.

### Operator configuration

Set one env var per mount handle:

```
FROGLET_MOUNT_postgres_<handle>=<dsn>
```

For example:

```
FROGLET_MOUNT_postgres_analytics=postgres://reader:hunter2@db.internal:5432/analytics
FROGLET_MOUNT_postgres_reporting=postgres://service:xyz@db.internal:5432/reporting
```

Handles are normalised to lowercase. DSNs are secrets — store them in your
operator config (systemd drop-in, Compose env file, Kubernetes Secret) and
never commit them to the public repo.

### Service declaration

A published service that needs a Postgres mount declares it on its offer:

```json
{
  "mounts": [
    { "kind": "postgres", "handle": "analytics", "read_only": true }
  ]
}
```

And requests the capability at deal time via `requested_access`:

```
["mount.postgres.read.analytics"]
```

### Runtime exposure

For every granted `postgres` mount the workload receives:

- `FROGLET_MOUNT_<HANDLE>_URL` — the DSN string
- `FROGLET_MOUNT_<HANDLE>_READ_ONLY` — `"true"` or `"false"`

`<HANDLE>` is upper-cased to match the standard env-var naming convention.

A Python handler can use the mount like this:

```python
import os
import psycopg

def handler(event, ctx):
    dsn = os.environ["FROGLET_MOUNT_ANALYTICS_URL"]
    read_only = os.environ.get("FROGLET_MOUNT_ANALYTICS_READ_ONLY") == "true"
    with psycopg.connect(dsn) as conn:
        with conn.cursor() as cur:
            cur.execute("SELECT count(*) FROM events")
            return {"events": cur.fetchone()[0]}
```

## Sandbox interaction

The Python sandbox (see [RUNTIME.md](RUNTIME.md) `Python sandbox`) blocks
all outbound network syscalls by default. When any granted mount requires
network access (today: `postgres`), the sandbox's `allow_network` flag is
set for that invocation, which re-enables `socket` / `connect` / `bind`.

This is intentionally coarse-grained for v1: a workload that is granted a
Postgres mount can currently open any outbound TCP connection, not just to
the DB host. Tightening egress to the exact `host:port` tuple (either by
running the sandbox inside a network namespace with an iptables allowlist
or by installing a per-handle BPF socket filter) is tracked as a security
hardening follow-up in `TODO.md`.

## Honor-system read-only

`read_only` is passed to the workload as an env var but is **not** enforced
at the protocol or network layer. A workload that wants to honor the
constraint must do so itself (e.g., open the Postgres connection in
read-only mode via `SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY`).
Operators who cannot rely on cooperating workloads should grant a DB role
that has no write privileges to begin with.

## Adding a new mount kind

Follow the same shape:

1. Add `FROGLET_MOUNT_<kind>_<handle>` config loading in `src/config.rs` or
   at use-site env lookup.
2. Extend `collect_<kind>_mount_env` in `src/api/mod.rs` so the granted
   mount injects the right env vars.
3. Decide whether the sandbox needs `allow_network=true` (DBs, remote
   stores) or stays network-free (read-only filesystem cache).
4. Document the handle → env-var shape in this file.

Follow-ups for additional kinds (SQLite, S3, KV) are tracked as a TODO.md
entry.
