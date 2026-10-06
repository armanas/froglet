"""Narrow public ingress for an unchanged native Froglet node on loopback.

Only free pure-Wasm jobs and the selected synthetic catalog are exposed.
Operator/runtime endpoints, credentials, Python/container execution and host
capabilities never pass this boundary. Native Froglet verifies signed records
and atomically enforces execution allowances; this service caps HTTP traffic.
"""
import hashlib
from contextlib import contextmanager
import http.server
import json
import math
import os
from pathlib import Path
import re
import socket
import sqlite3
import threading
import time
import urllib.error
import urllib.request

NODE = 'http://127.0.0.1:8080'
CATALOG = 'synthetic-terminology-demo'
MAX_REQUEST = 786432
MAX_RESPONSE = 1048576
MAX_HTTP_REQUESTS = 50000
MAX_HTTP_BYTES = 16 * 1024**3
MAX_INPUT = 131072
LIMITS = {'max_runtime_ms': 2000, 'max_memory_bytes': 8388608,
          'fuel_limit': 50000000, 'max_input_bytes': MAX_INPUT,
          'max_output_bytes': MAX_INPUT}
class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())


def encoded(value):
    return json.dumps(value, separators=(',', ':'), ensure_ascii=False,
                      allow_nan=False).encode()


def only(value, names):
    return isinstance(value, dict) and set(value).issubset(names)


def exact_int(value, low, high):
    return type(value) is int and low <= value <= high


def safe_json(value, depth=0):
    if depth > 64:
        return False
    if type(value) is float:
        return math.isfinite(value)
    if type(value) is int:
        return abs(value) <= 9007199254740991
    if isinstance(value, list):
        return all(safe_json(x, depth + 1) for x in value)
    if isinstance(value, dict):
        return all(safe_json(x, depth + 1) for x in value.values())
    return True


def hash_string(value):
    return isinstance(value, str) and re.fullmatch('[0-9a-f]{64}', value) is not None


def allowed_work(body):
    if not isinstance(body, dict) or not safe_json(body):
        return False
    if body.get('kind') == 'wasm':
        s = body.get('submission')
        if not only(s, {'schema_version', 'submission_type', 'workload', 'module_bytes_hex', 'input'}):
            return False
        w = s.get('workload')
        module = s.get('module_bytes_hex')
        return (s.get('schema_version') == 'froglet/v1' and s.get('submission_type') == 'wasm_submission'
                and only(w, {'schema_version', 'workload_kind', 'abi_version', 'module_format', 'module_hash', 'input_format', 'input_hash', 'requested_capabilities'})
                and w.get('schema_version') == 'froglet/v1' and w.get('workload_kind') == 'compute.wasm.v1'
                and w.get('abi_version') == 'froglet.wasm.run_json.v1' and w.get('module_format') == 'application/wasm'
                and w.get('input_format') == 'application/json+jcs' and hash_string(w.get('module_hash'))
                and hash_string(w.get('input_hash')) and w.get('requested_capabilities') == []
                and isinstance(module, str) and len(module) <= 524288
                and re.fullmatch('0061736d01000000(?:[0-9a-f]{2})*', module) is not None
                and len(encoded(s.get('input'))) <= MAX_INPUT)
    if body.get('kind') == 'execution':
        e = body.get('execution')
        if not only(e, {'schema_version', 'workload_kind', 'runtime', 'package_kind', 'entrypoint', 'contract_version', 'input_format', 'input_hash', 'security', 'input', 'module_hash', 'builtin_name'}):
            return False
        return (e.get('schema_version') == 'froglet/v1' and e.get('workload_kind') == CATALOG and e.get('builtin_name') == CATALOG
                and e.get('runtime') == e.get('package_kind') == 'builtin'
                and e.get('entrypoint') == {'kind': 'builtin', 'value': CATALOG}
                and e.get('contract_version') == 'froglet.builtin.data_query.json.v1'
                and e.get('input_format') == 'application/json+jcs' and hash_string(e.get('input_hash'))
                and hash_string(e.get('module_hash'))
                and e.get('security') == {'mode': 'standard', 'service_id': CATALOG}
                and len(encoded(e.get('input'))) <= MAX_INPUT)
    return False


