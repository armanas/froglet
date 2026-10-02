#!/usr/bin/env python3
"""Disposable free Alice/Bob demo using native MCP and configured A2A.

Python standard library only. Building requires Cargo/Rust and the installed
wasm32-unknown-unknown target; paired --binary/--module paths skip all builds.
No signing, execution or payment logic lives here.
"""
from __future__ import annotations

import argparse
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

if __package__:
    from .research_profile import ANNOTATION, demo_profile, demo_requirements, preflight, validate_rows
else:  # Direct `python3 examples/a2a_compute_demo.py` invocation.
    from research_profile import ANNOTATION, demo_profile, demo_requirements, preflight, validate_rows

REPO = Path(__file__).resolve().parents[1]
EXTENSION = "https://froglet.dev/a2a/bounded-deal/v1"
# Same no-import infinite-loop Wasm fixture as python/tests/test_support.py:
# memory, alloc(len)->16, run(ptr,len)->i64 loops forever. No WAT tool required.
LOOP_HEX = (
    "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279"
    "020005616c6c6f6300000372756e00010a0f02040041100b080003400c000b000b"
)
HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}))
PRIVATE_SENTINEL = "DEMO_PRIVATE_CURATOR_NOTE_DO_NOT_PUBLISH"


def ontology_source():
    """Synthetic labels, deliberately including duplicate and multiple-target rows."""
    rows = [
        ("DEMO:sample.plasma", "DEMO:specimen.plasma"),
        ("DEMO:sample.serum", "DEMO:specimen.serum"),
        ("DEMO:sample.blood", "DEMO:specimen.whole_blood"),
        ("DEMO:sample.blood", "DEMO:specimen.plasma"),
        ("DEMO:sample.plasma", "DEMO:specimen.plasma"),
    ]
    return {"terminology": [{"source": source, "target": target,
                              "curator_note": PRIVATE_SENTINEL} for source, target in rows]}


def mapping_oracle(request):
    """Independent Python computation; exact strings, with no ontology truth claim."""
    require(isinstance(request, dict) and set(request) == {"mappings", "observed_terms"},
            "Invalid mapping checker request")
    require(isinstance(request["mappings"], list)
            and isinstance(request["observed_terms"], list), "Expected mapping/term arrays")
    targets = {}
    for row in request["mappings"]:
        require(isinstance(row, dict) and set(row) == {"source", "target"}
                and all(isinstance(value, str) and value for value in row.values()),
                "Mapping rows must have nonempty source and target strings")
        targets.setdefault(row["source"], set()).add(row["target"])
    require(all(isinstance(term, str) and term for term in request["observed_terms"]),
            "Observed terms must be nonempty strings")
    observed = set(request["observed_terms"])
    conflicts = [{"source": source, "targets": sorted(values)}
                 for source, values in sorted(targets.items()) if len(values) > 1]
    return {"mappings_consistent": not conflicts, "conflicts": conflicts,
            "unmapped_terms": sorted(observed.difference(targets)),
            "mapped_terms": sum(len(targets.get(term, ())) == 1 for term in observed)}


def ontology_cases(retrieved_rows):
    """Every case derives mappings from the actually retrieved service result."""
    mapping_oracle({"mappings": retrieved_rows, "observed_terms": []})
    groups = {}
    for row in retrieved_rows:
        groups.setdefault(row["source"], set()).add(row["target"])
    require(any(len(values) > 1 for values in groups.values()),
            "Retrieved example must contain multiple targets")
    # Alice's explicit one-target policy: retain the lexically last target for
    # the consistent control case only. Never silently apply this to the audit.
    consistent = [dict(row) for row in retrieved_rows
                  if row["target"] == max(groups[row["source"]])]
    observed = sorted(groups)
    absent = "DEMO:sample.saliva"
    require(absent not in groups, "Example missing term is already mapped")
    return [{"name": name, "input": {"mappings": [dict(row) for row in rows],
                                      "observed_terms": list(terms)}}
            for name, rows, terms in (
                ("consistent", consistent, observed),
                ("conflicting_targets", retrieved_rows, observed),
                ("unmapped_term", consistent, observed + [absent]),
                ("conflict_and_unmapped", retrieved_rows, observed + [absent]))]


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def clean_environment():
    proxies = {"http_proxy", "https_proxy", "all_proxy", "no_proxy"}
    return {key: value for key, value in os.environ.items()
            if not key.startswith("FROGLET_") and key.lower() not in proxies}


