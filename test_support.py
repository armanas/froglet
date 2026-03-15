import asyncio
import hashlib
import os
import shutil
import signal
import socket
import sqlite3
import ssl
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
    def __init__(
        self,
        process: subprocess.Popen,
        log_path: Path,
        temp_root: Path,
        port: int,
        runtime_port: int,
        data_dir: Path,
    ):
        super().__init__(process=process, log_path=log_path, temp_root=temp_root)
        self.port = port
        self.base_url = f"http://127.0.0.1:{port}"
        self.runtime_port = runtime_port
        self.runtime_url = f"http://127.0.0.1:{runtime_port}"
        self.data_dir = data_dir

    def url(self, path: str) -> str:
        if path.startswith("/v1/runtime/"):
            return f"{self.runtime_url}{path}"
        return f"{self.base_url}{path}"


class MarketplaceServer(ManagedProcess):
    def __init__(self, process: subprocess.Popen, log_path: Path, temp_root: Path, port: int, db_path: Path):
        super().__init__(process=process, log_path=log_path, temp_root=temp_root)
        self.port = port
        self.base_url = f"http://127.0.0.1:{port}"
        self.db_path = db_path

    def url(self, path: str) -> str:
        return f"{self.base_url}{path}"


@dataclass
class RegtestLndNode:
    name: str
    alias: str
    rest_port: int
    rpc_port: int
    data_dir: Path

    @property
    def tls_cert_path(self) -> Path:
        return self.data_dir / "tls.cert"

    @property
    def admin_macaroon_path(self) -> Path:
        return self.data_dir / "data" / "chain" / "bitcoin" / "regtest" / "admin.macaroon"


