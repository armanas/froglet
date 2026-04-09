#!/usr/bin/env python3
import argparse
import asyncio
import json
import secrets
import shutil
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO_ROOT / "python" / "tests"))

from test_support import (  # noqa: E402
    VALID_WASM_HEX,
    build_wasm_request,
    create_protocol_deal,
    create_protocol_quote,
    generate_schnorr_signing_key,
    schnorr_pubkey_hex,
    sha256_hex,
    verify_signed_artifact,
)

RUN_NONCE = secrets.token_hex(6)


class RemoteNode:
    def __init__(self, role_name: str, role: dict, base_url: str | None = None):
        self.role_name = role_name
        self.role = role
        self.base_url = base_url or role.get("provider_public_url") or role.get("url")

    def url(self, path: str) -> str:
        return f"{self.base_url}{path}"


def request_json_sync(
    ca_cert_path: str,
    method: str,
    url: str,
    *,
    headers: dict | None = None,
    payload: dict | None = None,
) -> tuple[int, dict]:
    status_marker = "__FROGLET_HTTP_STATUS__:"
    command = [
        "curl",
        "-sS",
        "-X",
        method,
        "-w",
        f"\\n{status_marker}%{{http_code}}",
    ]
    if url.startswith("https://"):
        command.extend(["--cacert", ca_cert_path])
    for key, value in (headers or {}).items():
        command.extend(["-H", f"{key}: {value}"])
    if payload is not None:
        command.extend(["-H", "Content-Type: application/json"])
        command.extend(["--data-binary", json.dumps(payload, separators=(",", ":"))])
    command.append(url)
    completed = subprocess.run(
        command,
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"curl {method} {url} failed ({completed.returncode}): {completed.stderr.strip()}"
        )
    raw = completed.stdout
    body_text, marker, status_text = raw.rpartition(f"\n{status_marker}")
    if not marker:
        raise RuntimeError(f"missing HTTP status marker from curl output for {url}")
    status = int(status_text.strip())
    raw_body = body_text
    if not raw_body:
        body = {}
    else:
        body = json.loads(raw_body)
    return status, body


class UrlLibResponse:
    def __init__(self, status: int, body: dict):
        self.status = status
        self._body = body

    async def __aenter__(self) -> "UrlLibResponse":
        return self

    async def __aexit__(self, exc_type, exc, tb) -> bool:
        return False

    async def json(self) -> dict:
        return self._body


class UrlLibRequestContext:
    def __init__(
        self,
        ca_cert_path: str,
        method: str,
        url: str,
        *,
        headers: dict | None = None,
        json_payload: dict | None = None,
    ):
        self.ca_cert_path = ca_cert_path
        self.method = method
        self.url = url
        self.headers = headers
        self.json_payload = json_payload
        self._response: UrlLibResponse | None = None

    async def __aenter__(self) -> UrlLibResponse:
        status, body = await asyncio.to_thread(
            request_json_sync,
            self.ca_cert_path,
            self.method,
            self.url,
            headers=self.headers,
            payload=self.json_payload,
        )
        self._response = UrlLibResponse(status, body)
        return self._response

    async def __aexit__(self, exc_type, exc, tb) -> bool:
        return False


class UrlLibSession:
    def __init__(self, ca_cert_path: str):
        self.ca_cert_path = ca_cert_path

    def request(
        self,
        method: str,
        url: str,
        *,
        headers: dict | None = None,
        json: dict | None = None,
        ) -> UrlLibRequestContext:
        return UrlLibRequestContext(
            self.ca_cert_path,
            method,
            url,
            headers=headers,
            json_payload=json,
        )

    def get(self, url: str, *, headers: dict | None = None) -> UrlLibRequestContext:
        return self.request("GET", url, headers=headers)

    def post(
        self,
        url: str,
        *,
        headers: dict | None = None,
        json: dict | None = None,
    ) -> UrlLibRequestContext:
        return self.request("POST", url, headers=headers, json=json)


async def request_json(
    session: UrlLibSession,
    method: str,
    url: str,
    *,
    expected_statuses: tuple[int, ...] = (200,),
    headers: dict | None = None,
    payload: dict | None = None,
) -> tuple[int, dict]:
    async with session.request(method, url, headers=headers, json=payload) as response:
        body = await response.json()
    if response.status not in expected_statuses:
        raise AssertionError(f"expected {expected_statuses} from {url}, got {response.status}: {body}")
    return response.status, body


