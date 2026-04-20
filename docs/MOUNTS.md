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

## Supported kinds

| Kind | Needs network | Extra sandbox allow-list | Binding shape |
|---|---|---|---|
| `postgres` | yes | none | `postgres://user:pass@host:port/db` |
| `sqlite` | no | parent directory of the DB file | absolute path to the `.sqlite` file |
| `s3` | yes | none | `s3://access_key:secret@endpoint/bucket` |
| `redis` | yes | none | `redis://user:pass@host:port/db` |

All kinds inject the same env-var shape into the workload:

- `FROGLET_MOUNT_<HANDLE>_URL` — the operator-configured binding string
- `FROGLET_MOUNT_<HANDLE>_READ_ONLY` — `"true"` or `"false"`

Kind-specific sandbox effects are applied by
[`collect_data_mount_plan`](../src/api/mod.rs). Network-backed kinds
(`postgres`, `s3`, `redis`) flip the Python sandbox's `allow_network` flag.
File-backed kinds (`sqlite`) extend the sandbox's `writable_paths` to include
the DB file's parent directory so SQLite's `-journal` / `-wal` files work.

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

## SQLite mount

The `sqlite` mount kind gives the workload a local SQLite database file
protected by the Python sandbox's landlock filesystem restrictions.

### Operator configuration

```
FROGLET_MOUNT_sqlite_<handle>=/absolute/path/to/database.sqlite
```

For example:

```
FROGLET_MOUNT_sqlite_cache=/var/lib/froglet/cache.sqlite
```

The path must be absolute. The sandbox grants write access to the **parent
directory** of the DB file so SQLite's `-journal` and `-wal` sidecar files
can be created and updated. Put each SQLite mount in its own directory if
you want per-mount isolation at the filesystem layer.

### Service declaration

```json
{
  "mounts": [
    { "kind": "sqlite", "handle": "cache", "read_only": false }
  ]
}
```

Request with `requested_access`:

```
["mount.sqlite.write.cache"]
```

### Workload usage

```python
import os
import sqlite3

def handler(event, ctx):
    path = os.environ["FROGLET_MOUNT_CACHE_URL"]
    with sqlite3.connect(path) as conn:
        cur = conn.execute("SELECT count(*) FROM items")
        return {"items": cur.fetchone()[0]}
```

SQLite mounts do not open network syscalls; the sandbox's default
network-deny stays in effect.

## S3 mount

The `s3` mount kind exposes an operator-configured S3-compatible endpoint
and credentials to the workload as an environment variable. Like Postgres,
the workload uses its own S3 client (`boto3`, `aiobotocore`, `s3fs`, etc.)
to make requests.

### Operator configuration

```
FROGLET_MOUNT_s3_<handle>=s3://<access_key>:<secret_key>@<endpoint>/<bucket>
```

For example:

```
FROGLET_MOUNT_s3_backups=s3://AKIA...:wJalrXUt...@s3.us-east-1.amazonaws.com/my-backups
FROGLET_MOUNT_s3_local=s3://minio:minio123@minio.internal:9000/dev-bucket
```

The URL form works for AWS S3, MinIO, Cloudflare R2, Backblaze B2, and
other S3-compatible stores. The workload parses the URL itself; Froglet
does not proxy object-store calls.

### Service declaration

```json
{
  "mounts": [
    { "kind": "s3", "handle": "backups", "read_only": true }
  ]
}
```

Request with `requested_access`:

```
["mount.s3.read.backups"]
```

### Workload usage

```python
import os
from urllib.parse import urlparse
import boto3

def handler(event, ctx):
    raw = os.environ["FROGLET_MOUNT_BACKUPS_URL"]
    parsed = urlparse(raw)
    client = boto3.client(
        "s3",
        endpoint_url=f"https://{parsed.hostname}",
        aws_access_key_id=parsed.username,
        aws_secret_access_key=parsed.password,
    )
    bucket = parsed.path.lstrip("/")
    resp = client.list_objects_v2(Bucket=bucket, Prefix=event.get("prefix", ""))
    return {"keys": [obj["Key"] for obj in resp.get("Contents", [])]}
```

S3 mounts enable the sandbox's network flag, same as Postgres.

## Redis mount

The `redis` mount kind follows the Postgres pattern exactly — a DSN is
exposed via `FROGLET_MOUNT_<HANDLE>_URL`, the workload brings its own Redis
client, and the sandbox enables network syscalls for the invocation.

### Operator configuration

```
FROGLET_MOUNT_redis_<handle>=redis://[user:pass@]host:port[/db]
```

For example:

```
FROGLET_MOUNT_redis_cache=redis://default:secret@cache.internal:6379/0
```

### Service declaration + usage

Declare the mount with `kind: "redis"`, request the capability
`mount.redis.<read|write>.<handle>`, read `FROGLET_MOUNT_<HANDLE>_URL`
from the workload, and open the client connection. `redis.asyncio` or the
blocking `redis` client both work.

## Sandbox interaction

The Python sandbox (see [RUNTIME.md](RUNTIME.md) `Python sandbox`) blocks
all outbound network syscalls and all filesystem writes outside the
invocation tempdir by default. Data mounts extend that default:

- **Network-backed kinds** (`postgres`, `s3`, `redis`) flip the sandbox's
  `allow_network` flag. When a grant of any such mount is active, the
  `socket` / `connect` / `bind` syscalls are permitted.
- **File-backed kinds** (`sqlite`) extend the sandbox's `writable_paths`
  with the DB file's parent directory. Landlock grants that tree read +
  write; everything else stays denied.

Both are intentionally coarse-grained for v1:

- Network: a workload granted a Postgres mount can open any outbound TCP
  connection, not only to the configured DB host. Tightening egress to
  the exact `host:port` tuple (network namespace + iptables allow-list,
  or a per-handle BPF socket filter) is tracked as a security hardening
  follow-up.
- Filesystem: granting a SQLite mount grants the parent directory rather
  than only the `.db` file, so the `-journal` and `-wal` sidecars work.
  Operators who want stricter isolation should give each SQLite mount
  its own directory.

## Honor-system read-only

`read_only` is passed to the workload as an env var but is **not** enforced
at the protocol or network layer. A workload that wants to honor the
constraint must do so itself (e.g., open the Postgres connection in
read-only mode via `SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY`).
Operators who cannot rely on cooperating workloads should grant a DB role
that has no write privileges to begin with.

## Adding a new mount kind

Follow the existing shape in
[`collect_data_mount_plan`](../src/api/mod.rs):

1. Add the kind to the `NETWORK_KINDS` slice (if it needs outbound TCP) or
   the `FILE_KINDS` slice (if it needs a filesystem allow-list extension).
2. If the kind needs filesystem paths, add a branch that populates
   `plan.writable_paths` from the binding.
3. Add operator config via `FROGLET_MOUNT_<kind>_<handle>=<binding>`. The
   handle is already parsed uniformly; no additional env-loading code is
   needed.
4. Add tests in `src/api/mod.rs::tests` mirroring the existing postgres /
   sqlite / s3 tests.
5. Document the kind in the table + dedicated section above.

Planned follow-ups:

- kinds beyond the current postgres + sqlite + s3 + redis set
  (DynamoDB, GCS, a KV snapshot service, etc.)
- Tightening the network allow-list from coarse-grained "any outbound TCP"
  to per-handle `host:port` remains a hardening follow-up.
