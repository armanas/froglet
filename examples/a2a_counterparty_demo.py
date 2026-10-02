#!/usr/bin/env python3
"""Real local invite + scoped A2A setup/revocation qualification (no payments).

Uses independent Node processes and the operator configuration helper. A
temporary local CA is trusted only by these processes, never the OS trust store.
"""
import argparse
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import ssl
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

REPO = Path(__file__).resolve().parents[1]
if str(REPO) not in sys.path:
    sys.path.insert(0, str(REPO))
from examples import a2a_compute_demo as common  # noqa: E402
from examples.a2a_compute_demo import (  # noqa: E402
    EXTENSION, Node, counts, execution_evidence, get_json, mapping_oracle, mcp,
    ontology_source, require, verified_report,
)
from scripts import setup_a2a_counterparty as setup  # noqa: E402


def configure(request):
    plan = setup.prepare(request)[0]
    result = setup.apply(request, plan["plan_sha256"])
    return plan, result


def source_settings(node, path):
    """Execute the generated quoted operator fragment, never parse shell by hand."""
    command = ["/bin/sh", "-c", '. "$1"; env -0', "froglet-setup", str(path / "activate.sh")]
    result = subprocess.run(command, env=node.env, capture_output=True, timeout=10, check=True)
    node.env = dict(item.decode().split("=", 1) for item in result.stdout.split(b"\0") if item)


def refused(response, node, before, label):
    require(response.get("isError") is True, label + " unexpectedly succeeded")
    require(counts(node)["deals"] == before, label + " created a provider Deal")
    return {"refused": True, "provider_deals": before,
            "error": response.get("structuredContent", {}).get("error", "")}


class LocalTlsProxy:
    """Loopback-only qualification ingress with a short-lived dedicated CA."""
    def __init__(self, root):
        self.root = root / "local-tls"
        self.root.mkdir(mode=0o700)
        openssl = shutil.which("openssl")
        require(openssl is not None, "This local TLS qualification requires OpenSSL")
        extensions = self.root / "server.ext"
        extensions.write_text("basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:127.0.0.1\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid,issuer\n")
        commands = [
            ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-subj", "/CN=Froglet local qualification CA",
             "-keyout", "ca.key", "-out", "ca.crt", "-addext", "basicConstraints=critical,CA:TRUE",
             "-addext", "keyUsage=critical,keyCertSign,cRLSign", "-addext", "subjectKeyIdentifier=hash"],
            ["req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=127.0.0.1",
             "-keyout", "server.key", "-out", "server.csr"],
            ["x509", "-req", "-in", "server.csr", "-CA", "ca.crt", "-CAkey", "ca.key", "-CAcreateserial",
             "-days", "1", "-out", "server.crt", "-extfile", "server.ext"],
        ]
        for command in commands:
            subprocess.run([openssl, *command], cwd=self.root, capture_output=True, check=True, timeout=30)
        for file in self.root.iterdir():
            file.chmod(0o600)
        owner = self

        class ProxyHandler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *arguments):
                pass  # Credentials and request URLs must never enter fixture logs.

            def proxy(self):
                length = int(self.headers.get("Content-Length", "0"))
                if not 0 <= length <= 2 * 1024 * 1024:
                    self.send_error(413)
                    return
                connection = http.client.HTTPConnection("127.0.0.1", owner.backend_port, timeout=10)
                try:
                    headers = {name: value for name, value in self.headers.items()
                               if name.lower() not in ("host", "connection", "transfer-encoding")}
                    connection.request(self.command, self.path, self.rfile.read(length), headers)
                    response = connection.getresponse()
                    body = response.read(4 * 1024 * 1024 + 1)
                    require(len(body) <= 4 * 1024 * 1024, "TLS proxy response exceeds limit")
                    self.send_response(response.status)
                    for name, value in response.getheaders():
                        if name.lower() not in ("connection", "transfer-encoding", "content-length"):
                            self.send_header(name, value)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                finally:
                    connection.close()

            do_GET = proxy
            do_POST = proxy

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), ProxyHandler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(self.root / "server.crt", self.root / "server.key")
        self.server.socket = context.wrap_socket(self.server.socket, server_side=True)
        self.origin = "https://127.0.0.1:" + str(self.server.server_port)
        self.backend_port = 0
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.client_context = ssl.create_default_context(cafile=str(self.root / "ca.crt"))

    def stop(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)