def allowed_post(path, body, provider_id):
    if not allowed_work(body):
        return False
    if path == '/v1/provider/quotes':
        offer = 'execute.compute' if body['kind'] == 'wasm' else CATALOG
        return (only(body, {'offer_id', 'requester_id', 'kind', 'submission', 'execution', 'max_price_sats'})
                and body.get('offer_id') == offer and hash_string(body.get('requester_id'))
                and type(body.get('max_price_sats')) is int and body['max_price_sats'] == 0)
    if not only(body, {'quote', 'deal', 'kind', 'submission', 'execution', 'idempotency_key', 'payment'}):
        return False
    if body.get('payment') is not None or not isinstance(body.get('idempotency_key'), str) or not re.fullmatch('[a-zA-Z0-9_.:-]{1,128}', body['idempotency_key']):
        return False
    q, d = body.get('quote'), body.get('deal')
    if not isinstance(q, dict) or not isinstance(d, dict) or q.get('artifact_type') != 'quote' or d.get('artifact_type') != 'deal':
        return False
    p, dp = q.get('payload'), d.get('payload')
    if not isinstance(p, dict) or not isinstance(dp, dict):
        return False
    limits = p.get('execution_limits')
    terms = p.get('settlement_terms')
    bounds = dict(LIMITS)
    if body['kind'] == 'execution':
        bounds['max_input_bytes'] = bounds['max_output_bytes'] = 1048576
    return (p.get('provider_id') == dp.get('provider_id') == provider_id
            and p.get('workload_kind') == ('compute.wasm.v1' if body['kind'] == 'wasm' else CATALOG)
            and isinstance(terms, dict) and terms.get('method') == 'none'
            and type(terms.get('base_fee_msat')) is int and terms['base_fee_msat'] == 0
            and type(terms.get('success_fee_msat')) is int and terms['success_fee_msat'] == 0
            and p.get('capabilities_granted', []) == [] and isinstance(limits, dict)
            and all(exact_int(limits.get(k), 1 if k in {'max_runtime_ms', 'max_input_bytes', 'max_output_bytes'} else 0, n) for k, n in bounds.items()))


class TrafficLedger:
    def __init__(self, path, *, first_boot=False):
        self.path = Path(path)
        if self.path.is_symlink() or (not first_boot and not self.path.is_file()):
            raise RuntimeError('existing-traffic-ledger-required')
        with self.connect() as c:
            c.execute('PRAGMA journal_mode=WAL')
            c.execute('CREATE TABLE IF NOT EXISTS traffic (id INTEGER PRIMARY KEY CHECK(id=1), requests INTEGER NOT NULL CHECK(requests>=0), bytes INTEGER NOT NULL CHECK(bytes>=0))')
            if first_boot:
                c.execute('INSERT OR IGNORE INTO traffic VALUES(1,0,0)')
            if c.execute('PRAGMA integrity_check').fetchone()[0] != 'ok' or c.execute('SELECT COUNT(*) FROM traffic').fetchone()[0] != 1:
                raise RuntimeError('traffic-ledger-invalid')

    @contextmanager
    def connect(self):
        c = sqlite3.connect(self.path, timeout=2)
        try:
            c.execute('PRAGMA synchronous=FULL')
            with c:
                yield c
        finally:
            c.close()

    def reserve(self, request_bytes):
        with self.connect() as c:
            c.execute('BEGIN IMMEDIATE')
            requests, reserved = c.execute('SELECT requests,bytes FROM traffic WHERE id=1').fetchone()
            amount = request_bytes + MAX_RESPONSE
            if requests >= MAX_HTTP_REQUESTS or reserved + amount > MAX_HTTP_BYTES:
                return False
            c.execute('UPDATE traffic SET requests=requests+1,bytes=bytes+? WHERE id=1', (amount,))
            return True


def native(method, path, body=None, token=None):
    headers = {'accept': 'application/json'}
    if body is not None:
        headers['content-type'] = 'application/json'
    if token is not None:
        headers['authorization'] = 'Bearer ' + token
    req = urllib.request.Request(NODE + path, data=body, headers=headers, method=method)
    try:
        response = OPENER.open(req, timeout=10)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        raw = response.read(MAX_RESPONSE + 1)
        if len(raw) > MAX_RESPONSE:
            raise RuntimeError('response-too-large')
        json.loads(raw)
        return response.status, raw


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True
    request_queue_size = 16
    connection_slots = threading.BoundedSemaphore(16)

    def get_request(self):
        request, address = super().get_request()
        request.settimeout(10)
        return request, address

    def process_request(self, request, address):
        if not self.connection_slots.acquire(blocking=False):
            request.close()
            return
        try:
            super().process_request(request, address)
        except BaseException:
            self.connection_slots.release()
            raise

    def process_request_thread(self, request, address):
        try:
            super().process_request_thread(request, address)
        finally:
            self.connection_slots.release()