def private_json(path, value):
    with path.open("x", encoding="utf-8") as output:
        json.dump(value, output)
        output.write("\n")
    path.chmod(0o600)


def get_json(url, headers=None):
    request = urllib.request.Request(url, headers=headers or {})
    with HTTP.open(request, timeout=5) as response:
        content = response.read(4 * 1024 * 1024 + 1)
    require(len(content) <= 4 * 1024 * 1024, "HTTP response exceeds demo limit")
    return json.loads(content)


def stop(process):
    if process.poll() is None:
        try:
            process.terminate()
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            try:
                process.kill()
            except ProcessLookupError:
                pass
            process.wait(timeout=5)


class Node:
    def __init__(self, binary, root, name, role):
        self.binary, self.name, self.role = binary, name, role
        self.data = root / name
        self.data.mkdir(mode=0o700)
        self.env = clean_environment()
        self.env.update({
            "FROGLET_NODE_ROLE": role,
            "FROGLET_NETWORK_MODE": "clearnet",
            "FROGLET_DATA_DIR": str(self.data),
            "FROGLET_LISTEN_ADDR": "127.0.0.1:0",
            "FROGLET_RUNTIME_LISTEN_ADDR": "127.0.0.1:0",
            "FROGLET_PAYMENT_BACKEND": "none",
            "FROGLET_PRICE_EXEC_WASM": "0",
            "FROGLET_PRICE_EVENTS_QUERY": "0",
            "FROGLET_EXECUTION_TIMEOUT_SECS": "2",
            "FROGLET_PROVIDER_MAX_TOTAL_DEALS": "10",
            "FROGLET_PROVIDER_MAX_TOTAL_QUOTES": "20",
            "FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS": "20000",
            "RUST_LOG": "warn",
        })
        self.process = None
        self.starts = 0

    def identity(self):
        result = subprocess.run([str(self.binary), "print-identity"], cwd=REPO,
                                env=self.env, capture_output=True, text=True, timeout=30)
        require(result.returncode == 0, f"{self.name} identity initialization failed")
        identity = result.stdout.strip()
        require(re.fullmatch(r"[0-9a-f]{64}", identity), "Invalid public identity")
        return identity

    def start(self):
        self.starts += 1
        self.log = self.data / f"node-{self.starts}.log"
        with self.log.open("w", encoding="utf-8") as output:
            self.process = subprocess.Popen([str(self.binary)], cwd=REPO, env=self.env,
                                            stdout=output, stderr=subprocess.STDOUT)
        label = "Local API Gateway" if self.role == "provider" else "Local Runtime API"
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            require(self.process.poll() is None,
                    f"{self.name} exited during startup; inspect {self.log}")
            match = re.search(label + r": (http://127\.0\.0\.1:\d+)",
                              self.log.read_text(encoding="utf-8", errors="replace"))
            if match:
                self.url = match.group(1)
                try:
                    if get_json(self.url + "/health").get("status") == "ok":
                        return
                except (urllib.error.URLError, TimeoutError, json.JSONDecodeError):
                    pass
            time.sleep(0.1)
        raise RuntimeError(f"{self.name} startup timed out; inspect {self.log}")

    def stop(self):
        if self.process is not None:
            stop(self.process)


