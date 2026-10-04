#!/usr/bin/env python3
"""Replay an existing Wasm program against explicit original/held-out JSON cases.

This runs no LLM, compiler, installer, public host or payment backend, and never
reads agent-host configuration. It is deterministic replay of caller-supplied
program bytes, not qualification of a fresh agent's ability to create software.
Python standard library only. Native MCP, execution and signing remain in the
explicitly pinned froglet-node binary.
"""
from __future__ import annotations

import argparse
from contextlib import closing
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import sqlite3
import sys
import tempfile
import time
import traceback

if __package__:
    from . import a2a_compute_demo as helpers
    from .research_profile import ANNOTATION, validate_profile, validate_rows
else:
    import a2a_compute_demo as helpers
    from research_profile import ANNOTATION, validate_profile, validate_rows

FIELDS = ["record_id", "batch_id", "signal_milliunits", "qc_status"]
SCOPE = ("Deterministic facilitator replay of an already-generated Wasm program on two "
         "independently keyed localhost nodes. No fresh LLM session, compiler invocation, "
         "agent-host configuration access, independent human, fresh OS, public service, "
         "paid settlement, scientific truth or physical-execution attestation is established.")


def utc():
    return datetime.now(timezone.utc).isoformat()


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def load_json(path):
    def unique(pairs):
        value = {}
        for key, item in pairs:
            if key in value:
                raise ValueError("Duplicate JSON object key")
            value[key] = item
        return value
    raw = Path(path).read_bytes()
    require(len(raw) <= 4 * 1024 * 1024, "Input document exceeds four MiB")
    return json.loads(raw, object_pairs_hook=unique,
        parse_constant=lambda _value: (_ for _ in ()).throw(ValueError("Nonfinite JSON number")))


def write_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    path.chmod(0o600)


def exact_json(left, right):
    return json.dumps(left, sort_keys=True, separators=(",", ":"), allow_nan=False) == json.dumps(
        right, sort_keys=True, separators=(",", ":"), allow_nan=False)


def verified_native_response(response, expected_status, read=False):
    report = response["structuredContent"]
    require(report.get("status") == expected_status, "Unexpected native execution status")
    # get_task successfully reads a failed operation with isError=false. An
    # execution call itself reports its failed result with isError=true.
    require(response.get("isError") is (False if read else expected_status == "failed"),
            "Unexpected native MCP call/read error flag")
    require(report.get("receipt_verification", {}).get("verified") is True,
            "Native receipt verification failed")
    require(isinstance(report.get("deal_id"), str) and report["deal_id"], "Native operation ID missing")
    require(re.fullmatch(r"[0-9a-f]{64}", report.get("deal_hash", "")) is not None,
            "Signed Deal hash missing")
    # The HTTP API may use an opaque operation ID; A2A uses a signed Deal hash.
    # Independent verification binds the actual envelope hashes, not ID equality.
    return report


def verified_cached_task(response, prior):
    report = verified_native_response(response, prior["status"], read=True)
    for field in ("deal_id", "deal_hash", "quote_hash", "workload_hash", "status", "result_hash",
                  "result", "execution_limits", "receipt_verification"):
        require(exact_json(report.get(field), prior.get(field)),
                "Provider-stopped recovery changed " + field)
    return report


def pinned_file(path, expected, kind, maximum):
    require(re.fullmatch(r"[0-9a-f]{64}", expected or "") is not None, kind + " SHA-256 pin is required")
    path = Path(path)
    require(path.is_file() and not path.is_symlink() and path.stat().st_size <= maximum,
            kind + " must be a bounded regular nonsymlink file")
    path = path.resolve(strict=True)
    require(sha(path) == expected, kind + " bytes differ from explicit SHA-256 pin")
    return path