class Handler(http.server.BaseHTTPRequestHandler):
    server_version = 'FrogletPublicBeta'
    sys_version = ''
    protocol_version = 'HTTP/1.0'
    slots = threading.BoundedSemaphore(8)
    provider_id = ''
    ledger = None

    def log_message(self, *_):
        pass  # Never log caller input, job capabilities or private headers.

    def answer(self, status, body):
        raw = body if isinstance(body, bytes) else encoded(body)
        self.send_response(status)
        self.send_header('content-type', 'application/json')
        self.send_header('cache-control', 'no-store')
        self.send_header('x-content-type-options', 'nosniff')
        self.send_header('content-length', str(len(raw)))
        self.send_header('connection', 'close')
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        self.handle_public(False)

    def do_POST(self):
        self.handle_public(True)

    def handle_public(self, post):
        path = self.path
        known_get = path in {'/health', '/demo/status', '/v1/provider/descriptor', '/v1/provider/offers', '/v1/provider/services/' + CATALOG} or re.fullmatch('/v1/provider/deals/[a-zA-Z0-9_-]{1,128}', path)
        if (post and path not in {'/v1/provider/quotes', '/v1/provider/deals'}) or (not post and not known_get):
            self.answer(404, {'error': 'This operation is not exposed by the public demo.'})
            return
        if not self.slots.acquire(blocking=False):
            self.answer(429, {'error': 'The demo is busy. Retry the same job.'})
            return
        try:
            self.connection.settimeout(10)
            body = None
            size = 0
            if post:
                if self.headers.get('transfer-encoding') or self.headers.get('content-type', '').split(';')[0].strip().lower() != 'application/json':
                    self.answer(415, {'error': 'Send a bounded JSON request.'})
                    return
                sizes = self.headers.get_all('content-length') or []
                if len(sizes) != 1 or not re.fullmatch('[0-9]{1,8}', sizes[0]):
                    self.answer(411, {'error': 'A request length is required.'})
                    return
                size = int(sizes[0])
                if size > MAX_REQUEST:
                    self.answer(413, {'error': 'The request is too large.'})
                    return
                raw = self.rfile.read(size)
                if len(raw) != size:
                    self.answer(400, {'error': 'The request is incomplete.'})
                    return
                try:
                    value = json.loads(raw)
                    allowed = allowed_post(path, value, self.provider_id)
                except (ValueError, RecursionError, TypeError, OverflowError):
                    allowed = False
                if not allowed:
                    self.answer(400, {'error': 'Only free pure-Wasm jobs and the selected synthetic catalog are accepted.'})
                    return
                body = raw
            if path != '/health' and not self.ledger.reserve(size):
                self.answer(429, {'error': 'The shared beta traffic allowance is spent. Existing usage is preserved.'})
                return
            if path == '/demo/status':
                token_path = Path(os.environ['FROGLET_DATA_ROOT']) / 'runtime/froglet-control.token'
                if token_path.is_symlink() or not token_path.is_file():
                    raise RuntimeError('control-token-unavailable')
                token = token_path.read_text().strip()
                status, raw = native('GET', '/v1/provider/usage', token=token)
                if status != 200:
                    raise RuntimeError('status-unavailable')
                snapshot = json.loads(raw)
                self.answer(200, {'provider_id': self.provider_id, 'paused': snapshot['paused'],
                                  'usage': snapshot['usage'], 'remaining': snapshot['remaining'],
                                  'limits': {k: snapshot['policy'][k] for k in ('max_total_deals', 'max_total_quotes', 'max_total_runtime_ms')},
                                  'observed_at': int(time.time())})
            else:
                status, raw = native('POST' if post else 'GET', path, body)
                self.answer(status, raw)
        except (OSError, ValueError, RuntimeError, sqlite3.Error, KeyError):
            try:
                self.answer(503, {'error': 'The provider is unavailable. Preserve the same job and retry.'})
            except OSError:
                pass
        finally:
            self.slots.release()


def main():
    os.umask(0o077)
    root = Path(os.environ['FROGLET_DATA_ROOT']).parent
    expected = root / 'expected-provider-id'
    status, raw = native('GET', '/v1/provider/descriptor')
    if status != 200:
        raise RuntimeError('provider-unavailable')
    provider = json.loads(raw)['payload']['provider_id']
    if not hash_string(provider) or expected.is_symlink():
        raise RuntimeError('provider-identity-invalid')
    first_boot = not expected.exists()
    if not first_boot and expected.read_text().strip() != provider:
        raise RuntimeError('provider-identity-changed')
    Handler.ledger = TrafficLedger(root / 'traffic.db', first_boot=first_boot)
    if first_boot:
        with expected.open('x') as f:
            f.write(provider + '\n')
            f.flush()
            os.fsync(f.fileno())
    Handler.provider_id = provider
    Server(('0.0.0.0', 8088), Handler).serve_forever()


if __name__ == '__main__':
    main()
