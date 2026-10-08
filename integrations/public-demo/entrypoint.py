"""Supervise the pinned native node and its narrow public ingress."""
import hashlib
import json
import os
from pathlib import Path
import signal
import stat
import subprocess
import sys
import time
import tomllib
import urllib.request

ROOT = Path('/state')
DATA = ROOT / 'node'
BINARY = Path('/app/froglet-node')
BINARY_SHA = '5afdbaf98f7aa2eb10cff3c29dbeca25d099e421a46a4cff40439aeaf3a25b77'
children = []


def verify_approved_http_policy():
    """Optional named tools require the exact public profile and HTTP policy."""
    from provider_proxy import approved_profile_settings, load_approved_services

    settings = approved_profile_settings()
    if settings is None:
        return
    expected = ROOT / 'expected-provider-id'
    if expected.is_symlink() or not expected.is_file():
        raise RuntimeError('approved-http-existing-identity-required')
    services = load_approved_services(*settings, expected.read_text().strip(), root=ROOT)
    if not services:
        return
    policy_path = os.environ.get('FROGLET_WASM_POLICY_PATH')
    policy_sha = os.environ.get('FROGLET_PUBLIC_DEMO_HTTP_POLICY_SHA256')
    if policy_path != str(ROOT / 'approved-http-policy.toml') or not policy_sha:
        raise RuntimeError('approved-http-exact-policy-required')
    descriptor = os.open(policy_path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, 'rb') as file:
        info = os.fstat(file.fileno())
        if (not stat.S_ISREG(info.st_mode) or info.st_uid not in {0, os.geteuid()}
                or info.st_mode & 0o022 or not 0 < info.st_size <= 32768):
            raise RuntimeError('approved-http-policy-not-protected')
        raw = file.read(32769)
    if hashlib.sha256(raw).hexdigest() != policy_sha:
        raise RuntimeError('approved-http-policy-digest-mismatch')
    policy = tomllib.loads(raw.decode('utf-8'))
    expected_http = {
        'operations_only': True,
        'operation_hashes': sorted(service['operation_hash'] for service in services.values()),
        'allowed_hosts': ['froglet.dev'],
        'allow_private_networks': False,
        'max_calls_per_execution': 1,
        'max_timeout_ms': 2000,
        'max_request_body_bytes': 2048,
        'max_response_body_bytes': 131072,
        'max_redirects': 0,
    }
    # No auth profiles, general HTTP, extra hosts or capabilities enter this
    # credential-free public tool profile. Preserve the existing native limits.
    actual_http = policy.get('http')
    if (set(policy) != {'http'} or not isinstance(actual_http, dict)
            or set(actual_http) != set(expected_http)
            or any(type(actual_http[key]) is not type(value)
                   or actual_http[key] != value for key, value in expected_http.items())):
        raise RuntimeError('approved-http-policy-scope-mismatch')


def stop(*_):
    for child in children:
        if child.poll() is None:
            child.terminate()
    for child in children:
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)
    raise SystemExit(0)


def main():
    os.umask(0o077)
    if ROOT.is_symlink() or not ROOT.is_dir():
        raise RuntimeError('persistent-volume-required')
    if os.geteuid() == 0:
        os.chown(ROOT, 10001, 10001)
        os.chmod(ROOT, 0o700)
        os.setgid(10001)
        os.setuid(10001)
    if os.geteuid() != 10001 or os.getegid() != 10001:
        raise RuntimeError('expected-unprivileged-user-required')
    if (ROOT / 'expected-provider-id').exists():
        for path in (ROOT / 'traffic.db', DATA / 'node.db', DATA / 'identity/secp256k1.seed'):
            if path.is_symlink() or not path.is_file():
                raise RuntimeError('existing-state-required-no-refill')
    if hashlib.sha256(BINARY.read_bytes()).hexdigest() != BINARY_SHA:
        raise RuntimeError('released-runtime-digest-mismatch')
    verify_approved_http_policy()
    os.environ.update({
        'FROGLET_DATA_ROOT': str(DATA), 'FROGLET_DB_PATH': str(DATA / 'node.db'),
        'FROGLET_PROVIDER_CONTROL_TOKEN_PATH': str(DATA / 'runtime/froglet-control.token'),
        'FROGLET_DAEMON_URL': 'http://127.0.0.1:8080',
        'FROGLET_NODE_ROLE': 'dual', 'FROGLET_NETWORK_MODE': 'clearnet',
        'FROGLET_LISTEN_ADDR': '127.0.0.1:8080', 'FROGLET_RUNTIME_LISTEN_ADDR': '127.0.0.1:8081',
        'FROGLET_RUNTIME_ALLOW_NON_LOOPBACK': 'false', 'FROGLET_EXECUTION_TIMEOUT_SECS': '2',
        'FROGLET_PROVIDER_ACCESS_MODE': 'trial', 'FROGLET_PROVIDER_REQUIRE_PAYMENT': 'false',
        'FROGLET_PROVIDER_MAX_TOTAL_DEALS': '1000', 'FROGLET_PROVIDER_MAX_TOTAL_QUOTES': '3000',
        'FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS': '2000000',
        'FROGLET_PROVIDER_MIN_FREE_BYTES': '67108864', 'FROGLET_PROVIDER_MAX_DATABASE_BYTES': '268435456',
    })
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    node = subprocess.Popen([str(BINARY)])
    children.append(node)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    for _ in range(300):
        if node.poll() is not None:
            raise RuntimeError('native-node-exited')
        try:
            with opener.open('http://127.0.0.1:8080/health', timeout=1) as r:
                if json.load(r).get('status') == 'ok':
                    break
        except OSError:
            pass
        time.sleep(.1)
    else:
        raise RuntimeError('native-node-not-ready')
    with opener.open('http://127.0.0.1:8080/v1/provider/services', timeout=3) as r:
        services = json.load(r).get('services', [])
    if not any(s.get('service_id') == 'synthetic-terminology-demo' for s in services):
        project = ROOT / 'catalog-project'
        request_path = ROOT / 'catalog-request.json'
        request_path.write_text(json.dumps({
            'source': '/app/catalog-source.json', 'destination': str(project),
            'service_id': 'synthetic-terminology-demo',
            'summary': 'Five synthetic terminology rows; exact string lookup; no scientific ground truth.',
            'selection': {'terminology': ['source', 'target']},
            'example_input': {'op': 'select', 'collection': 'terminology', 'columns': ['source', 'target'], 'limit': 100},
        }))
        # All preparation/publication logs stay private on this new volume.
        with (ROOT / 'publication.log').open('ab') as log:
            subprocess.run([str(BINARY), 'prepare-service', '--request', str(request_path), '--json'], check=True, stdout=log, stderr=log, timeout=30)
            subprocess.run([str(BINARY), 'publish', '--host', 'local', '--json'], cwd=project, check=True, stdout=log, stderr=log, timeout=45)
    proxy = subprocess.Popen([sys.executable, '/app/provider_proxy.py'])
    children.append(proxy)
    while all(child.poll() is None for child in children):
        time.sleep(.2)
    raise RuntimeError('supervised-child-exited')


if __name__ == '__main__':
    try:
        main()
    finally:
        for child in children:
            if child.poll() is None:
                child.terminate()
        for child in children:
            try:
                child.wait(timeout=10)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=5)