def cases_from_files(original, original_expected, held, expected):
    input_cases, expected_cases = held["cases"], expected["cases"]
    require(isinstance(input_cases, list) and isinstance(expected_cases, list), "Case lists are required")
    expected_by_label = {}
    for case in expected_cases:
        label = case.get("label")
        require(isinstance(label, str) and label not in expected_by_label
                and "expected_result" in case, "Expected labels must be unique")
        expected_by_label[label] = case["expected_result"]
    original_answer = original_expected.get("expected_result", original_expected) if isinstance(original_expected, dict) else original_expected
    result = [{"label": "original", "input": original, "expected_result": original_answer}]
    labels = {"original"}
    for case in input_cases:
        label = case.get("label")
        require(isinstance(label, str) and re.fullmatch(r"[a-z0-9][a-z0-9-]{0,63}", label)
                and label not in labels and "input" in case,
                "Case labels must be unique safe names, excluding original")
        require(label in expected_by_label, "Held-out input has no expected answer")
        labels.add(label)
        result.append({"label": label, "input": case["input"], "expected_result": expected_by_label[label]})
    require(set(expected_by_label) == labels - {"original"}, "Expected/input case labels differ")
    require(len(result) <= 6, "At most original plus five held-out cases fit this bounded replay")
    return result


def selected_source(raw):
    require(isinstance(raw, dict) and isinstance(raw.get("assay_readouts"), list),
            "Optional source must contain an assay_readouts array")
    rows = raw["assay_readouts"]
    require(rows and all(isinstance(row, dict) and set(FIELDS) <= set(row) for row in rows),
            "Assay source rows must contain the four explicit selected fields")
    return {"assay_readouts": rows}, [{field: row[field] for field in FIELDS} for row in rows]


def own_runtime_url(node):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        require(node.process.poll() is None, "Dual provider exited before its runtime listener was available")
        match = re.search(r"Local Runtime API: (http://127\.0\.0\.1:\d+)", node.log.read_text(encoding="utf-8", errors="replace"))
        if match:
            result = match.group(1)
            require(result != node.url and helpers.get_json(result + "/health").get("status") == "ok",
                    "Dual provider's actual runtime health failed")
            return result
        time.sleep(0.1)
    raise RuntimeError("Dual provider runtime listener discovery timed out")