def rejected_quote(origin, token, admission_token, requester_id, offer_id,
                   expected_status, expected_message, module=None, input_value=None):
    payload = {"offer_id": offer_id, "requester_id": requester_id,
               "kind": "events_query", "kinds": [], "limit": 1, "max_price_sats": 0}
    if module is not None:
        workload = {"schema_version": "froglet/v1", "workload_kind": "compute.wasm.v1",
                    "abi_version": "froglet.wasm.run_json.v1", "module_format": "application/wasm",
                    "module_hash": hashlib.sha256(module).hexdigest(),
                    "input_format": "application/json+jcs", "input_hash": hashlib.sha256(
                        json.dumps(input_value, separators=(",", ":"), sort_keys=True).encode()).hexdigest(),
                    "requested_capabilities": []}
        payload = {"offer_id": offer_id, "requester_id": requester_id, "kind": "wasm",
                   "submission": {"schema_version": "froglet/v1", "submission_type": "wasm_submission",
                                  "workload": workload, "module_bytes_hex": module.hex(), "input": input_value},
                   "max_price_sats": 0}
    request = {"message": {"messageId": "unapproved-offer-qualification", "role": "ROLE_USER",
        "parts": [{"mediaType": "application/json", "data": {"schema": "froglet.a2a.v1",
        "operation": "quote", "payload": json.dumps(payload)}}]},
        "configuration": {"returnImmediately": False, "historyLength": 0}}
    http_request = urllib.request.Request(origin + "/a2a/v1/message:send", data=json.dumps(request).encode(),
        headers={"Content-Type": "application/json", "Authorization": "Bearer " + token,
                 "A2A-Version": "1.0", "A2A-Extensions": EXTENSION,
                 "x-froglet-access-token": admission_token})
    try:
        common.HTTP.open(http_request, timeout=10).close()
    except urllib.error.HTTPError as error:
        body = json.loads(error.read())
        require(error.code == expected_status and expected_message in json.dumps(body),
                "Negative quote failed for a different reason: " + json.dumps(body))
        return {"refused": True, "http_status": error.code, "provider_deals": 0}
    raise RuntimeError("Negative quote unexpectedly admitted")


