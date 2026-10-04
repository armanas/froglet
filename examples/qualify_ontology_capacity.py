#!/usr/bin/env python3
"""Bounded localhost qualification, not a production user/connection limit.

Requires existing froglet-node and ontology_check.wasm artifacts. Measures
fresh native MCP processes -> independent Alice runtime -> A2A -> Bob, against
a conventional HTTP/Python service performing the same selected-data audit.
Only Python's standard library is required; no money or public deployment.
"""
from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing
from datetime import datetime, timezone
import hashlib
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import os
from pathlib import Path
import platform
import re
import secrets
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from urllib.parse import urlsplit

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from examples.a2a_compute_demo import (  # noqa: E402
    EXTENSION, PRIVATE_SENTINEL, REPO, Node, clean_environment, counts,
    execution_evidence, get_json, mapping_oracle, mcp, ontology_cases,
    private_json, publish_ontology, require, stop, verified_report,
)


def positive_int(value):
    number = int(value)
    if not 1 <= number <= 128:
        raise argparse.ArgumentTypeError("Expected an integer from 1 to 128")
    return number


def concurrency_list(value):
    try:
        values = [positive_int(item) for item in value.split(",")]
    except (ValueError, argparse.ArgumentTypeError) as error:
        raise argparse.ArgumentTypeError("Use unique increasing integers from 1 to 128") from error
    if values != sorted(set(values)):
        raise argparse.ArgumentTypeError("Concurrency must be unique and increasing")
    return values


def percentile_nearest_rank(values, fraction):
    require(values and 0 < fraction <= 1, "Percentile needs samples and a fraction in (0, 1]")
    return sorted(values)[math.ceil(len(values) * fraction) - 1]


def summarize(samples, wall_seconds, requested_concurrency, peak_active):
    require(samples and wall_seconds > 0, "Cannot summarize an empty or zero-duration phase")
    latencies = [sample["latency_ms"] for sample in samples]
    successful = sum(sample["status"] == "succeeded" for sample in samples)
    return {"requested_concurrency": requested_concurrency,
            "peak_active_workflows": peak_active, "attempted": len(samples),
            "succeeded": successful, "errors": len(samples) - successful,
            "wall_seconds": round(wall_seconds, 6),
            "successful_workflows_per_second": round(successful / wall_seconds, 3),
            "latency_ms": {"p50": round(percentile_nearest_rank(latencies, 0.50), 3),
                           "p95": round(percentile_nearest_rank(latencies, 0.95), 3),
                           "maximum": round(max(latencies), 3),
                           "definition": "nearest rank; all attempted workflows, including errors"},
            "samples": samples}


def run_phase(operation, concurrency, iterations, label):
    """Start exactly concurrency workers together; each performs finite sequential work."""
    barrier = threading.Barrier(concurrency)
    lock = threading.Lock()
    active, peak = 0, 0

    def worker(worker_index):
        nonlocal active, peak
        barrier.wait(timeout=30)
        samples = []
        for iteration in range(iterations):
            key = f"{label}-c{concurrency}-w{worker_index}-i{iteration}"
            with lock:
                active += 1
                peak = max(peak, active)
            started = time.perf_counter()
            try:
                sample = operation(key)
            except Exception as error:  # Keep actual transport failures in the measured denominator.
                sample = {"status": "error", "error_type": type(error).__name__,
                          "error": str(error)[:512]}
            finally:
                elapsed = (time.perf_counter() - started) * 1000
                with lock:
                    active -= 1
            samples.append(dict(sample, request_key=key, latency_ms=round(elapsed, 6)))
        return samples

    started = time.perf_counter()
    with ThreadPoolExecutor(max_workers=concurrency) as executor:
        futures = [executor.submit(worker, index) for index in range(concurrency)]
        samples = [sample for future in futures for sample in future.result()]
    return summarize(samples, time.perf_counter() - started, concurrency, peak)


def selected_request(rows):
    return ontology_cases(rows)[-1]["input"]


def http_json(url, value=None):
    parsed = urlsplit(url)
    require(parsed.scheme == "http" and parsed.hostname == "127.0.0.1" and parsed.port,
            "The conventional baseline must use its literal localhost HTTP origin")
    connection = HTTPConnection(parsed.hostname, parsed.port, timeout=30)
    try:
        connection.request("GET" if value is None else "POST", parsed.path,
                           body=None if value is None else json.dumps(value).encode(),
                           headers={"Content-Type": "application/json"})
        response = connection.getresponse()
        require(response.status == 200, f"Conventional baseline returned HTTP{response.status}")
        payload = response.read(128 * 1024 + 1)
    finally:
        connection.close()
    require(len(payload) <= 128 * 1024, "Baseline response exceeds the qualification limit")
    return json.loads(payload)