def publish_selected(bob, private, output, source, original, profile):
    raw = load_json(source)
    normalized, selected = selected_source(raw)
    require(exact_json(original, {"rows": selected}), "Original replay input differs from exact source projection")
    normalized_path, project = private / "normalized-source.json", private / "assay-service"
    helpers.private_json(normalized_path, normalized)
    query = {"op": "select", "collection": "assay_readouts", "columns": FIELDS, "limit": 100}
    require(len(selected) <= 100, "Source exceeds this replay's complete one-page selection")
    environment = dict(bob.env, FROGLET_DAEMON_URL=bob.url, FROGLET_PROVIDER_URL=bob.url,
                       FROGLET_RUNTIME_URL=own_runtime_url(bob),
                       FROGLET_RUNTIME_AUTH_TOKEN_PATH=str(bob.data / "runtime/auth.token"),
                       FROGLET_PROVIDER_CONTROL_TOKEN_PATH=str(bob.data / "runtime/froglet-control.token"),
                       FROGLET_DATA_ROOT=str(bob.data), FROGLET_DATA_DIR=str(bob.data))
    request = {"action": "prepare_service", "source": str(normalized_path), "destination": str(project),
               "service_id": "replay-assay-readouts", "summary": "Synthetic assay rows for deterministic program replay; no scientific claim.",
               "selection": {"assay_readouts": FIELDS}, "example_input": query}
    if profile:
        validate_profile(profile)
        validate_rows(profile, "assay_readouts", selected)
        request["research_profile"] = profile
    prepared_response = helpers.mcp(bob, request, environment)
    write_json(output / "preparation-response.json", prepared_response)
    require(prepared_response.get("isError") is False, "Native selected-data preparation failed")
    prepared = prepared_response["structuredContent"]
    require(prepared.get("status") == "prepared" and prepared.get("public") is False,
            "Preparation did not remain private")
    preparation = load_json(project / ".froglet/preparation.json")
    snapshot = project / preparation["snapshot_file"]
    require(sha(snapshot) == prepared["snapshot_sha256"], "Actual snapshot differs from its preparation pin")
    snapshot_value = load_json(snapshot)
    require(exact_json(snapshot_value, {"assay_readouts": selected}), "Snapshot differs from selected source projection")
    shutil.copyfile(snapshot, output / "prepared-snapshot-actual.json")
    (output / "prepared-snapshot-actual.json").chmod(0o600)
    require(sha(output / "prepared-snapshot-actual.json") == prepared["snapshot_sha256"], "Snapshot capture changed its actual bytes")
    publication_response = helpers.mcp(bob, {"action": "marketplace_publish", "project_dir": str(project), "host": "local"}, environment)
    write_json(output / "publication-response.json", publication_response)
    require(publication_response.get("isError") is False, "Native local publication failed")
    publication = publication_response["structuredContent"]
    require(publication.get("status") == "local_verified" and publication["provider_id"] == bob.identity()
            and publication["public_url"].rstrip("/") == bob.url
            and publication.get("marketplace_offer_url") is None and publication.get("status_url") is None,
            "Publication escaped verified local scope")
    if profile:
        require(publication["publication_revision"]["payload"]["service"]["output_schema"].get(ANNOTATION) == profile,
                "Signed publication changed explicit research declarations")
    write_json(output / "publication.json", publication)
    canary_request = {"action": "invoke_service", "service_id": "replay-assay-readouts",
        "provider_id": publication["provider_id"], "input": query,
        "idempotency_key": "program-replay-facilitator-owner-local-canary", "max_price_sats": 0,
        "response_format": "compact"}
    canary_response = helpers.mcp(bob, canary_request, environment)
    write_json(output / "facilitator-owner-local-canary-response.json", canary_response)
    canary = verified_native_response(canary_response, "succeeded")
    require(exact_json(canary["result"]["rows"], selected)
            and canary["result_hash"] == publication["local_verification"]["result_hash"],
            "Explicit facilitator canary differs from verified publication example")
    return publication, query


def export_deals(bob, output):
    """Export all explicit records before roots; never depend on scoped A2A reads."""
    directory = output / "deals"
    directory.mkdir(exist_ok=True)
    with closing(sqlite3.connect((bob.data / "node.db").as_uri() + "?mode=ro", uri=True)) as db:
        db.row_factory = sqlite3.Row
        rows = db.execute("SELECT deal_id,idempotency_key,service_id,workload_hash,spec_json,quote_json,deal_artifact_json,status,result_json,result_hash,error,receipt_artifact_json,created_at,updated_at FROM deals ORDER BY created_at,deal_id").fetchall()
    for row in rows:
        write_json(directory / (row["deal_id"] + ".json"), dict(row))
    index = []
    for row in rows:
        entry = dict(row)
        path = directory / (entry["deal_id"] + ".json")
        entry["export_status"] = {"sql_record_saved": True, "full_signed_chain_exported": False}
        try:
            for field in ("spec_json", "quote_json", "deal_artifact_json", "result_json", "receipt_artifact_json"):
                value = entry.pop(field)
                entry[field[:-5]] = json.loads(value) if value is not None else None
            entry["provider_id"] = entry["quote"]["payload"]["provider_id"]
            entry["requester_id"] = entry["quote"]["payload"]["requester_id"]
            entry["workload_kind"] = entry["quote"]["payload"]["workload_kind"]
            write_json(path, entry)
            require(entry["receipt_artifact"] is not None, "No terminal Receipt stored")
            roots = {}
            for name in ("descriptor", "offer"):
                expected = entry["quote"]["payload"][name + "_hash"]
                document = helpers.get_json(bob.url + "/v1/artifacts/" + expected)["document"]
                document = json.loads(document) if isinstance(document, str) else document
                require(document["hash"] == expected, "Public root differs from Quote binding")
                roots[name] = document
                entry["root_artifacts"] = dict(roots)
                write_json(path, entry)
            entry["signed_artifacts"] = dict(roots, quote=entry["quote"], deal=entry["deal_artifact"], receipt=entry["receipt_artifact"])
            entry["export_status"]["full_signed_chain_exported"] = True
        except (OSError, RuntimeError, ValueError, KeyError) as error:
            entry["export_error"] = {"type": type(error).__name__, "message": str(error)}
        write_json(path, entry)
        index.append({"deal_id": entry["deal_id"], "path": str(path), "sha256": sha(path),
                      "signed_deal_hash": entry.get("deal_artifact", {}).get("hash"),
                      "status": entry["status"], "requester_id": entry.get("requester_id"),
                      "workload_kind": entry.get("workload_kind"), "export_status": entry["export_status"]})
    write_json(output / "deal-index.json", index)
    return index