async def wait_for_deal(
    session: UrlLibSession,
    node: RemoteNode,
    deal_id: str,
    terminal_states: set[str],
    *,
    timeout_secs: float = 30.0,
) -> dict:
    deadline = time.monotonic() + timeout_secs
    last = None
    while time.monotonic() < deadline:
      _, last = await request_json(
          session,
          "GET",
          node.url(f"/v1/provider/deals/{deal_id}"),
      )
      if last.get("status") in terminal_states:
          return last
      await asyncio.sleep(0.25)
    raise AssertionError(f"timed out waiting for deal {deal_id}; last={last}")


def gcloud_ssh(project: str, zone: str, instance: str, command: str, *, nat_ip: str | None = None) -> str:
    if shutil.which("gcloud") is not None:
        completed = subprocess.run(
            [
                "gcloud",
                "compute",
                "ssh",
                instance,
                f"--project={project}",
                f"--zone={zone}",
                "--quiet",
                f"--command={command}",
            ],
            capture_output=True,
            text=True,
        )
        if completed.returncode == 0:
            return completed.stdout.strip()

    if not nat_ip:
        raise RuntimeError(f"failed to ssh to {instance} and no nat_ip fallback was provided")

    key_path = Path.home() / ".ssh" / "google_compute_engine"
    if not key_path.exists():
        raise RuntimeError(f"failed to ssh to {instance}; missing fallback key {key_path}")

    completed = subprocess.run(
        [
            "ssh",
            "-i",
            str(key_path),
            "-o",
            "BatchMode=yes",
            "-o",
            "StrictHostKeyChecking=no",
            f"armanas@{nat_ip}",
            command,
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    return completed.stdout.strip()


def set_mock_invoice_bundle_states(project: str, role: dict, session_id: str, *, base_state: str, success_state: str) -> None:
    node_db = Path(role["data_root"]) / "node.db"
    script = (
        "python3 -c "
        + json.dumps(
            "import sqlite3; "
            f"conn=sqlite3.connect({json.dumps(str(node_db))}); "
            "conn.execute("
            "\"UPDATE lightning_invoice_bundles "
            "SET base_state = ?, success_state = ?, updated_at = strftime('%s','now') "
            "WHERE session_id = ?\", "
            f"({json.dumps(base_state)}, {json.dumps(success_state)}, {json.dumps(session_id)})"
            "); conn.commit()"
        )
    )
    gcloud_ssh(project, role["zone"], role["instance"], script, nat_ip=role.get("nat_ip"))

async def check_marketplace_visibility(
    session: UrlLibSession,
    runtime_url: str,
    runtime_token: str,
    free_seed: dict,
    paid_seed: dict,
) -> dict:
    _, search = await request_json(
        session,
        "POST",
        f"{runtime_url}/v1/runtime/search",
        headers={"Authorization": f"Bearer {runtime_token}"},
        payload={"limit": 20},
    )
    node_ids = {provider["provider_id"] for provider in search.get("providers", [])}
    assert free_seed["provider_id"] in node_ids
    assert paid_seed["provider_id"] in node_ids
    return {
        "node_ids": sorted(node_ids),
        "search_count": len(search.get("providers", [])),
    }


async def check_public_service_redaction(session: UrlLibSession, free_seed: dict) -> dict:
    provider = RemoteNode("froglet-provider-free", {"provider_public_url": free_seed["provider_public_url"]})
    hidden_service_id = free_seed["services"]["hidden"]["service_id"]
    visible_service_id = free_seed["services"]["free_python_inline"]["service_id"]

    _, visible = await request_json(session, "GET", provider.url(f"/v1/provider/services/{visible_service_id}"))
    assert visible["service"]["service_id"] == visible_service_id
    assert visible["service"].get("binding_hash")
    assert visible["service"].get("inline_source") is None
    assert visible["service"].get("module_bytes_hex") is None

    hidden_status, hidden = await request_json(
        session,
        "GET",
        provider.url(f"/v1/provider/services/{hidden_service_id}"),
        expected_statuses=(404,),
    )
    assert hidden_status == 404
    return {
        "visible_service_id": visible_service_id,
        "hidden_service_id": hidden_service_id,
        "visible_runtime": visible["service"]["runtime"],
        "hidden_error": hidden["error"],
    }


async def check_free_compute_chain(session: UrlLibSession, free_seed: dict) -> dict:
    provider = RemoteNode("froglet-provider-free", {"provider_public_url": free_seed["provider_public_url"]})
    requester_key = generate_schnorr_signing_key()
    requester_id = schnorr_pubkey_hex(requester_key)

    _, descriptor = await request_json(session, "GET", provider.url("/v1/provider/descriptor"))
    _, offers = await request_json(session, "GET", provider.url("/v1/provider/offers"))
    descriptor_artifact = descriptor.get("document", descriptor)
    offer_artifacts = [offer.get("offer", offer) for offer in offers.get("offers", [])]
    quote = await create_protocol_quote(
        session,
        provider,
        offer_id="execute.compute",
        request=build_wasm_request(VALID_WASM_HEX),
        requester_secret_key=requester_key,
    )
    created = await create_protocol_deal(
        session,
        provider,
        quote=quote,
        request=build_wasm_request(VALID_WASM_HEX),
        requester_secret_key=requester_key,
        idempotency_key=f"gcp-free-compute-{RUN_NONCE}",
    )

    terminal = created
    if terminal.get("status") not in {"succeeded", "failed"}:
        terminal = await wait_for_deal(session, provider, created["deal_id"], {"succeeded", "failed"})
    assert terminal["status"] == "succeeded"
    assert verify_signed_artifact(descriptor_artifact)
    assert all(verify_signed_artifact(offer) for offer in offer_artifacts)
    assert verify_signed_artifact(quote)
    assert verify_signed_artifact(terminal["deal"])
    assert verify_signed_artifact(terminal["receipt"])
    _, receipt_validation = await request_json(
        session,
        "POST",
        provider.url("/v1/receipts/verify"),
        payload={"receipt": terminal["receipt"]},
    )
    assert receipt_validation["valid"] is True

    tampered_receipt = json.loads(json.dumps(terminal["receipt"]))
    tampered_receipt["payload"]["result_hash"] = "00" * 32
    _, tampered_validation = await request_json(
        session,
        "POST",
        provider.url("/v1/receipts/verify"),
        payload={"receipt": tampered_receipt},
    )
    assert tampered_validation["valid"] is False

    return {
        "requester_id": requester_id,
        "descriptor_hash": descriptor_artifact["hash"],
        "offer_ids": [offer["payload"]["offer_id"] for offer in offer_artifacts],
        "quote_hash": quote["hash"],
        "deal_hash": terminal["deal"]["hash"],
        "receipt_hash": terminal["receipt"]["hash"],
        "result": terminal["result"],
        "tampered_receipt_valid": tampered_validation["valid"],
    }


async def check_mock_lightning(session: UrlLibSession, inventory: dict, paid_seed: dict) -> dict:
    provider_role = inventory["roles"]["froglet-provider-paid"]
    provider = RemoteNode("froglet-provider-paid", provider_role, paid_seed["provider_public_url"])
    requester_key = generate_schnorr_signing_key()
    requester_id = schnorr_pubkey_hex(requester_key)
    success_preimage = "44" * 32
    success_payment_hash = sha256_hex(bytes.fromhex(success_preimage))

    quote = await create_protocol_quote(
        session,
        provider,
        offer_id="execute.compute",
        request=build_wasm_request(VALID_WASM_HEX),
        requester_secret_key=requester_key,
    )
    created = await create_protocol_deal(
        session,
        provider,
        quote=quote,
        request=build_wasm_request(VALID_WASM_HEX),
        requester_secret_key=requester_key,
        idempotency_key=f"gcp-mock-lightning-{RUN_NONCE}",
        success_payment_hash=success_payment_hash,
    )
    assert created["status"] == "payment_pending"

    _, bundle = await request_json(
        session,
        "GET",
        provider.url(f"/v1/provider/deals/{created['deal_id']}/invoice-bundle"),
    )
    _, bundle_validation = await request_json(
        session,
        "POST",
        provider.url("/v1/invoice-bundles/verify"),
        payload={
            "bundle": bundle["bundle"],
            "quote": quote,
            "deal": created["deal"],
            "requester_id": requester_id,
        },
    )
    assert verify_signed_artifact(bundle["bundle"])
    assert bundle_validation["valid"] is True

    set_mock_invoice_bundle_states(
        inventory["project"],
        provider_role,
        bundle["session_id"],
        base_state="settled",
        success_state="accepted",
    )
    result_ready = await wait_for_deal(session, provider, created["deal_id"], {"result_ready", "failed"})
    assert result_ready["status"] == "result_ready"

    _, terminal = await request_json(
        session,
        "POST",
        provider.url(f"/v1/provider/deals/{created['deal_id']}/accept"),
        payload={
            "success_preimage": success_preimage,
            "expected_result_hash": result_ready["result_hash"],
        },
    )
    assert terminal["status"] == "succeeded"
    assert verify_signed_artifact(terminal["receipt"])

    tampered_bundle = json.loads(json.dumps(bundle["bundle"]))
    tampered_bundle["payload"]["quote_hash"] = "11" * 32
    _, tampered_validation = await request_json(
        session,
        "POST",
        provider.url("/v1/invoice-bundles/verify"),
        payload={
            "bundle": tampered_bundle,
            "quote": quote,
            "deal": created["deal"],
            "requester_id": requester_id,
        },
    )
    assert tampered_validation["valid"] is False

    return {
        "quote_hash": quote["hash"],
        "deal_hash": created["deal"]["hash"],
        "bundle_hash": bundle["bundle"]["hash"],
        "receipt_hash": terminal["receipt"]["hash"],
        "bundle_valid": bundle_validation["valid"],
        "tampered_bundle_valid": tampered_validation["valid"],
        "settlement_state": terminal["receipt"]["payload"]["settlement_state"],
    }


async def check_runtime_security(
    session: UrlLibSession,
    runtime_url: str,
    runtime_token: str,
    free_seed: dict,
    paid_seed: dict,
) -> dict:
    headers = {"Authorization": f"Bearer {runtime_token}"}
    _, mismatch = await request_json(
        session,
        "POST",
        f"{runtime_url}/v1/runtime/deals",
        headers=headers,
        payload={
            "provider": {
                "provider_id": free_seed["provider_id"],
                "provider_url": paid_seed["provider_public_url"],
            },
            "offer_id": "execute.compute",
            **build_wasm_request(VALID_WASM_HEX),
        },
        expected_statuses=(400,),
    )
    _, ssrf = await request_json(
        session,
        "POST",
        f"{runtime_url}/v1/runtime/deals",
        headers=headers,
        payload={
            "provider": {
                "provider_url": "https://127.0.0.1:8080",
            },
            "offer_id": "execute.compute",
            **build_wasm_request(VALID_WASM_HEX),
        },
        expected_statuses=(400,),
    )
    assert "provider_id does not match provider_url descriptor" in mismatch["error"]
    assert "local or private-network" in ssrf["error"]
    return {
        "mismatch_error": mismatch["error"],
        "ssrf_error": ssrf["error"],
    }


async def check_restart_recovery(
    session: UrlLibSession,
    inventory: dict,
    free_seed: dict,
) -> dict:
    provider_role = inventory["roles"]["froglet-provider-free"]
    provider = RemoteNode("froglet-provider-free", provider_role, free_seed["provider_public_url"])

    start = time.monotonic()
    gcloud_ssh(
        inventory["project"],
        provider_role["zone"],
        provider_role["instance"],
        "sudo -n /usr/bin/systemctl restart froglet-provider.service",
        nat_ip=provider_role.get("nat_ip"),
    )
    for _ in range(120):
        try:
            _, response = await request_json(session, "GET", provider.url("/health"))
            if response.get("status") == "ok":
                break
        except Exception:
            pass
        await asyncio.sleep(0.5)
    provider_recovery_secs = time.monotonic() - start

    assert provider_recovery_secs <= 60.0
    return {
        "provider_recovery_secs": round(provider_recovery_secs, 3),
    }


async def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--inventory", required=True)
    parser.add_argument("--seed-free", required=True)
    parser.add_argument("--seed-paid", required=True)
    parser.add_argument("--provider-url", required=True)
    parser.add_argument("--provider-token-path", required=True)
    parser.add_argument("--runtime-url")
    parser.add_argument("--runtime-token-path")
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    inventory = json.loads(Path(args.inventory).read_text())
    free_seed = json.loads(Path(args.seed_free).read_text())
    paid_seed = json.loads(Path(args.seed_paid).read_text())
    provider_token = Path(args.provider_token_path).read_text().strip()
    runtime_url = args.runtime_url or inventory["roles"]["froglet-marketplace"]["runtime_url"]
    runtime_token_path = (
        Path(args.runtime_token_path)
        if args.runtime_token_path
        else Path(inventory["roles"]["froglet-marketplace"]["token_paths"]["runtime_auth"])
    )
    runtime_token = runtime_token_path.read_text().strip()
    inventory_path = Path(args.inventory).resolve()
    run_local_ca = inventory_path.parent / "pki" / "ca.pem"
    ca_cert_path = str(run_local_ca if run_local_ca.exists() else Path(inventory["ca_cert_path"]))

    session = UrlLibSession(ca_cert_path)
    results = {
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "run_id": inventory["run_id"],
        "checks": {},
    }
    results["checks"]["marketplace_visibility"] = await check_marketplace_visibility(
        session, runtime_url, runtime_token, free_seed, paid_seed
    )
    results["checks"]["public_service_redaction"] = await check_public_service_redaction(
        session, free_seed
    )
    results["checks"]["free_compute_chain"] = await check_free_compute_chain(session, free_seed)
    results["checks"]["mock_lightning"] = await check_mock_lightning(session, inventory, paid_seed)
    results["checks"]["runtime_security"] = await check_runtime_security(
        session,
        runtime_url,
        runtime_token,
        free_seed,
        paid_seed,
    )
    results["checks"]["restart_recovery"] = await check_restart_recovery(
        session, inventory, free_seed
    )

    Path(args.out).write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    asyncio.run(main())