def baseline_server(path):
    """Deliberately conventional: selected JSON + a fixed server-side Python function."""
    rows = json.loads(path.read_text(encoding="utf-8"))
    mapping_oracle({"mappings": rows, "observed_terms": []})

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_arguments):
            pass

        def reply(self, code, value):
            encoded = json.dumps(value, separators=(",", ":")).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(encoded)))
            self.end_headers()
            self.wfile.write(encoded)

        def do_GET(self):
            if self.path != "/selected-table":
                self.reply(404, {"error": "not found"})
                return
            self.reply(200, {"rows": rows})

        def do_POST(self):
            if self.path != "/audit":
                self.reply(404, {"error": "not found"})
                return
            try:
                size = int(self.headers.get("Content-Length", "0"))
                require(0 < size <= 128 * 1024, "Invalid baseline input size")
                request = json.loads(self.rfile.read(size))
                self.reply(200, mapping_oracle(request))
            except (ValueError, RuntimeError) as error:
                self.reply(400, {"error": str(error)})

    # Avoid the stdlib's backlog 5 becoming an accidental comparison bottleneck.
    class Server(ThreadingHTTPServer):
        request_queue_size = 128
        daemon_threads = True

    server = Server(("127.0.0.1", 0), Handler)
    print(json.dumps({"url": f"http://127.0.0.1:{server.server_port}"}), flush=True)
    try:
        server.serve_forever(poll_interval=0.1)
    finally:
        server.server_close()


def hardware_details():
    details = {"platform": platform.platform(), "machine": platform.machine(),
               "python": platform.python_version(), "logical_cpu_count": os.cpu_count(),
               "load_average_before": list(os.getloadavg()) if hasattr(os, "getloadavg") else None,
               "isolation": "shared developer machine; no dedicated CPU allocation"}
    if sys.platform == "darwin":
        for name, query in (("cpu_model", "machdep.cpu.brand_string"),
                            ("physical_memory_bytes", "hw.memsize"),
                            ("physical_cpu_count", "hw.physicalcpu")):
            result = subprocess.run(["sysctl", "-n", query], text=True, capture_output=True,
                                    timeout=5)
            if result.returncode == 0:
                value = result.stdout.strip()
                details[name] = int(value) if value.isdigit() else value
    return details


def source_fingerprints():
    paths = ("examples/qualify_ontology_capacity.py", "examples/a2a_compute_demo.py",
             "src/api/mod.rs", "src/api/a2a.rs", "src/api/remote_client.rs",
             "src/deals.rs", "src/sandbox.rs", "src/provider_policy.rs")
    return {path: hashlib.sha256((REPO / path).read_bytes()).hexdigest() for path in paths}


def provider_usage(bob):
    token = (bob.data / "runtime/froglet-control.token").read_text().strip()
    return get_json(bob.url + "/v1/provider/usage", {"Authorization": "Bearer " + token})


def requester_status_counts(requesters):
    statuses = {}
    for requester in requesters:
        with closing(sqlite3.connect((requester.data / "node.db").as_uri() + "?mode=ro", uri=True)) as db:
            for status, count in db.execute("SELECT status, COUNT(*) FROM requester_deals GROUP BY status"):
                statuses[status] = statuses.get(status, 0) + count
    return statuses


def response_summary(response, expected):
    structured = response.get("structuredContent", {})
    if structured.get("status") == "succeeded":
        report = verified_report(response, "succeeded")
        require(report["result"] == expected, "Remote result differs from independent Python oracle")
        return {"status": "succeeded", "deal_id": report["deal_id"],
                "result_hash": report["result_hash"],
                "receipt_hash": report["receipt_verification"]["receipt_hash"],
                "receipt_verified": True}
    summary = {"status": "error", "error": str(structured.get("error", "Unexpected MCP response"))[:512]}
    if structured.get("deal_id"):
        summary["deal_id"] = structured["deal_id"]
    if structured.get("status") == "failed":
        report = verified_report(response, "failed")
        summary["receipt_verified"] = True
        summary["failure_code"] = report["receipt_verification"].get("failure_code")
    return summary


