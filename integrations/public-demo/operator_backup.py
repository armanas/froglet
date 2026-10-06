"""VM-only consistent SQLite backup and isolated restoration check.

Never restores production, exports a database, or resets usage. Run only through
the app's operator console. Public callers cannot reach this program.
"""
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import time


def snapshot(path):
    c = sqlite3.connect('file:' + str(path) + '?mode=ro', uri=True)
    try:
        if c.execute('PRAGMA integrity_check').fetchone()[0] != 'ok':
            raise RuntimeError('database-integrity-failed')
        tables = [r[0] for r in c.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")]
        schemas = list(c.execute("SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name"))
        rows = {}
        for table in tables:
            name = table.replace('"', '""')
            values = [repr(tuple(row)) for row in c.execute('SELECT * FROM "' + name + '"')]
            rows[table] = hashlib.sha256('\n'.join(sorted(values)).encode()).hexdigest()
        return {'schema': schemas, 'row_digests': rows}
    finally:
        c.close()


def main():
    os.umask(0o077)
    root = Path('/state')
    out = root / ('operator-backup-' + str(time.time_ns()))
    out.mkdir(mode=0o700)
    records = []
    for source in (root / 'node/node.db', root / 'traffic.db'):
        if source.is_symlink() or not source.is_file():
            raise RuntimeError('existing-database-required')
        backup = out / source.name
        src = sqlite3.connect('file:' + str(source) + '?mode=ro', uri=True)
        dst = sqlite3.connect(backup)
        try:
            src.backup(dst)
        finally:
            dst.close()
            src.close()
        os.chmod(backup, 0o600)
        restored = out / ('restored-' + source.name)
        src = sqlite3.connect('file:' + str(backup) + '?mode=ro', uri=True)
        dst = sqlite3.connect(restored)
        try:
            src.backup(dst)
        finally:
            dst.close()
            src.close()
        os.chmod(restored, 0o600)
        if snapshot(backup) != snapshot(restored):
            raise RuntimeError('isolated-restore-mismatch')
        records.append({'database': source.name, 'backup_sha256': hashlib.sha256(backup.read_bytes()).hexdigest(), 'restore_integrity_and_schema_and_rows_match': True})
    result = {'status': 'pass', 'records': records, 'production_restored': False,
              'usage_reset': False, 'credentials_or_databases_exported': False,
              'backup_location': str(out)}
    (out / 'verification.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({k: v for k, v in result.items() if k != 'backup_location'}))


if __name__ == '__main__':
    main()