def run(binary, module):
    root = Path(tempfile.mkdtemp(prefix="froglet-invite-a2a-qualification-")).resolve()
    root.chmod(0o700)
    bob, alice, charlie = (Node(binary, root, name, role) for name, role in
                           (("bob", "provider"), ("alice", "runtime"), ("charlie", "runtime")))
    tls = None
    original_opener = common.HTTP
    try:
        tls = LocalTlsProxy(root)
        common.HTTP = urllib.request.build_opener(urllib.request.ProxyHandler({}),
            urllib.request.HTTPSHandler(context=tls.client_context))
        bob.env["FROGLET_PROVIDER_ACCESS_MODE"] = "invite"
        bob.env["FROGLET_PUBLIC_BASE_URL"] = tls.origin
        for node in (bob, alice, charlie):
            node.env["FROGLET_HTTP_CA_CERT_PATH"] = str(tls.root / "ca.crt")
        bob_id, alice_id, charlie_id = bob.identity(), alice.identity(), charlie.identity()
        require(len({bob_id, alice_id, charlie_id}) == 3, "Independent identities required")
        bob.start()
        backend = bob.url
        tls.backend_port = int(backend.rsplit(":", 1)[1])
        origin = tls.origin
        offers = get_json(origin + "/v1/provider/offers")["offers"]
        offer = next(item for item in offers if item["payload"]["offer_id"] == "execute.compute"
                     and item["payload"]["offer_kind"] == "compute.wasm.v1"
                     and item["payload"]["settlement_method"] == "none")
        invite_file = root / "issued-alice.token"
        operator_env = dict(bob.env, FROGLET_DAEMON_URL=backend, FROGLET_DATA_ROOT=str(bob.data))
        issued = mcp(bob, {"action": "invite_create", "name": "Alice qualification",
                          "expires_at": int(time.time()) + 3600, "max_requests": 20,
                          "token_file": str(invite_file)}, operator_env)
        require(issued.get("isError") is False, "Existing invite issuance failed")
        invitation = issued["structuredContent"]
        bob_plan, bob_setup = configure({"kind": "provider", "destination": str(root / "bob-setup"),
            "provider_id": bob_id, "provider_url": origin, "requester_id": alice_id,
            "offer_hashes": [offer["hash"]], "admission_token_file": str(invite_file),
            "allow_loopback": True, "allowances": {"max_total_quotes": 10,
            "max_total_deals": 2, "max_total_runtime_ms": 4000}})
        alice_plan, alice_setup = configure({"kind": "requester", "destination": str(root / "alice-setup"),
            "handoff_file": bob_setup["private_handoff_file"], "expected_provider_id": bob_id,
            "expected_requester_id": alice_id})
        bob.stop()
        bob.env["FROGLET_LISTEN_ADDR"] = backend.removeprefix("http://")
        source_settings(bob, root / "bob-setup")
        source_settings(alice, root / "alice-setup")
        bob.start()
        alice.start()
        require(bob.url == backend, "Provider backend changed after setup restart")
        # Copy only Alice's requester A2A config into Charlie's operator test
        # context. A valid bearer does not transfer Alice's signing authority.
        charlie.env["FROGLET_A2A_CONFIG_PATH"] = alice.env["FROGLET_A2A_CONFIG_PATH"]
        charlie.start()
        rows = [{name: row[name] for name in ("source", "target")}
                for row in ontology_source()["terminology"]]
        input_value = {"mappings": rows, "observed_terms": ["DEMO:sample.plasma",
            "DEMO:sample.serum", "DEMO:sample.blood", "DEMO:sample.saliva"]}
        arguments = {"action": "run_compute", "wasm_module_hex": module.hex(), "input": input_value,
                     "provider_id": bob_id, "provider_url": origin, "max_price_sats": 0,
                     "response_format": "compact", "idempotency_key": "invited-compute-1"}  # gitleaks:allow -- public retry label, not an authentication credential
        negatives = {}
        negatives["missing_admission_invite"] = refused(mcp(alice, dict(arguments,
            idempotency_key="missing-admission-1")), bob, 0, "Missing invite")
        admitted = dict(arguments, access_token_file=alice_setup["access_token_file"])
        negatives["wrong_requester_key"] = refused(mcp(charlie, dict(admitted,
            idempotency_key="wrong-key-1")), bob, 0, "Wrong key")
        handoff = json.loads(Path(bob_setup["private_handoff_file"]).read_bytes())
        direct_key_refusal = rejected_quote(origin, handoff["a2a_token"], handoff["admission_token"],
            charlie_id, offer["payload"]["offer_id"], 403, "requester identity does not match A2A credential")
        negatives["wrong_requester_key"]["direct_a2a_quote"] = direct_key_refusal
        other_offer = next(item for item in offers if item["payload"]["offer_id"] == "events.query")
        negatives["unapproved_offer"] = rejected_quote(origin, handoff["a2a_token"],
            handoff["admission_token"], alice_id, other_offer["payload"]["offer_id"], 404,
            "Offer not found or inaccessible")
        require(counts(bob)["deals"] == 0, "Unapproved Offer created a Deal")
        first_response = mcp(alice, admitted)
        require(first_response.get("structuredContent", {}).get("status") == "succeeded",
                "Invited compute failed: " + json.dumps({key: first_response.get("structuredContent", {}).get(key)
                    for key in ("status", "error", "code")}))
        first = verified_report(first_response, "succeeded")
        require(first["result"] == mapping_oracle(input_value), "Invited compute returned wrong audit")
        evidence = execution_evidence(origin, handoff["a2a_token"], first)
        replay = verified_report(mcp(alice, admitted), "succeeded")
        require(replay["deal_hash"] == first["deal_hash"] and counts(bob)["deals"] == 1,
                "Exact retry created work")
        second = verified_report(mcp(alice, dict(admitted, idempotency_key="invited-compute-2")),  # gitleaks:allow -- public retry label, not an authentication credential
                                 "succeeded")
        require(first["deal_id"] != second["deal_id"], "Fresh key did not request new work")
        negatives["finite_allowance_exhaustion"] = refused(mcp(alice, dict(admitted,
            idempotency_key="invited-compute-over-quota")), bob, 2, "Quota exhaustion")
        revoked = mcp(bob, {"action": "invite_revoke", "invite_id": invitation["id"]}, operator_env)
        require(revoked.get("isError") is False, "Invite revocation failed")
        negatives["revoked_admission_invite"] = refused(mcp(alice, dict(admitted,
            idempotency_key="invited-compute-after-revoke")), bob, 2, "Revoked invite")
        direct_revocation = rejected_quote(origin, handoff["a2a_token"], handoff["admission_token"],
            alice_id, offer["payload"]["offer_id"], 403, "provider execution requires an access credential",
            module, input_value)
        direct_revocation["provider_deals"] = counts(bob)["deals"]
        require(direct_revocation["provider_deals"] == 2, "Revoked invitation admitted direct A2A work")
        negatives["revoked_admission_invite"]["direct_a2a_quote"] = direct_revocation
        # Same completed requester record is recoverable without loading an
        # invitation file. A2A task read remains scoped by the bearer/key/Offer.
        recovered = verified_report(mcp(alice, arguments), "succeeded")
        require(recovered["deal_hash"] == first["deal_hash"], "Completed recovery changed evidence")
        bob.stop()
        bob.start()
        recovered_evidence = execution_evidence(origin, handoff["a2a_token"], first)
        require(recovered_evidence == evidence, "Provider restart changed completed evidence")
        report = {"status": "qualified_local_tls", "payments": "none", "public_https_tested": False,
                  "trust_scope": "temporary CA in Node/Python processes only; OS trust unchanged",
                  "provider_id": bob_id, "requester_id": alice_id, "other_requester_id": charlie_id,
                  "provider_plan": bob_plan, "requester_plan": alice_plan,
                  "result": first["result"], "evidence": evidence, "negative_checks": negatives,
                  "exact_retry": True, "completed_recovery_without_invitation_file": True,
                  "provider_restart_preserved_evidence": True, "provider_counts": counts(bob)}
        text = json.dumps(report, indent=2) + "\n"
        secrets_to_check = [handoff["a2a_token"], handoff["admission_token"]]
        for node in (bob, alice, charlie):
            for file in node.data.rglob("*.token"):
                secrets_to_check.append(file.read_text().strip())
        for secret in secrets_to_check:
            require(secret not in text, "Credential leaked into sanitized report")
            for node in (bob, alice, charlie):
                for file in node.data.glob("node-*.log"):
                    require(secret not in file.read_text(), "Credential leaked into Node log")
        return report
    finally:
        for node in (bob, alice, charlie):
            node.stop()
        if tls is not None:
            tls.stop()
        common.HTTP = original_opener
        shutil.rmtree(root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--no-build", action="store_true")
    parser.add_argument("--binary", type=Path, help="pinned froglet-node binary; requires --module")
    parser.add_argument("--module", type=Path, help="compiled ontology-check Wasm; requires --binary")
    parser.add_argument("--output", type=Path)
    arguments = parser.parse_args()
    if (arguments.binary is None) != (arguments.module is None):
        parser.error("--binary and --module must be supplied together")
    if not arguments.no_build and arguments.binary is None:
        subprocess.run(["cargo", "build", "--locked", "--bin", "froglet-node"], cwd=REPO, check=True)
        subprocess.run(["cargo", "build", "--locked", "--release", "--target", "wasm32-unknown-unknown",
                        "--manifest-path", "examples/wasm-services/Cargo.toml", "-p", "ontology-check"],
                       cwd=REPO, check=True)
    target = Path(os.environ.get("CARGO_TARGET_DIR", "target"))
    if not target.is_absolute():
        target = REPO / target
    binary = (arguments.binary or target / "debug/froglet-node").resolve()
    module = (arguments.module or REPO / "examples/wasm-services/target/wasm32-unknown-unknown/release/ontology_check.wasm").resolve()
    binary_hash = hashlib.sha256(binary.read_bytes()).hexdigest()
    module_bytes = module.read_bytes()
    module_hash = hashlib.sha256(module_bytes).hexdigest()
    report = run(binary, module_bytes)
    require(hashlib.sha256(binary.read_bytes()).hexdigest() == binary_hash,
            "Pinned binary changed during qualification")
    report["binary_sha256"] = binary_hash
    require(hashlib.sha256(module.read_bytes()).hexdigest() == module_hash,
            "Pinned module changed during qualification")
    report["module_sha256"] = module_hash
    text = json.dumps(report, indent=2) + "\n"
    if arguments.output:
        arguments.output.write_text(text)
    print(text, end="")


if __name__ == "__main__":
    main()
