#!/usr/bin/env python3
"""Real Codex publishing rehearsal against an existing native installation.

Uses synthetic data, a fresh working directory and no repository knowledge.
Stops at exact public consent. This is not installation/recipient qualification.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import tomllib


PRIVATE_MARKER = "PRIVATE-NOTES-NATIVE-ACCEPTANCE-DO-NOT-PUBLISH"
CATALOG = {
    "metadata": {"title": "Synthetic workshop catalog", "version": "1"},
    "items": [
        {"id": "001", "name": "Widget", "stock": {"available": 62}, "notes": PRIVATE_MARKER},
        {"id": "002", "name": "Gadget", "stock": {"available": 12}, "notes": PRIVATE_MARKER},
        {"id": "003", "name": "Bracket", "stock": {"available": 83}, "notes": PRIVATE_MARKER},
    ],
}


def read_profile(path):
    """Read only the configured Froglet server, never log other MCP entries."""
    if path.suffix == ".toml":
        profile = tomllib.loads(path.read_text())["mcp_servers"]["froglet"]
    else:
        profile = json.loads(path.read_text())["mcpServers"]["froglet"]
    command = Path(profile["command"])
    if not command.is_absolute() or profile.get("args") != ["mcp"]:
        raise ValueError("Expected an absolute native Froglet command with args [mcp]")
    env = profile.get("env", {})
    if any("TOKEN" in k and not k.endswith("_PATH") for k in env):
        raise ValueError("Use token-file paths, not inline secrets, for the acceptance profile")
    return {"command": str(command), "args": ["mcp"], "env": env}


def cli(profile, *args, cwd=None):
    # Do not inherit an unrelated installation's credentials or data root.
    env = {k: v for k, v in os.environ.items() if not k.startswith("FROGLET_")}
    env.update(profile["env"])
    result = subprocess.run([profile["command"], *args], cwd=cwd, env=env,
                            capture_output=True, text=True, timeout=90, check=True)
    return json.loads(result.stdout)


def verify_package(run_dir, original_hash):
    """Fail on leaked/excluded data or invented evidence, not just bad prose."""
    source = run_dir / "catalog.json"
    assert hashlib.sha256(source.read_bytes()).hexdigest() == original_hash, "Original source changed"
    project = run_dir / "prepared"
    record = json.loads((project / ".froglet/preparation.json").read_text())
    manifest = tomllib.loads((project / "froglet-service.toml").read_text())
    snapshot_path = project / record["snapshot_file"]
    assert snapshot_path.resolve().is_relative_to(project.resolve()), "Snapshot escapes project"
    raw = snapshot_path.read_bytes()
    assert PRIVATE_MARKER.encode() not in raw, "Private notes leaked into snapshot"
    snapshot = json.loads(raw)
    assert set(snapshot) == {"metadata", "items"}, "Selected collections changed"
    assert snapshot["metadata"] == [CATALOG["metadata"]], "Metadata changed during normalization"
    assert snapshot["items"] == [
        {k: row[k] for k in ("id", "name", "stock")} for row in CATALOG["items"]
    ], "Rows changed or excluded fields leaked"
    digest = hashlib.sha256(raw).hexdigest()
    assert digest == record["snapshot_sha256"], "Snapshot hash does not match preparation"
    assert manifest["data"]["path"] == record["snapshot_file"], "Manifest serves different bytes"
    assert manifest["price"]["sats"] == 0 and manifest["settlement"]["method"] == "none", "Not free"
    query = manifest["verification"]["input"]
    assert query["op"] == "select" and query["collection"] == "items", "No meaningful item example"
    assert not ({"query", "filter", "gt"} & set(query)), "Unsupported query invented"
    return record, digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--froglet-config", type=Path, required=True)
    parser.add_argument("--mode", choices=["mcp", "cli", "discovery"], default="mcp")
    parser.add_argument("--guide", default="https://froglet.dev/publish/agent.md")
    parser.add_argument("--timeout", type=int, default=600)
    args = parser.parse_args()
    profile = read_profile(args.froglet_config)
    before = cli(profile, "status", "--json")
    if not before["provider_running"] or not before["runtime_running"]:
        raise RuntimeError("Acceptance requires a healthy existing provider and runtime")
    before_publications = before["publications"]["publications"]
    run_dir = Path(tempfile.mkdtemp(prefix=f"froglet-agent-{args.mode}-")).resolve()
    source = run_dir / "catalog.json"
    source.write_text(json.dumps(CATALOG, indent=2) + "\n")
    original_hash = hashlib.sha256(source.read_bytes()).hexdigest()
    # This is normal installed-agent context, not a corrective prompt. For the
    # CLI case the client has the installation record but no attached MCP tool.
    if args.mode != "discovery":
        (run_dir / ".mcp.json").write_text(json.dumps({"mcpServers": {"froglet": profile}}, indent=2))
    service_id = "agent-catalog-" + run_dir.name.rsplit("-", 1)[-1].replace("_", "a")
    prompt = f"""Make this catalog usable by another agent with Froglet. Read {args.guide}.
