#!/usr/bin/env python3
"""Phase 4.6 — clean up offers published during a Phase 4 matrix run.

A `--execute` matrix run publishes a real offer per cell to
`marketplace.froglet.dev`. After the run is assessed, the operator
suspends each of those test providers so the marketplace doesn't
accumulate clutter. The suspension mechanism is a SQL insert into
`provider_enforcements` (see migrations/0003_marketplace_arbiter.sql):
no HTTP admin endpoint exists in the marketplace-api yet.

This script reads the cell JSON files produced by `run_matrix.py`,
extracts the `provider_id` from each cell's `publish_response`, and
emits an idempotent SQL script the operator runs against the
marketplace Postgres.

Usage:

    # Default: find the most recent matrix run and print SQL to stdout
    python3 scripts/llm_acceptance_cleanup.py

    # Operate on a specific run
    python3 scripts/llm_acceptance_cleanup.py --from _tmp/llm_acceptance/20260516T0930Z

    # Write SQL to a file instead of stdout
    python3 scripts/llm_acceptance_cleanup.py --output cleanup.sql

    # Just list provider_ids, no SQL
    python3 scripts/llm_acceptance_cleanup.py --list

The emitted SQL is safe to run multiple times: each row uses a
deterministic enforcement_id derived from provider_id, and the insert
is guarded with ON CONFLICT DO NOTHING.

Running the cleanup (operator):

    psql "$MARKETPLACE_DB_URL" -f cleanup.sql

Or stream directly:

    python3 scripts/llm_acceptance_cleanup.py | psql "$MARKETPLACE_DB_URL"
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Optional


ROOT = Path(__file__).resolve().parent.parent
DEFAULT_RUNS_DIR = ROOT / "_tmp" / "llm_acceptance"


@dataclass
class CellRecord:
    cell_id: str
    provider_id: str
    offer_hash: Optional[str]
    public_url: Optional[str]
    passed: bool


def find_latest_run(runs_dir: Path) -> Path:
    """Return the most recent timestamped run directory under runs_dir."""
    if not runs_dir.is_dir():
        raise SystemExit(
            f"No matrix runs found under {runs_dir}. "
            "Run `python tests/llm_acceptance/run_matrix.py --execute` first."
        )
    candidates = sorted(
        (p for p in runs_dir.iterdir() if p.is_dir() and (p / "cells").is_dir()),
        key=lambda p: p.name,
    )
    if not candidates:
        raise SystemExit(
            f"No matrix runs with a cells/ subdirectory under {runs_dir}. "
            "Did the run complete?"
        )
    return candidates[-1]


def load_cells(run_dir: Path) -> list[CellRecord]:
    """Walk cells/*.json and pull out provider_ids that were actually
    published (have a non-empty publish_response.provider_id)."""
    cells_dir = run_dir / "cells"
    if not cells_dir.is_dir():
        raise SystemExit(f"{cells_dir} is missing — was this an --execute run?")

    records: list[CellRecord] = []
    for cell_file in sorted(cells_dir.glob("*.json")):
        try:
            data = json.loads(cell_file.read_text())
        except json.JSONDecodeError as e:
            print(
                f"warn: {cell_file.name} is not valid JSON ({e}); skipping",
                file=sys.stderr,
            )
            continue

        cell_id = cell_file.stem
        response = data.get("publish_response")
        if not isinstance(response, dict):
            # Cells without publish_response were structural-only
            # (not --execute) or the engine never returned. Nothing to
            # clean up.
            continue
        provider_id = (response.get("provider_id") or "").strip()
        if not provider_id:
            continue
        records.append(
            CellRecord(
                cell_id=cell_id,
                provider_id=provider_id,
                offer_hash=response.get("offer_hash"),
                public_url=response.get("public_url"),
                passed=bool(data.get("passed", False)),
            )
        )
    return records


def deterministic_enforcement_id(provider_id: str, run_ts: str) -> str:
    """Stable enforcement_id derived from provider_id + run timestamp.

    Keeps the SQL idempotent: re-running the same cleanup produces the
    same row keys, so ON CONFLICT DO NOTHING prevents duplicates.
    """
    digest = hashlib.sha256(f"{run_ts}:{provider_id}".encode()).hexdigest()[:16]
    return f"phase4-cleanup-{digest}"


def emit_sql(
    records: list[CellRecord],
    run_ts: str,
    operator_id: str,
    reason: str,
) -> str:
    """Build an idempotent SQL transaction that inserts a
    `suspend_provider` enforcement row per cleaned-up provider_id.

    Schema reference (froglet-services/migrations/0003_marketplace_arbiter.sql):

        provider_enforcements (
            enforcement_id TEXT PRIMARY KEY,
            provider_id TEXT NOT NULL,
            remedy TEXT CHECK (remedy IN ('warning','suspend_provider')),
            reason TEXT NOT NULL,
            operator_id TEXT NOT NULL,
            active BOOLEAN NOT NULL DEFAULT TRUE,
            created_at BIGINT NOT NULL, ...
        )

    The `suspended_providers_v1` view filters on
    (active = TRUE AND remedy = 'suspend_provider'), so a single row
    per provider_id is enough.
    """
    now_unix = int(time.time())
    lines = [
        f"-- Phase 4.6 cleanup — generated {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime(now_unix))}",
        f"-- Run: {run_ts}",
        f"-- Providers to suspend: {len(records)}",
        "--",
        "-- Safe to re-run: deterministic enforcement_id + ON CONFLICT DO NOTHING.",
        "",
        "BEGIN;",
    ]
    for r in records:
        eid = deterministic_enforcement_id(r.provider_id, run_ts)
        # Use $tag$ quoting to avoid any need to escape the reason string.
        # provider_id is opaque; we still parameterize it via SQL literal
        # quoting (single-quote doubling).
        safe_pid = r.provider_id.replace("'", "''")
        lines.append(
            f"INSERT INTO provider_enforcements "
            f"(enforcement_id, provider_id, remedy, reason, operator_id, active, created_at) "
            f"VALUES ("
            f"'{eid}', '{safe_pid}', 'suspend_provider', "
            f"$tag${reason} (cell {r.cell_id})$tag$, "
            f"'{operator_id}', TRUE, {now_unix}) "
            f"ON CONFLICT (enforcement_id) DO NOTHING;"
        )
    lines.append("COMMIT;")
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--from",
        dest="from_dir",
        type=Path,
        help="Path to a matrix run directory (default: most recent under _tmp/llm_acceptance/)",
    )
    parser.add_argument(
        "--output",
        "-o",
        type=Path,
        help="Write SQL to this file instead of stdout",
    )
    parser.add_argument(
        "--list",
        action="store_true",
        help="Just print provider_ids (one per line). No SQL.",
    )
    parser.add_argument(
        "--operator-id",
        default="phase4-cleanup",
        help="operator_id recorded on each enforcement row (default: phase4-cleanup)",
    )
    parser.add_argument(
        "--reason",
        default="Phase 4 LLM acceptance matrix test offer",
        help="Free-form reason recorded on each enforcement row",
    )
    args = parser.parse_args()

    run_dir = args.from_dir if args.from_dir else find_latest_run(DEFAULT_RUNS_DIR)
    if not run_dir.is_dir():
        raise SystemExit(f"--from {run_dir} is not a directory")
    run_ts = run_dir.name

    records = load_cells(run_dir)
    if not records:
        print(
            f"No provider_ids to clean up in {run_dir}. "
            "Either this was a structural-only run (no --execute) or "
            "every cell failed before publishing.",
            file=sys.stderr,
        )
        return 0

    if args.list:
        for r in records:
            print(r.provider_id)
        print(
            f"\n{len(records)} provider(s) from {run_dir.name}",
            file=sys.stderr,
        )
        return 0

    sql = emit_sql(records, run_ts, args.operator_id, args.reason)

    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(sql)
        print(
            f"Wrote suspension SQL for {len(records)} provider(s) to {args.output}\n"
            f"Run: psql \"$MARKETPLACE_DB_URL\" -f {args.output}",
            file=sys.stderr,
        )
    else:
        sys.stdout.write(sql)
        print(
            f"\n-- {len(records)} provider(s) from {run_dir.name}",
            file=sys.stderr,
        )

    return 0


if __name__ == "__main__":
    sys.exit(main())