def case_record(report, arguments, signed):
    return {"label": arguments["idempotency_key"].removeprefix("program-replay-"), "input": arguments["input"],
            "result": report.get("result"), "deal_id": report["deal_id"], "deal_hash": report["deal_hash"],
            "result_hash": report.get("result_hash"),
            "signed_artifacts": signed, "execution_limits": report["execution_limits"],
            "receipt_verification": report["receipt_verification"]}


def by_signed_deal_hash(records):
    result = {}
    for record in records:
        signed_hash = record["deal_artifact"]["hash"]
        require(signed_hash not in result, "Multiple provider records claim the same signed Deal")
        result[signed_hash] = record
    return result


def replay(args):
    binary = pinned_file(args.binary, args.binary_sha256, "Binary", 256 * 1024 * 1024)
    require(os.access(binary, os.X_OK), "Pinned binary is not executable")
    module = pinned_file(args.module, args.module_sha256, "Module", 256 * 1024)
    require(module.read_bytes().startswith(b"\0asm\x01\0\0\0"), "Module is not Wasm v1")
    cases = cases_from_files(load_json(args.original_input), load_json(args.original_expected),
                            load_json(args.held_out_inputs), load_json(args.held_out_expected))
    output = args.output_dir.resolve()
    require(not output.exists(), "Use a new evidence directory; earlier failures must survive")
    output.mkdir(parents=True, mode=0o700)
    inputs = [args.original_input, args.original_expected, args.held_out_inputs, args.held_out_expected]
    if args.source:
        inputs.append(args.source)
    if args.research_profile:
        require(args.source is not None, "Research declaration needs an explicit selected source")
        inputs.append(args.research_profile)
    pins = {str(path.resolve()): sha(path) for path in inputs}
    report = {"schema_version": "froglet.generated-program-replay.v1", "status": "running", "scope": SCOPE,
              "started_at": utc(), "binary_sha256": args.binary_sha256, "module_sha256": args.module_sha256,
              "module_bytes": module.stat().st_size, "input_file_sha256": pins,
              "llm_sessions_started": 0, "agent_host_configuration_accessed": False,
              "compiler_invoked": False, "independent_verification": "not yet run"}
    private = Path(tempfile.mkdtemp(prefix="froglet-program-replay-")).resolve()
    private.chmod(0o700)
    helpers.REPO = private
    binary_copy, module_copy = private / "froglet-node", output / "retained-module.wasm"
    shutil.copy2(binary, binary_copy)
    shutil.copyfile(module, module_copy)
    module_copy.chmod(0o600)
    require(sha(binary_copy) == args.binary_sha256 and sha(module_copy) == args.module_sha256, "Pinned file copy changed bytes")
    bob, alice = helpers.Node(binary_copy, private, "bob", "provider"), helpers.Node(binary_copy, private, "alice", "runtime")
    # Keep the helper's provider-listener parsing; the process itself provides
    # both actual APIs, and publication receives this node's own runtime URL.
    bob.env["FROGLET_NODE_ROLE"] = "dual"
    exported = False
    started = time.monotonic()
    records = {}
    evidence = {"schema_version": "froglet.fresh-journey.evidence.v1", "qualification": SCOPE,
                "compute_cases": [], "additional_records": []}
    try:
        bob_id, alice_id = bob.identity(), alice.identity()
        require(bob_id != alice_id, "Independent replay identities collided")
        evidence.update(provider_id=bob_id, requester_id=alice_id)
        report.update(provider_id=bob_id, requester_id=alice_id)
        bob.start()
        origin = bob.url
        profile = load_json(args.research_profile) if args.research_profile else None
        publication, query = publish_selected(bob, private, output, args.source, cases[0]["input"], profile) if args.source else (None, None)
        offers = helpers.get_json(origin + "/v1/provider/offers")["offers"]
        compute_offer = next(item for item in offers if item["payload"]["offer_id"] == "execute.compute"
                             and item["payload"]["offer_kind"] == "compute.wasm.v1")
        allowed = [compute_offer["hash"]] + ([publication["offer_hash"]] if publication else [])
        for offer in (item for item in offers if item["hash"] in allowed):
            require(offer["payload"]["settlement_method"] == "none" and offer["payload"]["price_schedule"] == {"base_fee_msat": 0, "success_fee_msat": 0}, "Replay permits free work only")
        bob.stop()
        bob.env["FROGLET_LISTEN_ADDR"] = origin.removeprefix("http://")
        token = secrets.token_urlsafe(36)
        helpers.private_json(bob.data / "a2a.json", {"clients": [{"requester_id": alice_id, "token": token, "offer_hashes": allowed}]})
        helpers.private_json(alice.data / "a2a.json", {"providers": [{"provider_url": origin, "token": token, "allow_loopback": True}]})
        bob.env["FROGLET_A2A_CONFIG_PATH"] = str(bob.data / "a2a.json")
        alice.env["FROGLET_A2A_CONFIG_PATH"] = str(alice.data / "a2a.json")
        bob.start()
        alice.start()
        require(bob.url == origin, "Transport setup changed signed provider origin")
        environment = dict(alice.env, FROGLET_DAEMON_URL=origin, FROGLET_PROVIDER_URL=origin,
            FROGLET_DATA_ROOT=str(alice.data), FROGLET_DATA_DIR=str(alice.data), FROGLET_RUNTIME_URL=alice.url,
            FROGLET_RUNTIME_AUTH_TOKEN_PATH=str(alice.data / "runtime/auth.token"))
        if publication:
            request = {"action": "invoke_service", "service_id": "replay-assay-readouts", "provider_id": bob_id,
                       "input": query, "idempotency_key": "program-replay-retrieval", "max_price_sats": 0, "response_format": "compact"}
            response = helpers.mcp(alice, request, environment)
            write_json(output / "retrieval-response.json", response)
            retrieved = verified_native_response(response, "succeeded")
            require(retrieved["result"].get("has_more") is False and exact_json({"rows": retrieved["result"]["rows"]}, cases[0]["input"]), "Returned data differs from exact original input")
            if profile:
                validate_rows(profile, "assay_readouts", retrieved["result"]["rows"])
            records[retrieved["deal_id"]] = ("retrieval", request, retrieved)
            evidence["publication_revision"] = publication["publication_revision"]
            evidence["retrieval"] = case_record(retrieved, request, {})
        for case in cases:
            request = {"action": "run_compute", "wasm_module_path": str(module_copy), "input": case["input"],
                       "provider_id": bob_id, "provider_url": origin, "idempotency_key": "program-replay-" + case["label"],
                       "max_price_sats": 0, "response_format": "compact"}
            response = helpers.mcp(alice, request, environment)
            write_json(output / (case["label"] + "-response.json"), response)
            actual = verified_native_response(response, "succeeded")
            records[actual["deal_id"]] = (case["label"], request, actual)
            require(exact_json(actual["result"], case["expected_result"]), "Program differs from explicit expected answer: " + case["label"])
            evidence["compute_cases"].append(case_record(actual, request, {}))
        first = evidence["compute_cases"][0]
        first_request = records[first["deal_id"]][1]
        before_retry = helpers.counts(bob)
        retry = verified_native_response(helpers.mcp(alice, first_request, environment), "succeeded")
        require(retry["deal_id"] == first["deal_id"] and helpers.counts(bob) == before_retry, "Exact retry created new work")
        write_json(output / "exact-retry-report.json", retry)
        changed = dict(first_request, input={"rows": []} if first_request["input"] != {"rows": []} else {"rows": [None]})
        refusal = helpers.mcp(alice, changed, environment)
        write_json(output / "changed-input-refusal.json", refusal)
        require(refusal.get("isError") is True and "idempotency key reused" in refusal["structuredContent"].get("error", "")
                and helpers.counts(bob) == before_retry, "Changed work was not refused before admission")
        failure_request = dict(first_request, wasm_module_path=None, wasm_module_hex=helpers.LOOP_HEX,
                               input=None, idempotency_key="program-replay-execution-budget-failure")
        del failure_request["wasm_module_path"]
        failure_response = helpers.mcp(alice, failure_request, environment)
        write_json(output / "execution-budget-failure-response.json", failure_response)
        failure = verified_native_response(failure_response, "failed")
        require(failure["receipt_verification"].get("failure_code") == "execution_limit_exceeded"
                and failure["receipt_verification"].get("limits_applied") == failure["execution_limits"],
                "Loop was not stopped with signed applied limits")
        records[failure["deal_id"]] = ("execution-budget-failure", failure_request, failure)
        recovery = []
        bob.stop()
        bob.start()
        require(bob.url == origin, "Recovery restart changed provider origin")
        for deal_id, (label, _request, prior) in records.items():
            response = helpers.mcp(alice, {"action": "get_task", "task_id": deal_id, "provider_id": bob_id,
                                          "response_format": "compact"}, environment)
            write_json(output / (label + "-recovery-response.json"), response)
            recovered = verified_native_response(response, prior["status"], read=True)
            for field in ("deal_id", "deal_hash", "result_hash", "result", "receipt_verification"):
                require(recovered.get(field) == prior.get(field), "Restart recovery changed signed work: " + label)
            recovery.append({"label": label, "deal_id": deal_id, "same_result_and_evidence": True, "status": recovered["status"]})
        index = export_deals(bob, output)
        exported = bool(index) and all(item["export_status"]["full_signed_chain_exported"] for item in index)
        require(exported, "Full-chain export incomplete; stopped private state must be retained")
        stored_by_hash = by_signed_deal_hash(load_json(item["path"]) for item in index)
        for record in evidence["compute_cases"] + ([evidence["retrieval"]] if "retrieval" in evidence else []):
            record["signed_artifacts"] = stored_by_hash[record["deal_hash"]]["signed_artifacts"]
        already = {record["deal_hash"] for record in evidence["compute_cases"]}
        if "retrieval" in evidence:
            already.add(evidence["retrieval"]["deal_hash"])
        for signed_hash, stored in stored_by_hash.items():
            if signed_hash in already:
                continue
            spec = stored["spec"]
            actual_input = spec["submission"]["input"] if spec["kind"] == "wasm" else spec["execution"]["input"]
            label = records[signed_hash][0] if signed_hash in records else "facilitator-owner-local-publication-canary-" + stored["deal_id"][:12]
            evidence["additional_records"].append({"label": label, "provider_id": stored["provider_id"],
                "requester_id": stored["requester_id"], "outcome": stored["status"], "input": actual_input,
                "result": stored["result"], "workload_spec": spec, "signed_artifacts": stored["signed_artifacts"]})
        write_json(output / "evidence.json", evidence)
        offline_before = {"provider": helpers.counts(bob), "requester": helpers.counts(alice)}
        bob.stop()
        require(bob.process.poll() is not None, "Provider process remained running during offline recovery")
        runtime_origin = alice.url
        alice.stop()
        alice.env["FROGLET_RUNTIME_LISTEN_ADDR"] = runtime_origin.removeprefix("http://")
        alice.start()
        require(alice.url == runtime_origin and alice.identity() == alice_id,
                "Requester restart changed its original identity or runtime origin")
        offline_response = helpers.mcp(alice, {"action": "get_task", "task_id": first["deal_id"],
                                             "provider_id": bob_id, "response_format": "compact"}, environment)
        write_json(output / "provider-stopped-recovery-response.json", offline_response)
        verified_cached_task(offline_response, records[first["deal_id"]][2])
        offline_recovery = []
        for deal_id, (label, _request, prior) in records.items():
            response = offline_response if deal_id == first["deal_id"] else helpers.mcp(alice,
                {"action": "get_task", "task_id": deal_id, "provider_id": bob_id,
                 "response_format": "compact"}, environment)
            write_json(output / (label + "-provider-stopped-recovery-response.json"), response)
            recovered = verified_cached_task(response, prior)
            offline_recovery.append({"label": label, "deal_id": deal_id, "status": recovered["status"],
                                     "same_result_and_evidence": True})
        offline_read = {"available": True, "scope": "Every saved terminal task is revalidated from its signed durable chain without contacting the stopped provider",
                        "records": offline_recovery}
        offline_after = {"provider": helpers.counts(bob), "requester": helpers.counts(alice)}
        require(offline_after == offline_before, "Provider-stopped recovery created new work")
        offline_retry_response = helpers.mcp(alice, first_request, environment)
        write_json(output / "provider-stopped-exact-compute-retry-response.json", offline_retry_response)
        offline_retry = verified_native_response(offline_retry_response, "succeeded")
        require(offline_retry["deal_id"] == first["deal_id"] and offline_retry.get("result") == first["result"]
                and helpers.counts(bob) == offline_before["provider"] and helpers.counts(alice) == offline_before["requester"],
                "Provider-stopped exact compute retry changed admitted work")
        if publication:
            retrieved_prior = records[evidence["retrieval"]["deal_id"]]
            offline_data_response = helpers.mcp(alice, retrieved_prior[1], environment)
            write_json(output / "provider-stopped-exact-data-retry-response.json", offline_data_response)
            offline_data = verified_native_response(offline_data_response, "succeeded")
            require(offline_data["deal_id"] == retrieved_prior[2]["deal_id"]
                    and exact_json(offline_data["result"], retrieved_prior[2]["result"])
                    and helpers.counts(bob) == offline_before["provider"] and helpers.counts(alice) == offline_before["requester"],
                    "Provider-stopped exact data invocation retry changed admitted work")
        write_json(output / "provider-stopped-recovery-check.json", {"provider_process_exit_code": bob.process.returncode,
            "requester_runtime_running": alice.process.poll() is None, "get_task": offline_read,
            "exact_retries_return_same_result_and_signed_evidence": True,
            "requester_restarted_preserving_identity_origin": True, "exact_compute_retry_same_deal": True,
            "exact_data_retry_same_deal": bool(publication),
            "counts_before": offline_before, "counts_after": offline_after,
            "scope": "Bob's provider process is stopped; Alice's requester runtime was restarted and reads its durable terminal record and exact compute retry."})
        report.update(status="runtime_pass_pending_independent_verification", compute_cases_passed=len(cases),
            signed_execution_budget_failure=True, exact_retry_same_deal=True, changed_input_refused_before_deal=True,
            get_task_with_provider_stopped=offline_read,
            requester_restart_then_offline_recovery=True, exact_compute_retry_with_provider_stopped=True,
            exact_data_retry_with_provider_stopped=bool(publication),
            restart_recovery=recovery, provider_counts=helpers.counts(bob), requester_counts=helpers.counts(alice),
            exported_full_chains=len(index), evidence_sha256=sha(output / "evidence.json"), deal_index_sha256=sha(output / "deal-index.json"))
    except BaseException as error:
        report.update(status="failed", error_type=type(error).__name__, error=str(error))
        (output / "failure-traceback.txt").write_text(traceback.format_exc(), encoding="utf-8")
        try:
            if bob.process is not None and bob.process.poll() is None:
                index = export_deals(bob, output)
                exported = bool(index) and all(item["export_status"]["full_signed_chain_exported"] for item in index)
                report["partial_export_full_chains"] = sum(item["export_status"]["full_signed_chain_exported"] for item in index)
        except BaseException as export_error:
            report["partial_export_error"] = str(export_error)
    finally:
        for node in (alice, bob):
            node.stop()
        report["input_files_unchanged"] = all(sha(Path(path)) == expected for path, expected in pins.items())
        report["pinned_binary_and_module_unchanged"] = sha(binary) == args.binary_sha256 and sha(module) == args.module_sha256
        if not report["input_files_unchanged"] or not report["pinned_binary_and_module_unchanged"]:
            report.update(status="failed", error="Caller-supplied input/program bytes changed")
        secret_values = [path.read_text().strip() for path in private.rglob("*.token")]
        if "token" in locals():
            secret_values.append(token)
        logs = output / "node-logs"
        logs.mkdir(exist_ok=True)
        for path in private.rglob("*.log"):
            content = path.read_text(encoding="utf-8", errors="replace")
            for value in secret_values:
                if value:
                    content = content.replace(value, "[REDACTED]")
            (logs / (path.parent.name + "-" + path.name)).write_text(content, encoding="utf-8")
        leaks = []
        for path in output.rglob("*.json"):
            content = path.read_text(encoding="utf-8")
            if any(value and value in content for value in secret_values):
                leaks.append(str(path))
        report["credential_values_absent_in_evidence"] = not leaks
        if leaks:
            report.update(status="failed", credential_leak_files=leaks)
        report.update(nodes_stopped=True, duration_seconds=round(time.monotonic() - started, 3), finished_at=utc(),
                      full_chain_export_complete=exported, private_state_path=str(private))
        # Persist the complete report before removing the last private DB copy.
        report["private_state_removed"] = False
        write_json(output / "qualification.json", report)
        if exported and not leaks:
            shutil.rmtree(private)
            report["private_state_removed"] = not private.exists()
        else:
            private.chmod(0o700)
            report["private_state_retained_reason"] = "Incomplete export or credential leak; stopped state retained for explicit recovery"
        write_json(output / "qualification.json", report)
    return report


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    for name in ("binary", "module", "original-input", "original-expected", "held-out-inputs", "held-out-expected", "output-dir"):
        result.add_argument("--" + name, type=Path, required=True)
    for name in ("binary-sha256", "module-sha256"):
        result.add_argument("--" + name, required=True)
    result.add_argument("--source", type=Path, help="Optional private assay source to publish selected rows locally and verify original input binding")
    result.add_argument("--research-profile", type=Path, help="Optional explicit declaration JSON bound in the existing signed output_schema")
    return result


if __name__ == "__main__":
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    try:
        outcome = replay(parser().parse_args())
        print(json.dumps({key: outcome.get(key) for key in ("status", "compute_cases_passed", "exported_full_chains", "private_state_removed", "duration_seconds")}))
        raise SystemExit(1 if outcome["status"] == "failed" else 0)
    except (OSError, RuntimeError, ValueError, KeyError) as error:
        print("Replay refused: " + str(error), file=sys.stderr)
        raise SystemExit(1) from None