My source is {source}. Create the service project at {run_dir / 'prepared'},
with service ID {service_id}. Share metadata and each item's id, name and stock;
exclude notes. Recipients will want items where stock.available is greater than 50.
Preserve my original export; you may create a normalized copy in this directory.
Use my existing installation. Do not install, restart or stop anything.
Prepare and verify a meaningful local example, then obtain the exact FREE public
publication plan and STOP for my approval. I have not approved public publication.
Do not read a source checkout or old conversation to solve this.
Save the actual plan JSON to publication-plan.json in this directory and explain
what worked, the exact public selection and any limitations. Do not invent evidence.
"""
    (run_dir / "prompt.txt").write_text(prompt)
    command = ["codex", "exec", "--ephemeral", "--ignore-user-config", "--approve-for-me",
               "--skip-git-repo-check", "-C", str(run_dir), "--json",
               "--output-last-message", str(run_dir / "agent-final.md")]
    if args.mode == "mcp":
        command += ["-c", "mcp_servers.froglet.command=" + json.dumps(profile["command"]),
                    "-c", 'mcp_servers.froglet.args=["mcp"]']
        for key, value in profile["env"].items():
            command += ["-c", f"mcp_servers.froglet.env.{key}=" + json.dumps(value)]
    command += ["-"]
    print(f"Acceptance artifacts: {run_dir}", flush=True)
    started = time.monotonic()
    summary = {"mode": args.mode, "guide": args.guide, "scope": "existing_installation_to_public_consent",
               "run_dir": str(run_dir), "passed": False, "public_execution_tested": False}
    try:
        with (run_dir / "events.jsonl").open("w") as log, (run_dir / "stderr.log").open("w") as err:
            result = subprocess.run(command, input=prompt, text=True, stdout=log, stderr=err,
                                    timeout=args.timeout, check=False)
        assert result.returncode == 0, f"Agent exited {result.returncode}"
        record, digest = verify_package(run_dir, original_hash)
        # Execute the recorded example in the released native handler again;
        # neither the transcript nor a hand-authored proof is trusted.
        request_path = run_dir / "recheck-request.json"
        request_path.write_text(json.dumps(record["request"]))
        local = cli(profile, "prepare-service", "--request", str(request_path), "--json")
        (run_dir / "rechecked-preparation.json").write_text(json.dumps(local, indent=2))
        assert local["stage"] == "local_example_verified" and local["public"] is False
        assert local["example_result"]["rows"], "Local query returned no rows"
        plan = cli(profile, "publish", "--host", "relay", "--plan", "--json", cwd=run_dir / "prepared")
        (run_dir / "rechecked-plan.json").write_text(json.dumps(plan, indent=2))
        agent_plan = json.loads((run_dir / "publication-plan.json").read_text())
        assert plan["status"] == "approval_required", "Not awaiting consent"
        assert plan["consent_hash"] == agent_plan["consent_hash"], "Agent did not obtain the exact plan"
        assert plan["summary"]["package_digest"] == digest, "Plan is not bound to selected bytes"
        after = cli(profile, "status", "--json")
        assert before_publications == after["publications"]["publications"], "Publication state changed without approval"
        summary.update(passed=True, consent_hash=plan["consent_hash"], snapshot_sha256=digest,
                       stage=local["stage"], provider_id=plan["summary"]["provider_id"])
    except (AssertionError, KeyError, ValueError, OSError, subprocess.SubprocessError) as error:
        summary["failure"] = str(error)
    summary["elapsed_seconds"] = round(time.monotonic() - started, 1)
    (run_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary, indent=2))
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