def mcp(alice, arguments, environment=None, command=None):
    environment = environment if environment is not None else dict(alice.env,
        FROGLET_RUNTIME_URL=alice.url,
        FROGLET_RUNTIME_AUTH_TOKEN_PATH=str(alice.data / "runtime/auth.token"))
    messages = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "froglet-local-a2a-demo", "version": "1"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "froglet", "arguments": arguments}},
    ]
    process = subprocess.Popen(command or [str(alice.binary), "mcp"], cwd=REPO, env=environment,
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        output, _ = process.communicate("".join(json.dumps(item) + "\n" for item in messages),
                                        timeout=90)
        require(process.returncode == 0, "Native MCP process failed")
        responses = [json.loads(line) for line in output.splitlines() if line.strip()]
        response = next(item for item in responses if item.get("id") == 2)
        require("error" not in response, "Native MCP rejected the tool request")
        return response["result"]
    finally:
        stop(process)


def verified_report(response, expected_status):
    report = response.get("structuredContent") if isinstance(response, dict) else None
    require(isinstance(report, dict), "Native MCP response lacks a structured execution report")
    verification = report.get("receipt_verification")
    verification = verification if isinstance(verification, dict) else {}
    if report.get("status") != expected_status:
        # Keep the execution boundary visible without echoing raw errors,
        # input, results or credentials from the MCP response.
        def label(value):
            return value if isinstance(value, str) and re.fullmatch(r"[a-z][a-z0-9_]{0,63}", value) else "missing_or_invalid"

        details = ["expected=" + label(expected_status), "status=" + label(report.get("status")),
                   "receipt_verified=" + str(verification.get("verified") is True)]
        for field, value in (("stage", report.get("stage")),
                             ("failure_code", verification.get("failure_code"))):
            if value is not None:
                details.append(field + "=" + label(value))
        limits = report.get("execution_limits")
        if isinstance(limits, dict) and type(limits.get("max_runtime_ms")) is int:
            details.append("max_runtime_ms=" + str(limits["max_runtime_ms"]))
        raise RuntimeError("Unexpected execution status (" + ", ".join(details) + ")")
    require(response.get("isError") == (expected_status == "failed"), "Unexpected MCP error flag")
    require(verification.get("verified") is True,
            "Native MCP did not verify receipt evidence")
    require(report["deal_id"] == report["deal_hash"], "A2A task identity differs from signed Deal")
    return report


def publish_ontology(bob, root):
    source, project = root / "bob-selected-terminology.json", root / "terminology-service"
    private_json(source, ontology_source())
    query = {"op": "select", "collection": "terminology",
             "columns": ["source", "target"], "limit": 100}
    # Bob's operator context is used only for preparation/publication. Alice
    # never receives Bob's provider-control credential or its file path.
    environment = dict(bob.env, FROGLET_DAEMON_URL=bob.url,
                       FROGLET_DATA_ROOT=str(bob.data))
    prepared_response = mcp(bob, {"action": "prepare_service", "source": str(source),
        "destination": str(project), "service_id": "demo-ontology-terminology",
        "summary": "Synthetic DEMO terminology labels; no scientific ground truth. "
                   "Alice audits exact strings under an explicit one-target policy.",
        "selection": {"terminology": ["source", "target"]}, "example_input": query,
        "research_profile": demo_profile()},
        environment)
    require(prepared_response.get("isError") is False, "Bob could not prepare selected data")
    prepared = prepared_response["structuredContent"]
    require(prepared.get("status") == "prepared" and prepared.get("public") is False,
            "Preparation unexpectedly published the source")
    preparation = json.loads((project / ".froglet/preparation.json").read_text())
    snapshot = (project / preparation["snapshot_file"]).read_bytes()
    require(PRIVATE_SENTINEL.encode() not in snapshot and b"curator_note" not in snapshot,
            "Excluded private column leaked into the selected snapshot")
    require(hashlib.sha256(snapshot).hexdigest() == prepared["snapshot_sha256"],
            "Prepared snapshot hash differs from its actual bytes")
    published_response = mcp(bob, {"action": "marketplace_publish",
        "project_dir": str(project), "host": "local"}, environment)
    require(published_response.get("isError") is False,
            "Bob could not publish the selected localhost data service")
    published = published_response["structuredContent"]
    private_json(root / "publication.json", published)
    require(published["provider_id"] == bob.identity()
            and published["public_url"].rstrip("/") == bob.url
            and published.get("marketplace_offer_url") is None
            and published.get("status_url") is None
            and published.get("local_verification") is not None,
            "Publication did not remain locally verified and localhost-only")
    profile = published["publication_revision"]["payload"]["service"]["output_schema"].get(ANNOTATION)
    require(profile == demo_profile(), "Signed publication lost or altered the research declarations")
    compatibility = preflight(profile, demo_requirements())
    require(compatibility["compatible"], "Alice's explicit research requirements are incompatible")
    return {"service_id": prepared["service_id"], "query": query,
            "research_profile": profile, "semantic_preflight": compatibility,
            "selected_fields": ["source", "target"], "omitted_fields": ["curator_note"],
            "source_sha256": prepared["source_sha256"],
            "snapshot_sha256": prepared["snapshot_sha256"],
            "offer_hash": published["offer_hash"],
            "publication_status": published["status"],
            "publication_revision": published.get("publication_revision"),
            "local_verification": published["local_verification"],
            "excluded_private_values_checked": True}


def provider_record(origin, token, report, expected_state="TASK_STATE_COMPLETED"):
    task = get_json(origin + "/a2a/v1/tasks/" + report["deal_id"], {
        "Authorization": "Bearer " + token, "A2A-Version": "1.0", "A2A-Extensions": EXTENSION})
    record = json.loads(task["artifacts"][0]["parts"][0]["data"]["payload"])
    require(task["id"] == report["deal_id"] and task["status"]["state"] == expected_state
            and record["deal"]["hash"] == report["deal_hash"]
            and record["quote"]["hash"] == report["quote_hash"]
            and record.get("result_hash") == report.get("result_hash")
            and record.get("result") == report.get("result")
            and record["receipt"]["hash"] == report["receipt_verification"]["receipt_hash"],
            "Provider Task disagrees with native verified execution report")
    return record


def execution_evidence(origin, token, report, expected_state="TASK_STATE_COMPLETED"):
    record = provider_record(origin, token, report, expected_state)
    roots = {}
    for name in ("descriptor", "offer"):
        artifact = get_json(origin + "/v1/artifacts/" + record["quote"]["payload"][name + "_hash"])
        document = artifact["document"]
        if isinstance(document, str):
            document = json.loads(document)
        require(document["hash"] == record["quote"]["payload"][name + "_hash"],
                "Public root artifact differs from the signed Quote")
        roots[name] = document
    return {"deal_id": report["deal_id"], "result_hash": report.get("result_hash"),
            "execution_limits": report["execution_limits"],
            "receipt_verification": report["receipt_verification"],
            "signed_artifacts": dict(roots, quote=record["quote"], deal=record["deal"],
                                     receipt=record["receipt"])}


def recover_task(alice, bob_id, report):
    recovered = verified_report(mcp(alice, {"action": "get_task",
        "task_id": report["deal_id"], "provider_id": bob_id,
        "response_format": "compact"}), "succeeded")
    for field in ("deal_id", "deal_hash", "result_hash", "receipt_verification", "result"):
        require(recovered[field] == report[field], "Native recovery changed signed evidence")


def counts(node):
    with closing(sqlite3.connect(node.data.joinpath("node.db").as_uri() + "?mode=ro", uri=True)) as db:
        return {table: db.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
                for table in ("deals", "jobs", "requester_deals")}


def write_report(report, output):
    encoded = json.dumps(report, indent=2) + "\n"
    if output:
        output.write_text(encoded, encoding="utf-8")
    print(encoded, end="", flush=True)


def run(binary, module, keep_state, serve=False, output=None, scenario="adder"):
    root = Path(tempfile.mkdtemp(prefix="froglet-local-a2a-demo-")).resolve()
    root.chmod(0o700)
    bob, alice = Node(binary, root, "bob", "provider"), Node(binary, root, "alice", "runtime")
    succeeded = False
    try:
        alice_id, bob_id = alice.identity(), bob.identity()
        require(alice_id != bob_id, "Alice and Bob must use independent signing keys")
        bob.start()
        origin = bob.url
        catalog = publish_ontology(bob, root) if scenario == "ontology" else None
        offers = get_json(origin + "/v1/provider/offers")["offers"]
        offer = next(offer for offer in offers
                     if offer["payload"]["offer_id"] == "execute.compute"
                     and offer["payload"]["offer_kind"] == "compute.wasm.v1"
                     and offer["payload"]["settlement_method"] == "none"
                     and offer["payload"]["price_schedule"] == {
                         "base_fee_msat": 0, "success_fee_msat": 0})
        allowed_hashes = [offer["hash"]]
        if catalog is not None:
            data_offer = next(item for item in offers if item["hash"] == catalog["offer_hash"])
            require(data_offer["payload"]["offer_id"] == catalog["service_id"]
                    and data_offer["payload"]["settlement_method"] == "none"
                    and data_offer["payload"]["price_schedule"] == {
                        "base_fee_msat": 0, "success_fee_msat": 0},
                    "Published terminology Offer is not a free data service")
            allowed_hashes.append(data_offer["hash"])
        bob.stop()
        # Reuse the exact bound origin: it is part of the signed Descriptor/Offer.
        bob.env["FROGLET_LISTEN_ADDR"] = origin.removeprefix("http://")
        token = secrets.token_urlsafe(36)
        bob_config, alice_config = bob.data / "a2a.json", alice.data / "a2a.json"
        private_json(bob_config, {"clients": [{"requester_id": alice_id,
            "token": token, "offer_hashes": allowed_hashes}]})
        private_json(alice_config, {"providers": [{"provider_url": origin,
            "token": token, "allow_loopback": True}]})
        bob.env["FROGLET_A2A_CONFIG_PATH"] = str(bob_config)
        alice.env["FROGLET_A2A_CONFIG_PATH"] = str(alice_config)
        bob.start()
        alice.start()
        require(bob.url == origin, "Bob restarted on a different origin")
        require(get_json(alice.url + "/v1/node/identity")["node_id"] == alice_id,
                "Alice runtime identity changed")
        require(set(allowed_hashes).issubset({item["hash"] for item in
                    get_json(origin + "/v1/provider/offers")["offers"]}),
                "Bob's allowed Offer changed after restart")
        card = get_json(origin + "/.well-known/agent-card.json")
        require(any(interface.get("protocolBinding") == "HTTP+JSON"
                    and interface.get("protocolVersion") == "1.0"
                    and interface.get("url") == origin + "/a2a/v1"
                    for interface in card["supportedInterfaces"]), "Unexpected A2A interface")
        require(any(item.get("uri") == EXTENSION and item.get("required") is True
                    for item in card["capabilities"]["extensions"]), "Missing required extension")
        request = {"action": "run_compute", "wasm_module_hex": module.hex(),
            "input": {"a": 6, "b": 7}, "provider_id": bob_id, "provider_url": origin,
            "idempotency_key": "alice-adder-1", "max_price_sats": 0,
            "response_format": "compact"}
        case_reports = []
        if catalog is None:
            success = verified_report(mcp(alice, request), "succeeded")
            require(success.get("result") == {"sum": 13, "product": 42}, "Incorrect adder result")
            successful_deals = 1
            changed_input = {"a": 7, "b": 5}
        else:
            # Trusted local provider metadata resolution uses Bob's public API;
            # all signed invocation/work stays in Alice's separate runtime.
            environment = dict(alice.env, FROGLET_DATA_ROOT=str(alice.data),
                FROGLET_DAEMON_URL=origin, FROGLET_PROVIDER_URL=origin,
                FROGLET_RUNTIME_URL=alice.url,
                FROGLET_RUNTIME_AUTH_TOKEN_PATH=str(alice.data / "runtime/auth.token"))
            lookup = {"action": "invoke_service", "service_id": catalog["service_id"],
                "provider_id": bob_id, "input": catalog["query"],
                "idempotency_key": "alice-ontology-table-1", "max_price_sats": 0,
                "response_format": "compact"}
            retrieved = verified_report(mcp(alice, lookup, environment), "succeeded")
            result = retrieved["result"]
            require(result.get("collection") == "terminology"
                    and result.get("columns") == ["source", "target"]
                    and result.get("returned") == 5 and result.get("has_more") is False,
                    "Alice did not retrieve the selected terminology snapshot")
            require(retrieved["result_hash"] == catalog["local_verification"]["result_hash"],
                    "Alice's returned data differs from Bob's locally verified publication example")
            require(PRIVATE_SENTINEL not in json.dumps(result)
                    and "curator_note" not in json.dumps(result),
                    "Excluded private column leaked into Alice's retrieved data")
            catalog["rows"] = result["rows"]
            validate_rows(catalog["research_profile"], "terminology", result["rows"])
            catalog["semantic_preflight"]["retrieved_row_structure_checked"] = True
            catalog["retrieval"] = dict(execution_evidence(origin, token, retrieved),
                                          action="invoke_service", input=lookup["input"], result=result)
            lookup_replay = verified_report(mcp(alice, lookup, environment), "succeeded")
            require(lookup_replay["deal_id"] == retrieved["deal_id"]
                    and lookup_replay["result_hash"] == retrieved["result_hash"]
                    and counts(bob)["deals"] == 1,
                    "Terminology exact retry created additional work")
            catalog["exact_retry"] = True
            lookup_conflict = mcp(alice, dict(lookup, input=dict(lookup["input"], limit=1)), environment)
            private_json(root / "lookup-conflict.json", lookup_conflict)
            require(lookup_conflict.get("isError") is True
                    and "invocation_conflict" in lookup_conflict["structuredContent"].get("error", "")
                    and counts(bob)["deals"] == 1,
                    "Changed data lookup under the same retry key was not refused")
            catalog["changed_input_refused"] = True
            for case in ontology_cases(result["rows"]):
                request = dict(request, input=case["input"],
                               idempotency_key="alice-ontology-" + case["name"] + "-1")
                success = verified_report(mcp(alice, request), "succeeded")
                oracle = mapping_oracle(case["input"])
                require(success["result"] == oracle, "Wasm mapping result differs from Python oracle")
                case_reports.append(dict(case, result=success["result"], oracle_result=oracle,
                    oracle_matched=True, **execution_evidence(origin, token, success)))
            successful_deals = 1 + len(case_reports)
            changed_input = dict(request["input"],
                observed_terms=request["input"]["observed_terms"] + ["DEMO:sample.changed"])
        replay = verified_report(mcp(alice, request), "succeeded")
        require(replay["deal_id"] == success["deal_id"]
                and replay["result_hash"] == success["result_hash"], "Exact retry changed work")
        replay_counts = counts(bob)
        require(replay_counts["deals"] == successful_deals and replay_counts["jobs"] == 0,
                "Exact retry created additional provider work")
        conflict = mcp(alice, dict(request, input=changed_input))
        require(conflict.get("isError") is True
                and "idempotency key reused" in conflict["structuredContent"].get("error", ""),
                "Changed input with the same key was not refused")
        require(counts(bob)["deals"] == successful_deals, "Refused changed input created another Deal")
        failure = verified_report(mcp(alice, dict(request, wasm_module_hex=LOOP_HEX,
            input=None, idempotency_key="alice-fuel-stop-1")), "failed")
        require(failure["receipt_verification"].get("failure_code") == "execution_limit_exceeded",
                "Nonterminating program was not stopped by its execution limit")
        require(failure["receipt_verification"]["limits_applied"] == failure["execution_limits"],
                "Failure receipt limits differ from signed Quote")

        bob.stop()
        bob.start()
        recover_task(alice, bob_id, success)
        provider_record(origin, token, success)
        if catalog is not None:
            recover_task(alice, bob_id, retrieved)
            require(execution_evidence(origin, token, retrieved) == {
                key: catalog["retrieval"][key] for key in ("deal_id", "result_hash",
                    "execution_limits", "receipt_verification", "signed_artifacts")},
                    "Bob's restarted data service changed signed evidence")
            lookup_restarted = verified_report(mcp(alice, lookup, environment), "succeeded")
            require(lookup_restarted["deal_id"] == retrieved["deal_id"]
                    and lookup_restarted["result_hash"] == retrieved["result_hash"],
                    "Terminology retry after restart changed signed evidence")
            catalog["provider_restart_recovery"] = True
            for case in case_reports:
                require(execution_evidence(origin, token, dict(case,
                    deal_hash=case["signed_artifacts"]["deal"]["hash"],
                    quote_hash=case["signed_artifacts"]["quote"]["hash"])) == {
                    key: case[key] for key in ("deal_id", "result_hash", "execution_limits",
                        "receipt_verification", "signed_artifacts")},
                        "Bob's restarted compute result changed signed evidence")
        require(counts(bob)["deals"] == successful_deals + 1
                and counts(alice)["requester_deals"] == successful_deals + 1,
                "Recovery created unexpected work")
        report = {"qualification": "Free localhost run with operator-configured counterparties; "
            "no real-money payments, Tor, public federation or independent result-quality proof.",
            "transport": "native MCP stdio → Alice requester runtime → A2A 1.0 HTTP+JSON → Bob",
            "alice_id": alice_id, "bob_id": bob_id, "offer_hash": offer["hash"],
            "input": request["input"], "result": success["result"],
            "deal_id": success["deal_id"], "result_hash": success["result_hash"],
            "execution_limits": success["execution_limits"],
            "verification_component": "Froglet requester runtime and native MCP CLI",
            "receipt_verification": success["receipt_verification"],
            "exact_retry_provider_counts": replay_counts,
            "changed_input_refused": True, "provider_restart_recovery": True,
            "bounded_failure": {"deal_id": failure["deal_id"],
                "status": failure["status"], "error": failure.get("error"),
                "receipt_verification": failure["receipt_verification"]},
            "final_provider_counts": counts(bob), "final_requester_counts": counts(alice)}
        if catalog is not None:
            report.update({"scenario": "ontology", "catalog": catalog, "cases": case_reports,
                "compute_module": {"sha256": hashlib.sha256(module).hexdigest(),
                    "byte_length": len(module), "abi": "froglet.wasm.run_json.v1"},
                "qualification": "Synthetic illustrative DEMO labels and exact-string matching "
                    "under Alice's one-target policy. Multiple targets indicate policy ambiguity, "
                    "not scientific incorrectness. Free localhost operator-configured execution; "
                    "no real-money settlement, Tor or public federation.",
                "oracle": "Independent Python recomputation from actually retrieved rows and "
                    "each explicit request; no scientific truth or semantic equivalence proof.",
                "signed_artifacts": case_reports[-1]["signed_artifacts"]})
            report["bounded_failure"].update(execution_evidence(origin, token, failure,
                                                               "TASK_STATE_FAILED"))
            require(PRIVATE_SENTINEL not in json.dumps(report), "Private source value leaked into report")
        require(token not in json.dumps(report), "Credential appeared in output")
        for node in (alice, bob):
            for credential_path in (node.data / "runtime").glob("*.token"):
                credential = credential_path.read_text().strip()
                require(not credential or credential not in json.dumps(report),
                        "Local operator/runtime credential appeared in output")
        host_config, agent_call = root / "mcp-host.json", root / "agent-call.json"
        private_json(host_config, {"mcpServers": {"froglet-local-demo": {
            "command": str(binary), "args": ["mcp"], "env": {
                "FROGLET_DATA_ROOT": str(alice.data), "FROGLET_RUNTIME_URL": alice.url,
                "FROGLET_PROVIDER_URL": origin, "FROGLET_DAEMON_URL": origin,
                "FROGLET_RUNTIME_AUTH_TOKEN": "",
                "FROGLET_RUNTIME_AUTH_TOKEN_PATH": str(alice.data / "runtime/auth.token"),
                "NO_PROXY": "127.0.0.1,::1", "no_proxy": "127.0.0.1,::1",
                "HTTP_PROXY": "", "http_proxy": "", "HTTPS_PROXY": "", "https_proxy": "",
                "ALL_PROXY": "", "all_proxy": ""}}}})
        private_json(agent_call, {"tool": "froglet", "arguments": dict(request,
            idempotency_key="alice-agent-" + scenario + "-1"), "recovery_template": {
            "action": "get_task", "task_id": "<deal_id returned by this agent call>",
            "provider_id": bob_id, "response_format": "compact"},
            "recovery_instructions": "Replace the template task_id with the deal_id returned by your run_compute call.",
            "boundary":
            "Use a new key for different work. This local configuration works only while the demo is running."})
        host = json.loads(host_config.read_text())["mcpServers"]["froglet-local-demo"]
        # Hosts inherit their own environment; explicit generated overrides must
        # work without depending on the demo's scrubbed child environment.
        status = mcp(alice, {"action": "status"}, dict(os.environ, **host["env"]),
                     [host["command"], *host["args"]])
        require(status.get("isError") is False
                and status["structuredContent"].get("provider_url") == origin
                and status["structuredContent"].get("runtime_url") == alice.url,
                "Generated MCP host configuration did not reach the demo Nodes")
        report["host_configuration_status_checked"] = True
        if serve or keep_state:
            report["host_configuration_file"] = str(host_config)
            report["agent_call_file"] = str(agent_call)
        succeeded = True
        if serve:
            write_report(report, output)
            print("Alice and Bob are running. Private MCP configuration and call arguments "
                  "are listed above. Host settings were not modified. Press Ctrl-C to stop.",
                  file=sys.stderr, flush=True)
            while True:
                require(alice.process.poll() is None and bob.process.poll() is None,
                        "A demo node exited while serving")
                time.sleep(0.5)
        return report
    except Exception:
        succeeded = False
        raise
    finally:
        alice.stop()
        bob.stop()
        if succeeded and not keep_state:
            shutil.rmtree(root)
        else:
            print(f"Private demo state and logs retained at {root}", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--no-build", action="store_true", help="Use existing local build artifacts")
    parser.add_argument("--keep-state", action="store_true", help="Retain private state after success")
    parser.add_argument("--serve", action="store_true",
                        help="After checks, expose private MCP host config and run until Ctrl-C")
    parser.add_argument("--output", type=Path, help="Write the sanitized JSON report to this file")
    parser.add_argument("--scenario", choices=("adder", "ontology"), default="adder",
                        help="Adder by default, or publish/retrieve/audit synthetic terminology")
    parser.add_argument("--binary", type=Path,
                        help="Use this packaged froglet-node; requires --module and skips all builds")
    parser.add_argument("--module", type=Path,
                        help="Use this precompiled Wasm module; requires --binary and skips all builds")
    args = parser.parse_args()
    if bool(args.binary) != bool(args.module):
        parser.error("--binary and --module must be supplied together")
    os.umask(0o077)
    binary = args.binary.resolve() if args.binary else REPO / "target/debug" / (
        "froglet-node.exe" if os.name == "nt" else "froglet-node")
    module_name = "ontology_check" if args.scenario == "ontology" else "adder"
    module = args.module.resolve() if args.module else REPO / (
        "examples/wasm-services/target/wasm32-unknown-unknown/release") / (module_name + ".wasm")
    if not args.no_build and not args.binary:
        installed = subprocess.run(["rustup", "target", "list", "--installed"],
            capture_output=True, text=True, check=True).stdout.splitlines()
        require("wasm32-unknown-unknown" in installed,
                "Install the target first: rustup target add wasm32-unknown-unknown")
        for command in [
            ["cargo", "build", "--locked", "-p", "froglet", "--bin", "froglet-node",
             "--target-dir", str(REPO / "target")],
            ["cargo", "build", "--locked", "--release", "--target", "wasm32-unknown-unknown",
             "--manifest-path", str(REPO / "examples/wasm-services/Cargo.toml"),
             "--target-dir", str(REPO / "examples/wasm-services/target")],
        ]:
            subprocess.run(command, cwd=REPO, env=clean_environment(), check=True)
    require(binary.is_file() and module.is_file(),
            "Missing binary or Wasm module; check explicit paths or run without --no-build")
    module_bytes = module.read_bytes()
    require(module_bytes.startswith(b"\0asm\x01\0\0\0") and len(module_bytes) <= 256 * 1024,
            "Program must be a Wasm v1 module within native MCP's inline limit")
    report = run(binary, module_bytes, args.keep_state, args.serve, args.output, args.scenario)
    write_report(report, args.output)


if __name__ == "__main__":
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    try:
        main()
    except KeyboardInterrupt:
        print("Demo interrupted; all started nodes stopped.", file=sys.stderr)
        sys.exit(130)
    except (OSError, RuntimeError, subprocess.SubprocessError, StopIteration, KeyError,
            json.JSONDecodeError) as error:
        print(f"Demo failed: {error}", file=sys.stderr)
        sys.exit(1)
