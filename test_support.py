import asyncio
import hashlib
import os
import shutil
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time
import unittest
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Optional

import aiohttp
from ecdsa import curves, ellipticcurve

REPO_ROOT = Path(__file__).resolve().parent
TARGET_DIR = REPO_ROOT / "target" / "debug"
FROGLET_BIN = TARGET_DIR / "froglet"
MARKETPLACE_BIN = TARGET_DIR / "marketplace"
VALID_CASHU_TOKEN = "cashuAeyJ0b2tlbiI6W3sibWludCI6Imh0dHBzOi8vODMzMy5zcGFjZTozMzM4IiwicHJvb2ZzIjpbeyJhbW91bnQiOjIsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6IjQwNzkxNWJjMjEyYmU2MWE3N2UzZTZkMmFlYjRjNzI3OTgwYmRhNTFjZDA2YTZhZmMyOWUyODYxNzY4YTc4MzciLCJDIjoiMDJiYzkwOTc5OTdkODFhZmIyY2M3MzQ2YjVlNDM0NWE5MzQ2YmQyYTUwNmViNzk1ODU5OGE3MmYwY2Y4NTE2M2VhIn0seyJhbW91bnQiOjgsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSIsInNlY3JldCI6ImZlMTUxMDkzMTRlNjFkNzc1NmIwZjhlZTBmMjNhNjI0YWNhYTNmNGUwNDJmNjE0MzNjNzI4YzcwNTdiOTMxYmUiLCJDIjoiMDI5ZThlNTA1MGI4OTBhN2Q2YzA5NjhkYjE2YmMxZDVkNWZhMDQwZWExZGUyODRmNmVjNjlkNjEyOTlmNjcxMDU5In1dfV0sInVuaXQiOiJzYXQiLCJtZW1vIjoiVGhhbmsgeW91IHZlcnkgbXVjaC4ifQ=="
VALID_WASM_HEX = (
    "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279"
    "020005616c6c6f6300000372756e00010a0b02040041100b040042020b0b08010041000b023432"
)
TRAPPING_WASM_HEX = (
    "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279"
    "020005616c6c6f6300000372756e00010a0a02040041100b0300000b"
)
LONG_RUNNING_WASM_HEX = (
    "0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279"
    "020005616c6c6f6300000372756e00010a0f02040041100b080003400c000b000b"
)

_BUILD_DONE = False


def ensure_binaries() -> None:
    global _BUILD_DONE
    if _BUILD_DONE:
        return

    subprocess.run(["cargo", "build", "--bins"], cwd=REPO_ROOT, check=True)
    if not FROGLET_BIN.exists() or not MARKETPLACE_BIN.exists():
        raise RuntimeError("Expected compiled froglet binaries in target/debug")
    _BUILD_DONE = True


def reserve_tcp_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        return int(sock.getsockname()[1])


async def wait_for_http(url: str, timeout: float = 20.0) -> None:
    deadline = time.monotonic() + timeout
    last_error: Optional[Exception] = None

    async with aiohttp.ClientSession() as session:
        while time.monotonic() < deadline:
            try:
                async with session.get(url) as resp:
                    if resp.status == 200:
                        return
            except Exception as exc:  # pragma: no cover - startup race path
                last_error = exc
            await asyncio.sleep(0.2)

    raise RuntimeError(f"Timed out waiting for {url}. Last error: {last_error}")