def worker_index_from_key(key):
    return int(key.rsplit("-w", 1)[1].split("-i", 1)[0])


def failed_invocation_task(error):
    match = re.fullmatch(r'invocation ([0-9a-f]{64}) ended in status "failed": .+', error)
    return match.group(1) if match else None


def verified_failed_task(response, task):
    # get_task succeeded in retrieving a failed task: the tool operation itself
    # is successful. run_compute's isError=True convention does not apply here.
    report = response["structuredContent"]
    require(response.get("isError") is False and report.get("status") == "failed",
            "Task recovery did not successfully retrieve the terminal failure")
    require(report.get("receipt_verification", {}).get("verified") is True,
            "Recovered terminal failure receipt was not verified")
    require(report.get("deal_id") == report.get("deal_hash") == task,
            "Recovered terminal failure has a different signed task identity")
    return report


def run_qualification(binary, module, concurrency, iterations, output, keep_state=False,
                      requester_mode="shared"):
    total_workflows = sum(concurrency) * iterations
    require(total_workflows <= 5000, "Finite qualification is capped at 5000 workflows")
    root = Path(tempfile.mkdtemp(prefix="froglet-ontology-capacity-")).resolve()
    root.chmod(0o700)
    supplied_binary = binary
    supplied_binary_sha256_at_start = hashlib.sha256(supplied_binary.read_bytes()).hexdigest()
    binary = root / "froglet-node"
    shutil.copy2(supplied_binary, binary)
    binary.chmod(0o700)
    pinned_binary_sha256 = hashlib.sha256(binary.read_bytes()).hexdigest()
    require(pinned_binary_sha256 == supplied_binary_sha256_at_start,
            "The supplied binary changed while creating the private pinned executable")
    bob, alice = Node(binary, root, "bob", "provider"), Node(binary, root, "alice", "runtime")
    alices = [alice]
    baseline_process = None
    # Two Deals per complete workflow; publication/warmup/retries need finite headroom.
    deal_budget = total_workflows * 2 + 32
    # A bounded quote-collision retry can make four metered issuance attempts per Deal.
    quote_budget = deal_budget * 5
    configuration = {"FROGLET_PROVIDER_MAX_TOTAL_DEALS": str(deal_budget),
        "FROGLET_PROVIDER_MAX_TOTAL_QUOTES": str(quote_budget),
        "FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS": str(deal_budget * 30000),
        "FROGLET_QUOTE_QUOTA_PER_IDENTITY": str(quote_budget),
        "FROGLET_WASM_CONCURRENCY_LIMIT": "16", "FROGLET_PROCESS_CONCURRENCY": "4"}
    bob.env.update(configuration)
    succeeded = False
    try:
        source_at_start = source_fingerprints()
        hardware = hardware_details()
        if requester_mode == "per-worker":
            for index in range(1, max(concurrency)):
                alices.append(Node(binary, root, f"alice-{index}", "runtime"))
        requester_ids = [requester.identity() for requester in alices]
        alice_id, bob_id = requester_ids[0], bob.identity()
        require(alice_id != bob_id, "Requester and provider must have distinct signing keys")
        require(len(set(requester_ids + [bob_id])) == len(alices) + 1,
                "Independent requester identities collided")
        bob.start()
        origin = bob.url
        catalog = publish_ontology(bob, root)
        offers = get_json(origin + "/v1/provider/offers")["offers"]
        compute_offer = next(item for item in offers if item["payload"]["offer_id"] == "execute.compute"
                             and item["payload"]["offer_kind"] == "compute.wasm.v1"
                             and item["payload"]["settlement_method"] == "none")
        for offer in (compute_offer, next(item for item in offers if item["hash"] == catalog["offer_hash"])):
            require(offer["payload"]["price_schedule"] == {"base_fee_msat": 0, "success_fee_msat": 0},
                    "Qualification must remain free")
        bob.stop()
        bob.env["FROGLET_LISTEN_ADDR"] = origin.removeprefix("http://")
        tokens = [secrets.token_urlsafe(36) for _ in alices]
        token = tokens[0]
        bob_config = bob.data / "a2a.json"
        private_json(bob_config, {"clients": [{"requester_id": requester_id, "token": credential,
            "offer_hashes": [compute_offer["hash"], catalog["offer_hash"]]}
            for requester_id, credential in zip(requester_ids, tokens)]})
        for requester, credential in zip(alices, tokens):
            config = requester.data / "a2a.json"
            private_json(config, {"providers": [{"provider_url": origin, "token": credential,
                                                  "allow_loopback": True}]})
            requester.env["FROGLET_A2A_CONFIG_PATH"] = str(config)
        bob.env["FROGLET_A2A_CONFIG_PATH"] = str(bob_config)
        bob.start()
        for requester in alices:
            requester.start()
        require(bob.url == origin, "Provider origin changed after A2A configuration")
        environments = [dict(requester.env, FROGLET_DATA_ROOT=str(requester.data),
            FROGLET_DAEMON_URL=origin, FROGLET_PROVIDER_URL=origin,
            FROGLET_RUNTIME_URL=requester.url,
            FROGLET_RUNTIME_AUTH_TOKEN_PATH=str(requester.data / "runtime/auth.token"))
            for requester in alices]
        environment = environments[0]
        lookup_base = {"action": "invoke_service", "service_id": catalog["service_id"],
            "provider_id": bob_id, "input": catalog["query"], "max_price_sats": 0,
            "response_format": "compact"}
        compute_base = {"action": "run_compute", "wasm_module_hex": module.hex(),
            "provider_id": bob_id, "provider_url": origin, "max_price_sats": 0,
            "response_format": "compact"}
        warmup_lookup = dict(lookup_base, idempotency_key="capacity-warmup-data")
        warmup_data = verified_report(mcp(alice, warmup_lookup, environment), "succeeded")
        rows = warmup_data["result"]["rows"]
        require(warmup_data["result_hash"] == catalog["local_verification"]["result_hash"],
                "Warmup selected data differs from locally verified publication")
        require(PRIVATE_SENTINEL not in json.dumps(rows) and "curator_note" not in json.dumps(rows),
                "Excluded private field leaked")
        request = selected_request(rows)
        expected = mapping_oracle(request)
        warmup_compute = dict(compute_base, input=request, idempotency_key="capacity-warmup-compute")
        warmup_result = verified_report(mcp(alice, warmup_compute), "succeeded")
        require(warmup_result["result"] == expected, "Warmup Wasm/Python outputs differ")

        def froglet_workflow(key):
            requester_index = worker_index_from_key(key) if requester_mode == "per-worker" else 0
            requester = alices[requester_index]
            selected = mcp(requester, dict(lookup_base, idempotency_key=key + "-data"),
                           environments[requester_index])
            data_summary = response_summary(selected, warmup_data["result"])
            if data_summary["status"] != "succeeded":
                task = failed_invocation_task(data_summary["error"])
                if task is not None:
                    # invoke_service exposes terminal failure's task only inside its error text.
                    # Recover and verify it before counting the reference as provider evidence.
                    recovered = mcp(requester, {"action": "get_task", "task_id": task,
                        "provider_id": bob_id, "response_format": "compact"})
                    failure = verified_failed_task(recovered, task)
                    data_summary.update({"deal_id": task, "receipt_verified": True,
                        "receipt_hash": failure["receipt_verification"]["receipt_hash"],
                        "failure_code": failure["receipt_verification"].get("failure_code"),
                        "failed_task_recovered": True,
                        "failure_signed_evidence": execution_evidence(origin, tokens[requester_index],
                            failure, "TASK_STATE_FAILED")})
                return dict(data_summary, stage="selected_data", deal_ids=[data_summary["deal_id"]]
                            if "deal_id" in data_summary else [])
            returned = selected["structuredContent"]["result"]["rows"]
            explicit_input = selected_request(returned)
            response = mcp(requester, dict(compute_base, input=explicit_input,
                                       idempotency_key=key + "-compute"))
            computed = response_summary(response, mapping_oracle(explicit_input))
            return dict(computed, stage="compute", selected_data=data_summary,
                        deal_ids=[data_summary["deal_id"]] + ([computed["deal_id"]]
                                  if "deal_id" in computed else []))

        selected_file = root / "baseline-selected-data.json"
        private_json(selected_file, rows)
        baseline_process = subprocess.Popen([sys.executable, str(Path(__file__).resolve()),
            "--baseline-server", str(selected_file)], cwd=REPO, env=clean_environment(),
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        baseline_origin = json.loads(baseline_process.stdout.readline())["url"]

        def http_workflow(_key):
            returned = http_json(baseline_origin + "/selected-table")["rows"]
            require(returned == rows, "Baseline selected data differs from Froglet selected data")
            result = http_json(baseline_origin + "/audit", selected_request(returned))
            require(result == expected, "Baseline output differs from the same independent oracle")
            return {"status": "succeeded", "result": result}

        def local_file_workflow(_key):
            returned = json.loads(selected_file.read_text(encoding="utf-8"))
            require(returned == rows, "Local file differs from Froglet selected data")
            result = mapping_oracle(selected_request(returned))
            require(result == expected, "Local Python output differs from expected audit")
            return {"status": "succeeded", "result": result}

        def discovery(_key):
            card = get_json(origin + "/.well-known/agent-card.json")
            require(any(item.get("uri") == EXTENSION for item in card["capabilities"]["extensions"]),
                    "Expected A2A extension absent")
            require(get_json(origin + "/v1/provider/offers")["offers"], "Empty catalog")
            return {"status": "succeeded"}

        phases = {"froglet_selected_data_and_compute": [], "conventional_http_python": [],
                  "local_selected_file_python": [], "public_card_and_catalog_reads": []}
        for level in concurrency:
            for name, operation in (("public_card_and_catalog_reads", discovery),
                                    ("local_selected_file_python", local_file_workflow),
                                    ("conventional_http_python", http_workflow),
                                    ("froglet_selected_data_and_compute", froglet_workflow)):
                before = counts(bob)
                phase = run_phase(operation, level, iterations, name)
                after = counts(bob)
                if name == "froglet_selected_data_and_compute":
                    phase["active_requester_identities"] = level if requester_mode == "per-worker" else 1
                    ids = [deal for sample in phase["samples"] for deal in sample.get("deal_ids", [])]
                    require(len(ids) == len(set(ids)), "Different benchmark keys returned a duplicate Deal")
                    phase["provider_counts_before"] = before
                    phase["provider_counts_after"] = after
                    phase["returned_unique_task_references"] = len(ids)
                    phase["count_matches_returned_task_references"] = after["deals"] - before["deals"] == len(ids)
                    phase["task_reference_warning"] = "An error may retain an Alice task reference " \
                        "without a corresponding admitted Bob Deal; counts below show the actual provider effects."
                    require(after["jobs"] == 0, "Unexpected separate job queue effects")
                    if phase["errors"] == 0:
                        require(phase["count_matches_returned_task_references"], "Successful workflows created unaccounted Deals")
                phases[name].append(phase)
                diagnostic_samples = [sample for sample in phase["samples"] if sample["status"] != "succeeded"]
                for sample in diagnostic_samples[:3]:
                    if sample["status"] != "succeeded":
                        require(token not in json.dumps(sample), "Private A2A value in diagnostic")
                        print(json.dumps({"phase": name, "error_sample": sample}), file=sys.stderr, flush=True)
                print(f"{name} concurrency={level}: {phase['succeeded']}/{phase['attempted']} "
                      f"p95={phase['latency_ms']['p95']}ms", file=sys.stderr, flush=True)

        before_retry = counts(bob)
        def exact_retry(_key):
            replay = verified_report(mcp(alice, warmup_compute), "succeeded")
            require(replay["deal_id"] == warmup_result["deal_id"]
                    and replay["result_hash"] == warmup_result["result_hash"], "Retry changed evidence")
            return {"status": "succeeded", "deal_id": replay["deal_id"], "result_hash": replay["result_hash"]}
        retries = run_phase(exact_retry, min(max(concurrency), 8), 1, "exact_retries")
        require(retries["errors"] == 0 and counts(bob) == before_retry,
                "Exact retries created extra provider work or failed")
        usage_before = provider_usage(bob)
        bob.stop()
        bob.env["FROGLET_PROVIDER_MAX_TOTAL_DEALS"] = str(usage_before["usage"]["reserved_deals"])
        bob.start()
        usage_restarted = provider_usage(bob)
        require(usage_restarted["usage"] == usage_before["usage"], "Restart reset persistent reservations")
        recovered = exact_retry("restarted-retry")
        denied = mcp(alice, dict(compute_base, input=request, idempotency_key="capacity-exhausted-new-work"))
        require(token not in json.dumps(denied), "Private A2A value in denial diagnostic")
        require(denied.get("isError") is True
                and '"upstream_status":503' in denied.get("structuredContent", {}).get("error", "")
                and usage_restarted["usage"]["reserved_deals"] == usage_restarted["policy"]["max_total_deals"],
                "Provider did not reject new work at the persisted cumulative limit")
        require(counts(bob) == before_retry, "Exhausted new work created a provider Deal")
        final_usage = provider_usage(bob)
        require(final_usage["usage"]["reserved_deals"] == usage_before["usage"]["reserved_deals"],
                "Exhausted new work consumed another reservation")
        evidence = {"selected_data": execution_evidence(origin, token, warmup_data),
                    "compute": execution_evidence(origin, token, warmup_result)}
        hardware["load_average_after"] = list(os.getloadavg()) if hasattr(os, "getloadavg") else None
        source_at_end = source_fingerprints()
        require(hashlib.sha256(binary.read_bytes()).hexdigest() == pinned_binary_sha256,
                "The measured private binary changed during the qualification")
        final_requester_statuses = requester_status_counts(alices)
        report = {"schema": "froglet.ontology-capacity-qualification.v1",
            "created_at": datetime.now(timezone.utc).isoformat(),
            "qualification": "Measured free localhost workloads on one shared machine with one provider "
                "identity. This is not a public HTTPS, independent-machine, "
                "many-identity production, paid, adversarial, long-running or scientific usefulness qualification.",
            "connections": "Concurrency is simultaneous client workflows, not observed TCP connections, "
                "audience size or simultaneous executing programs. Each Froglet workflow uses two "
                "fresh native MCP subprocesses; Bob/Alice are separate persistent node processes.",
            "hardware": hardware, "configuration": dict(configuration,
                FROGLET_EXECUTION_TIMEOUT_SECS=bob.env["FROGLET_EXECUTION_TIMEOUT_SECS"],
                FROGLET_PAYMENT_BACKEND="none", FROGLET_PROVIDER_ACCESS_MODE="open",
                FROGLET_PUBLIC_REQUEST_QUOTA="6000", FROGLET_PUBLIC_WRITE_QUOTA_WINDOW_SECS="900"),
            "configured_execution_slots": 16, "a2a_operation_slots": 16,
            "selected_data_process_slots": 4,
            "configuration_warning": "Quote quota is explicitly raised from the default 60 per "
                "identity/window for this finite run; do not interpret the result as default capacity.",
            "build": {"binary_sha256": pinned_binary_sha256,
                      "binary_bytes": binary.stat().st_size,
                      "binary_pinned_private_copy": True, "binary_unchanged_during_run": True,
                      "supplied_binary_sha256_at_start": supplied_binary_sha256_at_start,
                      "supplied_binary_sha256_at_end": hashlib.sha256(supplied_binary.read_bytes()).hexdigest(),
                      "wasm_sha256": hashlib.sha256(module).hexdigest(), "wasm_bytes": len(module),
                      "source_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                      "source_sha256_at_start": source_at_start,
                      "source_sha256_at_end": source_at_end,
                      "source_unchanged_during_run": source_at_start == source_at_end,
                      "source_boundary": "The binary fingerprint identifies the executable actually "
                          "measured; source fingerprints describe the working tree and do not by "
                          "themselves prove that this binary was compiled from those sources."},
            "identities": {"provider": bob_id, "requester": alice_id,
                           "independent_signing_keys": True, "provider_identity_count": 1,
                           "requester_identity_count": len(requester_ids),
                           "requester_mode": requester_mode, "requesters": requester_ids},
            "selected_data": {"rows": rows, "columns": ["source", "target"],
                "source_sha256": catalog["source_sha256"], "snapshot_sha256": catalog["snapshot_sha256"],
                "result_hash": warmup_data["result_hash"], "private_column_excluded": True},
            "explicit_compute_input": request, "expected_and_observed_result": expected,
            "execution_limits": warmup_result["execution_limits"], "warmup_signed_evidence": evidence,
            "phases": phases, "exact_retry": dict(retries, provider_counts_unchanged=True),
            "restart_and_cumulative_allowance": {"usage_before": usage_before["usage"],
                "usage_after_restart": usage_restarted["usage"], "same_result_recovered": recovered,
                "new_work_refused": True, "provider_counts_unchanged": True,
                "native_error": denied["structuredContent"],
                "error_boundary": "Native A2A reports upstream503 and a generic rejection; "
                    "the provider's actual persisted usage equals the configured Deal ceiling.",
                "final_deal_limit": bob.env["FROGLET_PROVIDER_MAX_TOTAL_DEALS"]},
            "final_provider_counts": counts(bob),
            "final_requester_counts": {name: sum(counts(requester)[name] for requester in alices)
                for name in ("deals", "jobs", "requester_deals")},
            "final_requester_statuses": final_requester_statuses,
            "ambiguous_submission_pending_count": final_requester_statuses.get("submission_pending", 0),
            "comparison": {"shared_semantics": "Identical five selected rows, exact case-sensitive "
                "one-target-per-source rule, duplicate handling and observed terms; outputs checked "
                "against the same independent Python oracle.",
                "froglet_costs_included": "Selection retrieval, fresh MCP process initialization per "
                    "tool call, signed Quote/Deal admission, 80KB inline Wasm transmission, provider "
                    "execution, requester-side receipt verification and durable result storage.",
                "baseline_costs_included": "Two fresh HTTP requests to a separate persistent Python "
                    "server: selected JSON retrieval, then a fixed server-side Python audit. Plain "
                    "HTTPConnection avoids constructing an unused TLS context for each HTTP request.",
                "local_file_costs_included": "Read the identical selected JSON file and run the "
                    "same deterministic Python audit directly in the requesting Python process.",
                "baseline_omissions": "No requester-supplied program, sandbox resource contract, "
                    "signed authorization/receipts, durable tasks, retry keys, recovery or payment.",
                "interpretation": "For this tiny trusted fixed-function audit, conventional HTTP/Python "
                    "is simpler. Timing does not quantify the value of Froglet's extra boundaries. "
                    "No developer-time, maintenance-cost, user-demand or repeat-use measurement."}}
        encoded = json.dumps(report, indent=2) + "\n"
        require(all(credential not in encoded for credential in tokens)
                and PRIVATE_SENTINEL not in encoded, "Private value in exported report")
        for node in [*alices, bob]:
            for credential_path in (node.data / "runtime").glob("*.token"):
                credential = credential_path.read_text().strip()
                require(not credential or credential not in encoded, "Operator/runtime credential in exported report")
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(encoded, encoding="utf-8")
        print(f"Sanitized measured report: {output}", file=sys.stderr)
        succeeded = True
        return report
    finally:
        for requester in alices:
            requester.stop()
        bob.stop()
        if baseline_process is not None:
            stop(baseline_process)
            for pipe in (baseline_process.stdout, baseline_process.stderr):
                if pipe is not None:
                    pipe.close()
        if succeeded and not keep_state:
            shutil.rmtree(root)
        else:
            print(f"Private qualification state retained at {root}", file=sys.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=REPO / "target/debug/froglet-node")
    parser.add_argument("--module", type=Path,
        default=REPO / "examples/wasm-services/target/wasm32-unknown-unknown/release/ontology_check.wasm")
    parser.add_argument("--concurrency", type=concurrency_list, default=[1, 2, 4, 8, 16, 32])
    parser.add_argument("--iterations", type=positive_int, default=2,
                        help="Finite sequential workflows per concurrent worker; default 2")
    parser.add_argument("--requester-identities", choices=("shared", "per-worker"), default="shared",
                        help="One Alice runtime, or independent signing/runtime processes per worker")
    parser.add_argument("--output", type=Path, default=REPO / "_tmp/ontology-capacity/report.json")
    parser.add_argument("--keep-state", action="store_true", help="Retain private node state after success")
    parser.add_argument("--baseline-server", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    os.umask(0o077)
    if args.baseline_server is not None:
        baseline_server(args.baseline_server)
        return
    binary, module_path = args.binary.resolve(), args.module.resolve()
    require(binary.is_file() and module_path.is_file(), "Build or supply froglet-node and ontology_check.wasm first")
    module = module_path.read_bytes()
    require(module.startswith(b"\0asm\x01\0\0\0") and len(module) <= 256 * 1024,
            "Expected a Wasm v1 program within native MCP's inline limit")
    run_qualification(binary, module, args.concurrency, args.iterations, args.output.resolve(),
                      args.keep_state, args.requester_identities)


if __name__ == "__main__":
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    try:
        main()
    except KeyboardInterrupt:
        print("Qualification interrupted; started processes stopped.", file=sys.stderr)
        sys.exit(130)
    except (OSError, RuntimeError, subprocess.SubprocessError, StopIteration, KeyError,
            json.JSONDecodeError) as error:
        print(f"Qualification failed: {error}", file=sys.stderr)
        sys.exit(1)