class LndRegtestCluster:
    def __init__(
        self,
        *,
        temp_root: Path,
        network_name: str,
        bitcoind_name: str,
        nodes: dict[str, RegtestLndNode],
    ) -> None:
        self.temp_root = temp_root
        self.network_name = network_name
        self.bitcoind_name = bitcoind_name
        self.nodes = nodes
        self._payment_processes: list[subprocess.Popen[str]] = []

    async def stop(self) -> None:
        for proc in self._payment_processes:
            if proc.poll() is None:
                proc.terminate()
                try:
                    await asyncio.to_thread(proc.wait, 5)
                except Exception:
                    proc.kill()

        for node in self.nodes.values():
            await self._docker("rm", "-f", node.name, check=False)
        await self._docker("rm", "-f", self.bitcoind_name, check=False)
        await self._docker("network", "rm", self.network_name, check=False)
        shutil.rmtree(self.temp_root, ignore_errors=True)

    def lightning_env(self, node_key: str) -> dict[str, str]:
        node = self.nodes[node_key]
        return {
            "FROGLET_PAYMENT_BACKEND": "lightning",
            "FROGLET_LIGHTNING_MODE": "lnd_rest",
            "FROGLET_LIGHTNING_REST_URL": f"https://127.0.0.1:{node.rest_port}",
            "FROGLET_LIGHTNING_TLS_CERT_PATH": str(node.tls_cert_path),
            "FROGLET_LIGHTNING_MACAROON_PATH": str(node.admin_macaroon_path),
            "FROGLET_LIGHTNING_REQUEST_TIMEOUT_SECS": "10",
            "FROGLET_LIGHTNING_SYNC_INTERVAL_MS": "100",
        }

    async def pay_invoice(self, payer_key: str, invoice: str, *, timeout: str = "60s") -> str:
        return await self._lncli(
            payer_key,
            "payinvoice",
            "--force",
            "--timeout",
            timeout,
            invoice,
        )

    def pay_invoice_async(
        self, payer_key: str, invoice: str, *, timeout: str = "60s"
    ) -> subprocess.Popen[str]:
        node = self.nodes[payer_key]
        proc = subprocess.Popen(
            [
                "docker",
                "exec",
                node.name,
                "lncli",
                "--network",
                "regtest",
                "payinvoice",
                "--force",
                "--timeout",
                timeout,
                invoice,
            ],
            cwd=REPO_ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        self._payment_processes.append(proc)
        return proc

    async def wait_payment_process(
        self, proc: subprocess.Popen[str], timeout: float = 30.0
    ) -> tuple[int, str, str]:
        def communicate() -> tuple[int, str, str]:
            stdout, stderr = proc.communicate(timeout=timeout)
            return proc.returncode, stdout, stderr

        try:
            return await asyncio.to_thread(communicate)
        except subprocess.TimeoutExpired as exc:
            proc.kill()
            stdout, stderr = proc.communicate()
            raise RuntimeError(
                f"Timed out waiting for lncli payinvoice process: stdout={stdout}\nstderr={stderr}"
            ) from exc

    async def settle_hold_invoice(self, node_key: str, preimage_hex: str) -> None:
        await self._lncli(node_key, "settleinvoice", preimage_hex)

    async def cancel_hold_invoice(self, node_key: str, payment_hash_hex: str) -> None:
        await self._lncli(node_key, "cancelinvoice", payment_hash_hex)

    async def wait_invoice_state(
        self,
        node_key: str,
        payment_hash_hex: str,
        expected_state: str,
        timeout: float = 30.0,
    ) -> dict:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            invoice = await self.lookup_invoice(node_key, payment_hash_hex)
            if invoice.get("state") == expected_state:
                return invoice
            await asyncio.sleep(0.5)
        raise RuntimeError(
            f"Timed out waiting for invoice {payment_hash_hex} to reach {expected_state}"
        )

    async def lookup_invoice(self, node_key: str, payment_hash_hex: str) -> dict:
        output = await self._lncli(node_key, "lookupinvoice", payment_hash_hex)
        return json.loads(output)

    async def _run(
        self,
        args: list[str],
        *,
        check: bool = True,
    ) -> subprocess.CompletedProcess[str]:
        proc = await asyncio.to_thread(
            subprocess.run,
            args,
            cwd=REPO_ROOT,
            capture_output=True,
            text=True,
        )
        if check and proc.returncode != 0:
            raise RuntimeError(
                f"Command failed ({proc.returncode}): {' '.join(args)}\nstdout:\n{proc.stdout}\nstderr:\n{proc.stderr}"
            )
        return proc

    async def _docker(self, *args: str, check: bool = True) -> str:
        proc = await self._run(["docker", *args], check=check)
        return proc.stdout.strip()

    async def _bitcoin_cli(self, *args: str, wallet: Optional[str] = None) -> str:
        cmd = [
            "docker",
            "exec",
            self.bitcoind_name,
            "bitcoin-cli",
            "-regtest",
            "-rpcuser=user",
            "-rpcpassword=pass",
        ]
        if wallet:
            cmd.append(f"-rpcwallet={wallet}")
        cmd.extend(args)
        proc = await self._run(cmd)
        return proc.stdout.strip()

    async def _lncli(self, node_key: str, *args: str) -> str:
        node = self.nodes[node_key]
        proc = await self._run(
            ["docker", "exec", node.name, "lncli", "--network", "regtest", *args]
        )
        return proc.stdout.strip()


async def start_lnd_regtest_cluster() -> LndRegtestCluster:
    temp_root = Path(tempfile.mkdtemp(prefix="froglet-lnd-regtest-"))
    network_name = f"froglet-lnd-regtest-{os.getpid()}-{int(time.time() * 1000)}"
    bitcoind_name = f"{network_name}-bitcoind"
    nodes = {
        "alice": RegtestLndNode(
            name=f"{network_name}-alice",
            alias="alice",
            rest_port=reserve_tcp_port(),
            rpc_port=reserve_tcp_port(),
            data_dir=temp_root / "alice",
        ),
        "bob": RegtestLndNode(
            name=f"{network_name}-bob",
            alias="bob",
            rest_port=reserve_tcp_port(),
            rpc_port=reserve_tcp_port(),
            data_dir=temp_root / "bob",
        ),
    }
    for node in nodes.values():
        node.data_dir.mkdir(parents=True, exist_ok=True)

    cluster = LndRegtestCluster(
        temp_root=temp_root,
        network_name=network_name,
        bitcoind_name=bitcoind_name,
        nodes=nodes,
    )

    try:
        await cluster._docker("network", "create", network_name)
        await cluster._docker(
            "run",
            "-d",
            "--name",
            bitcoind_name,
            "--network",
            network_name,
            "-v",
            f"{temp_root / 'bitcoind'}:/home/bitcoin/.bitcoin",
            "ruimarinho/bitcoin-core:24",
            "-regtest=1",
            "-server=1",
            "-txindex=1",
            "-fallbackfee=0.0002",
            "-rpcbind=0.0.0.0",
            "-rpcallowip=0.0.0.0/0",
            "-rpcuser=user",
            "-rpcpassword=pass",
            "-zmqpubrawblock=tcp://0.0.0.0:28332",
            "-zmqpubrawtx=tcp://0.0.0.0:28333",
        )
        await _wait_for(
            lambda: cluster._bitcoin_cli("getblockchaininfo"),
            timeout=30.0,
            description="bitcoind RPC",
        )
        await cluster._bitcoin_cli("createwallet", "miner")

        for node in nodes.values():
            await cluster._docker(
                "run",
                "-d",
                "--name",
                node.name,
                "--network",
                network_name,
                "-p",
                f"127.0.0.1:{node.rest_port}:8080",
                "-p",
                f"127.0.0.1:{node.rpc_port}:10009",
                "-v",
                f"{node.data_dir}:/root/.lnd",
                "lightninglabs/lnd:v0.20.0-beta",
                "--noseedbackup",
                "--trickledelay=50",
                "--bitcoin.active",
                "--bitcoin.regtest",
                "--bitcoin.node=bitcoind",
                f"--bitcoind.rpchost={bitcoind_name}",
                "--bitcoind.rpcuser=user",
                "--bitcoind.rpcpass=pass",
                f"--bitcoind.zmqpubrawblock=tcp://{bitcoind_name}:28332",
                f"--bitcoind.zmqpubrawtx=tcp://{bitcoind_name}:28333",
                "--rpclisten=0.0.0.0:10009",
                "--restlisten=0.0.0.0:8080",
                "--listen=0.0.0.0:9735",
                "--tlsextradomain=localhost",
                "--tlsextraip=127.0.0.1",
            )

        for node_key in nodes:
            await _wait_for(
                lambda node_key=node_key: cluster._lncli(node_key, "getinfo"),
                timeout=45.0,
                description=f"{node_key} lncli",
            )
            await _wait_for_path(
                nodes[node_key].admin_macaroon_path,
                timeout=45.0,
                description=f"{node_key} admin macaroon",
            )

        alice_address = json.loads(await cluster._lncli("alice", "newaddress", "p2wkh"))[
            "address"
        ]
        await cluster._bitcoin_cli("generatetoaddress", "110", alice_address, wallet="miner")

        await _wait_for_chain_sync(cluster, "alice", min_height=110)
        await _wait_for_chain_sync(cluster, "bob", min_height=110)

        bob_info = json.loads(await cluster._lncli("bob", "getinfo"))
        await _wait_for(
            lambda: _connect_lnd_peer(cluster, bob_info["identity_pubkey"], nodes["bob"].name),
            timeout=45.0,
            description="alice connects to bob",
        )
        await _wait_for(
            lambda: _open_lnd_channel(cluster, bob_info["identity_pubkey"]),
            timeout=45.0,
            description="alice opens channel to bob",
        )

        miner_address = await cluster._bitcoin_cli("getnewaddress", wallet="miner")
        await cluster._bitcoin_cli("generatetoaddress", "6", miner_address, wallet="miner")
        await _wait_for_active_channel(cluster, "alice")
        await _wait_for_active_channel(cluster, "bob")
        for node_key in nodes:
            await _wait_for_lnd_rest_ready(cluster, node_key)
        return cluster
    except Exception:
        await cluster.stop()
        raise


async def _wait_for(
    operation,
    *,
    timeout: float,
    description: str,
) -> str:
    deadline = time.monotonic() + timeout
    last_error: Optional[Exception] = None
    while time.monotonic() < deadline:
        try:
            return await operation()
        except Exception as exc:  # pragma: no cover - startup race path
            last_error = exc
        await asyncio.sleep(0.5)
    raise RuntimeError(f"Timed out waiting for {description}: {last_error}")


async def _wait_for_path(path: Path, *, timeout: float, description: str) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            return
        await asyncio.sleep(0.5)
    raise RuntimeError(f"Timed out waiting for {description} at {path}")


async def _wait_for_chain_sync(
    cluster: LndRegtestCluster, node_key: str, *, min_height: int, timeout: float = 60.0
) -> None:
    async def op() -> str:
        info = json.loads(await cluster._lncli(node_key, "getinfo"))
        if info.get("synced_to_chain") and int(info.get("block_height", 0)) >= min_height:
            return "ok"
        raise RuntimeError(f"not yet synced: {info}")

    await _wait_for(op, timeout=timeout, description=f"{node_key} chain sync")


async def _wait_for_active_channel(
    cluster: LndRegtestCluster, node_key: str, timeout: float = 60.0
) -> None:
    async def op() -> str:
        channels = json.loads(await cluster._lncli(node_key, "listchannels"))
        if channels.get("channels") and all(ch.get("active") for ch in channels["channels"]):
            return "ok"
        raise RuntimeError(f"channels not active yet: {channels}")

    await _wait_for(op, timeout=timeout, description=f"{node_key} active channel")


async def _connect_lnd_peer(
    cluster: LndRegtestCluster, identity_pubkey: str, host_name: str
) -> str:
    try:
        return await cluster._lncli(
            "alice",
            "connect",
            f"{identity_pubkey}@{host_name}:9735",
        )
    except RuntimeError as exc:
        message = str(exc)
        if "already connected to peer" in message.lower():
            return "already connected"
        raise


async def _open_lnd_channel(cluster: LndRegtestCluster, identity_pubkey: str) -> str:
    try:
        return await cluster._lncli("alice", "openchannel", identity_pubkey, "1000000")
    except RuntimeError as exc:
        message = str(exc)
        if "server is still in the process of starting" in message.lower():
            raise
        if "channel with peer already exists" in message.lower():
            return "already open"
        raise


async def _wait_for_lnd_rest_ready(
    cluster: LndRegtestCluster, node_key: str, timeout: float = 45.0
) -> None:
    node = cluster.nodes[node_key]
    macaroon_hex = node.admin_macaroon_path.read_bytes().hex()
    ssl_context = ssl.create_default_context(cafile=str(node.tls_cert_path))

    async def op() -> str:
        async with aiohttp.ClientSession(
            connector=aiohttp.TCPConnector(ssl=ssl_context)
        ) as session:
            async with session.get(
                f"https://127.0.0.1:{node.rest_port}/v1/getinfo",
                headers={"Grpc-Metadata-macaroon": macaroon_hex},
            ) as resp:
                if resp.status != 200:
                    body = await resp.text()
                    raise RuntimeError(f"unexpected status {resp.status}: {body}")
                payload = await resp.json()
                if payload.get("identity_pubkey"):
                    return "ok"
                raise RuntimeError(f"missing identity_pubkey in payload: {payload}")

    await _wait_for(op, timeout=timeout, description=f"{node_key} lnd rest")


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
    runtime_port: Optional[int] = None,
    tor_backend_port: Optional[int] = None,
    data_dir: Optional[Path] = None,
    extra_env: Optional[dict[str, str]] = None,
) -> FrogletNode:
    ensure_binaries()
    port = port or reserve_tcp_port()
    runtime_port = runtime_port or reserve_tcp_port()
    tor_backend_port = tor_backend_port or reserve_tcp_port()
    temp_root = Path(tempfile.mkdtemp(prefix="froglet-node-"))
    log_path = temp_root / "froglet.log"
    data_dir = data_dir or (temp_root / "data")

    env = os.environ.copy()
    env.update(
        {
            "FROGLET_NETWORK_MODE": "clearnet",
            "FROGLET_LISTEN_ADDR": f"127.0.0.1:{port}",
            "FROGLET_RUNTIME_LISTEN_ADDR": f"127.0.0.1:{runtime_port}",
            "FROGLET_TOR_BACKEND_LISTEN_ADDR": f"127.0.0.1:{tor_backend_port}",
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

    node = FrogletNode(
        process=process,
        log_path=log_path,
        temp_root=temp_root,
        port=port,
        runtime_port=runtime_port,
        data_dir=data_dir,
    )

    try:
        network_mode = env.get("FROGLET_NETWORK_MODE", "clearnet").lower()
        if network_mode in {"clearnet", "dual"}:
            await wait_for_http(node.url("/health"))
        await wait_for_http(f"{node.runtime_url}/health")
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
    created_at = int(time.time())
    event = {
        "id": "",
        "pubkey": PUBKEY_HEX,
        "created_at": created_at,
        "kind": kind,
        "tags": tags or [["t", "test"]],
        "content": content,
    }
    event["id"] = canonical_event_id(event)
    signature = schnorr_sign_message(SIGNING_KEY, canonical_event_signing_bytes(event))
    event["sig"] = signature
    return event


def canonical_event_id(event: dict) -> str:
    return hashlib.sha256(canonical_event_id_bytes(event)).hexdigest()


def canonical_event_id_bytes(event: dict) -> bytes:
    return json.dumps(
        [
            event["pubkey"],
            event["created_at"],
            event["kind"],
            event["tags"],
            event["content"],
        ],
        separators=(",", ":"),
    ).encode("utf-8")


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
            artifact["schema_version"],
            artifact["artifact_type"],
            artifact["signer"],
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


def sign_artifact(
    artifact_type: str,
    payload: dict,
    *,
    secret_key: bytes = SIGNING_KEY,
    created_at: Optional[int] = None,
) -> dict:
    signer = schnorr_pubkey_hex(secret_key)
    issued_at = created_at if created_at is not None else int(time.time())
    payload_hash = sha256_hex(canonical_json_bytes(payload))
    artifact = {
        "artifact_type": artifact_type,
        "schema_version": "froglet/v1",
        "signer": signer,
        "created_at": issued_at,
        "payload_hash": payload_hash,
        "payload": payload,
    }
    signing_bytes = canonical_artifact_signing_bytes(artifact)
    artifact["hash"] = sha256_hex(signing_bytes)
    artifact["signature"] = schnorr_sign_message(secret_key, signing_bytes)
    return artifact


def sign_deal_artifact_from_quote(
    quote: dict,
    requester_secret_key: bytes,
    *,
    success_payment_hash: str,
    created_at: Optional[int] = None,
) -> dict:
    issued_at = created_at if created_at is not None else int(time.time())
    runtime_ms = int(quote["payload"]["execution_limits"]["max_runtime_ms"])
    execution_window_secs = max(1, (runtime_ms + 999) // 1000)
    settlement_terms = quote["payload"]["settlement_terms"]
    total_msat = int(settlement_terms["base_fee_msat"]) + int(
        settlement_terms["success_fee_msat"]
    )
    if (
        settlement_terms["method"] == "lightning.base_fee_plus_success_fee.v1"
        and total_msat > 0
    ):
        quote_expires_at = int(quote["payload"]["expires_at"])
        hold_window_secs = int(settlement_terms["max_success_hold_expiry_secs"])
        admission_window_secs = max(
            int(settlement_terms["max_base_invoice_expiry_secs"]),
            hold_window_secs,
        )
        latest_admission_deadline = quote_expires_at - execution_window_secs - hold_window_secs
        admission_deadline = min(
            latest_admission_deadline,
            issued_at + admission_window_secs,
        )
        if admission_deadline < issued_at:
            raise ValueError(
                "quote no longer has enough time for the Lightning execution and acceptance windows"
            )
        completion_deadline = admission_deadline + execution_window_secs
        acceptance_deadline = completion_deadline + hold_window_secs
    else:
        admission_deadline = int(quote["payload"]["expires_at"])
        completion_deadline = admission_deadline + execution_window_secs
        acceptance_deadline = completion_deadline
    payload = {
        "requester_id": schnorr_pubkey_hex(requester_secret_key),
        "provider_id": quote["payload"]["provider_id"],
        "quote_hash": quote["hash"],
        "workload_hash": quote["payload"]["workload_hash"],
        "success_payment_hash": success_payment_hash,
        "admission_deadline": admission_deadline,
        "completion_deadline": completion_deadline,
        "acceptance_deadline": acceptance_deadline,
    }
    return sign_artifact(
        "deal",
        payload,
        secret_key=requester_secret_key,
        created_at=issued_at,
    )


def default_success_payment_hash(label: str) -> str:
    return sha256_hex(label.encode("utf-8"))


async def create_protocol_quote(
    session: aiohttp.ClientSession,
    node: FrogletNode,
    *,
    offer_id: str,
    request: dict,
    requester_secret_key: bytes,
    max_price_sats: Optional[int] = None,
) -> dict:
    payload = {
        "offer_id": offer_id,
        "requester_id": schnorr_pubkey_hex(requester_secret_key),
        **request,
    }
    if max_price_sats is not None:
        payload["max_price_sats"] = max_price_sats
    async with session.post(node.url("/v1/quotes"), json=payload) as resp:
        quote = await resp.json()
    if resp.status != 201:
        raise AssertionError(f"expected 201 from /v1/quotes, got {resp.status}: {quote}")
    return quote


async def create_protocol_deal(
    session: aiohttp.ClientSession,
    node: FrogletNode,
    *,
    quote: dict,
    request: dict,
    requester_secret_key: bytes,
    idempotency_key: Optional[str] = None,
    payment: Optional[dict] = None,
    success_payment_hash: Optional[str] = None,
    expected_statuses: tuple[int, ...] = (200, 202),
) -> dict:
    deal = sign_deal_artifact_from_quote(
        quote,
        requester_secret_key,
        success_payment_hash=success_payment_hash
        or default_success_payment_hash(idempotency_key or quote["hash"]),
    )
    payload = {
        "quote": quote,
        "deal": deal,
        **request,
    }
    if idempotency_key is not None:
        payload["idempotency_key"] = idempotency_key
    if payment is not None:
        payload["payment"] = payment

    async with session.post(node.url("/v1/deals"), json=payload) as resp:
        created = await resp.json()
    if resp.status not in expected_statuses:
        raise AssertionError(
            f"expected {expected_statuses} from /v1/deals, got {resp.status}: {created}"
        )
    return created


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
        artifact["signer"],
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

    async def wait_for_deal_status(
        self,
        node: FrogletNode,
        deal_id: str,
        statuses: set[str] | frozenset[str],
        timeout: float = 15.0,
    ) -> dict:
        deadline = time.monotonic() + timeout

        async with aiohttp.ClientSession() as session:
            while time.monotonic() < deadline:
                async with session.get(node.url(f"/v1/deals/{deal_id}")) as resp:
                    payload = await resp.json()
                if payload["status"] in statuses:
                    return payload
                await asyncio.sleep(0.2)

        raise RuntimeError(f"Timed out waiting for deal {deal_id} to reach one of {statuses}")

    async def wait_for_deal_status_in_db(
        self,
        node: FrogletNode,
        deal_id: str,
        statuses: set[str] | frozenset[str],
        timeout: float = 15.0,
    ) -> str:
        deadline = time.monotonic() + timeout

        while time.monotonic() < deadline:
            row = read_db_row(
                node.data_dir / "node.db",
                "SELECT status FROM deals WHERE deal_id = ?",
                (deal_id,),
            )
            status = row[0]
            if status in statuses:
                return status
            await asyncio.sleep(0.2)

        raise RuntimeError(
            f"Timed out waiting for deal {deal_id} in DB to reach one of {statuses}"
        )