@dataclass
class ManagedProcess:
    process: subprocess.Popen
    log_path: Path
    temp_root: Path

    def output(self) -> str:
        if self.log_path.exists():
            return self.log_path.read_text()
        return ""

    async def stop(self) -> None:
        if self.process.poll() is None:
            try:
                os.killpg(self.process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            except Exception:
                self.process.terminate()
            await asyncio.sleep(0.5)

        if self.process.poll() is None:
            try:
                os.killpg(self.process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except Exception:
                self.process.kill()
            await asyncio.sleep(0.2)

        shutil.rmtree(self.temp_root, ignore_errors=True)


class FrogletNode(ManagedProcess):
    def __init__(self, process: subprocess.Popen, log_path: Path, temp_root: Path, port: int, data_dir: Path):
        super().__init__(process=process, log_path=log_path, temp_root=temp_root)
        self.port = port
        self.base_url = f"http://127.0.0.1:{port}"
        self.data_dir = data_dir

    def url(self, path: str) -> str:
        return f"{self.base_url}{path}"


class MarketplaceServer(ManagedProcess):
    def __init__(self, process: subprocess.Popen, log_path: Path, temp_root: Path, port: int, db_path: Path):
        super().__init__(process=process, log_path=log_path, temp_root=temp_root)
        self.port = port
        self.base_url = f"http://127.0.0.1:{port}"
        self.db_path = db_path

    def url(self, path: str) -> str:
        return f"{self.base_url}{path}"


async def start_marketplace(*, port: Optional[int] = None, extra_env: Optional[dict[str, str]] = None) -> MarketplaceServer:
    ensure_binaries()
    port = port or reserve_tcp_port()
    temp_root = Path(tempfile.mkdtemp(prefix="froglet-marketplace-"))
    log_path = temp_root / "marketplace.log"
    db_path = temp_root / "marketplace.db"

    env = os.environ.copy()
    env.update(
        {
            "FROGLET_MARKETPLACE_LISTEN_ADDR": f"127.0.0.1:{port}",
            "FROGLET_MARKETPLACE_DB_PATH": str(db_path),
        }
    )
    if extra_env:
        env.update(extra_env)

    with log_path.open("w") as log_file:
        process = subprocess.Popen(
            [str(MARKETPLACE_BIN)],
            cwd=REPO_ROOT,
            env=env,
            stdout=log_file,
            stderr=subprocess.STDOUT,
            text=True,
            start_new_session=True,
        )

    server = MarketplaceServer(process=process, log_path=log_path, temp_root=temp_root, port=port, db_path=db_path)

    try:
        await wait_for_http(server.url("/health"))
    except Exception:
        output = server.output()
        await server.stop()
        raise RuntimeError(f"Marketplace failed to start:\n{output}")

    return server


async def start_node(
    *,
    port: Optional[int] = None,
    data_dir: Optional[Path] = None,
    extra_env: Optional[dict[str, str]] = None,
) -> FrogletNode:
    ensure_binaries()
    port = port or reserve_tcp_port()
    temp_root = Path(tempfile.mkdtemp(prefix="froglet-node-"))
    log_path = temp_root / "froglet.log"
    data_dir = data_dir or (temp_root / "data")

    env = os.environ.copy()
    env.update(
        {
            "FROGLET_NETWORK_MODE": "clearnet",
            "FROGLET_LISTEN_ADDR": f"127.0.0.1:{port}",
            "FROGLET_DATA_DIR": str(data_dir),
        }
    )
    if extra_env:
        env.update(extra_env)

    with log_path.open("w") as log_file:
        process = subprocess.Popen(
            [str(FROGLET_BIN)],
            cwd=REPO_ROOT,
            env=env,
            stdout=log_file,
            stderr=subprocess.STDOUT,
            text=True,
            start_new_session=True,
        )

    node = FrogletNode(process=process, log_path=log_path, temp_root=temp_root, port=port, data_dir=data_dir)

    try:
        await wait_for_http(node.url("/health"))
    except Exception:
        output = node.output()
        await node.stop()
        raise RuntimeError(f"Froglet failed to start:\n{output}")

    return node


SECP256K1 = curves.SECP256k1
FIELD_PRIME = SECP256K1.curve.p()
GROUP_ORDER = SECP256K1.order
GENERATOR = SECP256K1.generator


def tagged_hash(tag: str, data: bytes) -> bytes:
    tag_hash = hashlib.sha256(tag.encode("utf-8")).digest()
    return hashlib.sha256(tag_hash + tag_hash + data).digest()


def int_from_bytes(data: bytes) -> int:
    return int.from_bytes(data, "big")


def int_to_bytes(value: int) -> bytes:
    return value.to_bytes(32, "big")


def xor_bytes(left: bytes, right: bytes) -> bytes:
    return bytes(a ^ b for a, b in zip(left, right))


def has_even_y(point: ellipticcurve.Point) -> bool:
    return point.y() % 2 == 0


def lift_x(pubkey_bytes: bytes) -> Optional[ellipticcurve.Point]:
    x = int_from_bytes(pubkey_bytes)
    if x >= FIELD_PRIME:
        return None

    y_sq = (pow(x, 3, FIELD_PRIME) + 7) % FIELD_PRIME
    y = pow(y_sq, (FIELD_PRIME + 1) // 4, FIELD_PRIME)
    if pow(y, 2, FIELD_PRIME) != y_sq:
        return None
    if y % 2 == 1:
        y = FIELD_PRIME - y

    return ellipticcurve.Point(SECP256K1.curve, x, y, GROUP_ORDER)


def generate_schnorr_signing_key() -> bytes:
    while True:
        candidate = os.urandom(32)
        secret = int_from_bytes(candidate)
        if 1 <= secret < GROUP_ORDER:
            return candidate


def schnorr_pubkey_hex(secret_key: bytes) -> str:
    secret = int_from_bytes(secret_key)
    point = secret * GENERATOR
    return int_to_bytes(point.x()).hex()


def schnorr_sign_message(secret_key: bytes, message: bytes) -> str:
    secret = int_from_bytes(secret_key)
    if not 1 <= secret < GROUP_ORDER:
        raise ValueError("invalid secp256k1 secret key")

    message_digest = hashlib.sha256(message).digest()
    point = secret * GENERATOR
    secret_scalar = secret if has_even_y(point) else GROUP_ORDER - secret
    pubkey_bytes = int_to_bytes(point.x())

    aux = bytes(32)
    t = xor_bytes(int_to_bytes(secret_scalar), tagged_hash("BIP0340/aux", aux))
    nonce = int_from_bytes(
        tagged_hash("BIP0340/nonce", t + pubkey_bytes + message_digest)
    ) % GROUP_ORDER
    if nonce == 0:
        raise ValueError("derived invalid Schnorr nonce")

    nonce_point = nonce * GENERATOR
    signing_nonce = nonce if has_even_y(nonce_point) else GROUP_ORDER - nonce
    nonce_x = int_to_bytes(nonce_point.x())
    challenge = int_from_bytes(
        tagged_hash("BIP0340/challenge", nonce_x + pubkey_bytes + message_digest)
    ) % GROUP_ORDER
    signature = nonce_x + int_to_bytes(
        (signing_nonce + challenge * secret_scalar) % GROUP_ORDER
    )
    return signature.hex()


def schnorr_verify_message(pubkey_hex: str, signature_hex: str, message: bytes) -> bool:
    try:
        pubkey_bytes = bytes.fromhex(pubkey_hex)
        signature = bytes.fromhex(signature_hex)
    except ValueError:
        return False

    if len(pubkey_bytes) != 32 or len(signature) != 64:
        return False

    message_digest = hashlib.sha256(message).digest()
    point = lift_x(pubkey_bytes)
    if point is None:
        return False

    r = int_from_bytes(signature[:32])
    s = int_from_bytes(signature[32:])
    if r >= FIELD_PRIME or s >= GROUP_ORDER:
        return False

    challenge = int_from_bytes(
        tagged_hash("BIP0340/challenge", signature[:32] + pubkey_bytes + message_digest)
    ) % GROUP_ORDER
    candidate = s * GENERATOR + ((GROUP_ORDER - challenge) % GROUP_ORDER) * point
    if candidate == ellipticcurve.INFINITY or not has_even_y(candidate):
        return False

    return candidate.x() == r


SIGNING_KEY = generate_schnorr_signing_key()
PUBKEY_HEX = schnorr_pubkey_hex(SIGNING_KEY)


def create_signed_event(content: str, *, kind: str = "market.listing", tags: Optional[list[list[str]]] = None) -> dict:
    content_bytes = content.encode("utf-8")
    created_at = int(time.time())
    event_id = __import__("hashlib").sha256(content_bytes).hexdigest()
    event = {
        "id": event_id,
        "pubkey": PUBKEY_HEX,
        "created_at": created_at,
        "kind": kind,
        "tags": tags or [["t", "test"]],
        "content": content,
    }
    signature = schnorr_sign_message(SIGNING_KEY, canonical_event_signing_bytes(event))
    event["sig"] = signature
    return event


def canonical_event_signing_bytes(event: dict) -> bytes:
    return json.dumps(
        [
            event["id"],
            event["pubkey"],
            event["created_at"],
            event["kind"],
            event["tags"],
            event["content"],
        ],
        separators=(",", ":"),
    ).encode("utf-8")


def canonical_artifact_signing_bytes(artifact: dict) -> bytes:
    return canonical_json_bytes(
        [
            artifact["kind"],
            artifact["actor_id"],
            artifact["created_at"],
            artifact["payload_hash"],
            artifact["payload"],
        ]
    )


def canonical_json_bytes(value: object) -> bytes:
    return json.dumps(
        value,
        separators=(",", ":"),
        sort_keys=True,
        ensure_ascii=False,
        allow_nan=False,
    ).encode("utf-8")


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def build_wasm_submission(module_hex: str, *, input: object = None) -> dict:
    input_value = input if input is not None else None
    module_bytes = bytes.fromhex(module_hex)
    return {
        "schema_version": "froglet/v1",
        "submission_type": "wasm_submission",
        "workload": {
            "schema_version": "froglet/v1",
            "workload_kind": "compute.wasm.v1",
            "abi_version": "froglet.wasm.run_json.v1",
            "module_format": "application/wasm",
            "module_hash": sha256_hex(module_bytes),
            "input_format": "application/json+jcs",
            "input_hash": sha256_hex(canonical_json_bytes(input_value)),
            "requested_capabilities": [],
        },
        "module_bytes_hex": module_hex,
        "input": input_value,
    }


def build_wasm_request(module_hex: str, *, input: object = None) -> dict:
    return {
        "kind": "wasm",
        "submission": build_wasm_submission(module_hex, input=input),
    }


def workload_hash_from_submission(submission: dict) -> str:
    return sha256_hex(canonical_json_bytes(submission["workload"]))


def read_db_row(db_path: Path, query: str, params: tuple[object, ...]) -> tuple:
    conn = sqlite3.connect(db_path)
    try:
        row = conn.execute(query, params).fetchone()
    finally:
        conn.close()
    if row is None:
        raise AssertionError(f"no row returned for query: {query}")
    return row


def read_db_rows(db_path: Path, query: str, params: tuple[object, ...]) -> list[tuple]:
    conn = sqlite3.connect(db_path)
    try:
        rows = conn.execute(query, params).fetchall()
    finally:
        conn.close()
    return rows


def execute_db(db_path: Path, query: str, params: tuple[object, ...]) -> None:
    conn = sqlite3.connect(db_path)
    try:
        conn.execute(query, params)
        conn.commit()
    finally:
        conn.close()


def verify_signed_artifact(artifact: dict) -> bool:
    payload_bytes = canonical_json_bytes(artifact["payload"])
    if sha256_hex(payload_bytes) != artifact["payload_hash"]:
        return False

    signing_bytes = canonical_artifact_signing_bytes(artifact)
    if sha256_hex(signing_bytes) != artifact["hash"]:
        return False

    return schnorr_verify_message(
        artifact["actor_id"],
        artifact["signature"],
        signing_bytes,
    )


class FrogletAsyncTestCase(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        await asyncio.to_thread(ensure_binaries)
        asyncio.get_running_loop().slow_callback_duration = 1.0

    async def start_node(self, **kwargs) -> FrogletNode:
        node = await start_node(**kwargs)
        self.addAsyncCleanup(node.stop)
        return node

    async def start_marketplace(self, **kwargs) -> MarketplaceServer:
        marketplace = await start_marketplace(**kwargs)
        self.addAsyncCleanup(marketplace.stop)
        return marketplace

    async def wait_for_job(self, node: FrogletNode, job_id: str, timeout: float = 15.0) -> dict:
        deadline = time.monotonic() + timeout

        async with aiohttp.ClientSession() as session:
            while time.monotonic() < deadline:
                async with session.get(node.url(f"/v1/node/jobs/{job_id}")) as resp:
                    payload = await resp.json()
                if payload["status"] in {"succeeded", "failed"}:
                    return payload
                await asyncio.sleep(0.2)

        raise RuntimeError(f"Timed out waiting for job {job_id}")

    async def wait_for_deal(self, node: FrogletNode, deal_id: str, timeout: float = 15.0) -> dict:
        deadline = time.monotonic() + timeout

        async with aiohttp.ClientSession() as session:
            while time.monotonic() < deadline:
                async with session.get(node.url(f"/v1/deals/{deal_id}")) as resp:
                    payload = await resp.json()
                if payload["status"] in {"succeeded", "failed", "rejected"}:
                    return payload
                await asyncio.sleep(0.2)

        raise RuntimeError(f"Timed out waiting for deal {deal_id}")
