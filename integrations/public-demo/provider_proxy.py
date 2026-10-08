"""Narrow public ingress for an unchanged native Froglet node on loopback.

Discovery exposes only free pure-Wasm, the selected synthetic catalog and
exact profile-pinned offers, retaining their signed documents unchanged. An
optional protected profile permits three named Wasm services, each with one
exact HTTP-operation capability. Operator/runtime endpoints, credentials,
Python/container execution and arbitrary host access never pass this boundary.
Native Froglet verifies signed records and execution allowances; this service
caps HTTP traffic.
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
import stat
import threading
import time
import urllib.error
import urllib.request

NODE = 'http://127.0.0.1:8080'
CATALOG = 'synthetic-terminology-demo'
APPROVED_SERVICE_IDS = frozenset({'marketplace-search', 'marketplace-provider', 'marketplace-receipts'})
APPROVED_PROFILE_SCHEMA = 'froglet.public-demo-approved-services.v1'
MAX_REQUEST = 786432
MAX_RESPONSE = 1048576
MAX_METADATA_RESPONSE = 32768
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


def load_approved_services(path, expected_sha256, provider_id, *, root):
    """Load one operator-pinned file directly inside the protected volume."""
    path, root = Path(path), Path(root)
    if (not path.is_absolute() or path.parent != root
            or not hash_string(expected_sha256) or not hash_string(provider_id)):
        raise RuntimeError('approved-profile-location-or-digest-invalid')
    flags = os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK
    try:
        directory = os.open(root, flags | os.O_DIRECTORY)
        try:
            directory_stat = os.fstat(directory)
            if (directory_stat.st_uid not in {0, os.geteuid()}
                    or directory_stat.st_mode & 0o022):
                raise RuntimeError('approved-profile-volume-not-protected')
            descriptor = os.open(path.name, flags, dir_fd=directory)
        finally:
            os.close(directory)
        with os.fdopen(descriptor, 'rb') as file:
            info = os.fstat(file.fileno())
            if (not stat.S_ISREG(info.st_mode) or info.st_uid not in {0, os.geteuid()}
                    or info.st_mode & 0o022 or not 0 < info.st_size <= MAX_METADATA_RESPONSE):
                raise RuntimeError('approved-profile-file-not-protected')
            raw = file.read(MAX_METADATA_RESPONSE + 1)
    except OSError as error:
        raise RuntimeError('approved-profile-file-unavailable') from error
    if hashlib.sha256(raw).hexdigest() != expected_sha256:
        raise RuntimeError('approved-profile-digest-mismatch')

    def distinct_fields(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError('duplicate profile field')
            result[key] = value
        return result

    try:
        value = json.loads(raw, object_pairs_hook=distinct_fields)
    except (ValueError, RecursionError) as error:
        raise RuntimeError('approved-profile-json-invalid') from error
    if (not isinstance(value, dict) or set(value) != {'schema_version', 'provider_id', 'services'}
            or value['schema_version'] != APPROVED_PROFILE_SCHEMA
            or value['provider_id'] != provider_id or not isinstance(value['services'], list)):
        raise RuntimeError('approved-profile-schema-invalid')
    fields = {'service_id', 'offer_id', 'offer_hash', 'descriptor_hash', 'revision_hash',
              'module_hash', 'binding_hash', 'operation_hash', 'entrypoint'}
    services = {}
    for service in value['services']:
        if (not isinstance(service, dict) or set(service) != fields
                or not isinstance(service.get('service_id'), str)
                or service.get('service_id') not in APPROVED_SERVICE_IDS
                or service['offer_id'] != service['service_id']
                or service['service_id'] in services
                or not all(hash_string(service[k]) for k in fields - {'service_id', 'offer_id', 'entrypoint'})
                or not isinstance(service['entrypoint'], str)
                or not 1 <= len(service['entrypoint']) <= 128
                or any(ord(c) < 32 or ord(c) == 127 for c in service['entrypoint'])):
            raise RuntimeError('approved-profile-service-invalid')
        services[service['service_id']] = service
    if services and (set(services) != APPROVED_SERVICE_IDS
                     or len({s['offer_hash'] for s in services.values()}) != 3
                     or len({s['revision_hash'] for s in services.values()}) != 3):
        raise RuntimeError('approved-profile-exact-three-services-required')
    return services


def approved_work_profile(body, approved_services):
    if not isinstance(body, dict) or body.get('kind') != 'execution':
        return None
    execution = body.get('execution')
    security = execution.get('security') if isinstance(execution, dict) else None
    service_id = security.get('service_id') if isinstance(security, dict) else None
    return approved_services.get(service_id) if isinstance(service_id, str) else None


def allowed_work(body, approved_services=None):
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
        profile = approved_work_profile(body, approved_services or {})
        if profile is not None:
            return (isinstance(e, dict) and set(e) == {
                        'schema_version', 'workload_kind', 'runtime', 'package_kind', 'entrypoint',
                        'contract_version', 'input_format', 'input_hash', 'security', 'input',
                        'module_hash', 'requested_access'}
                    and e.get('schema_version') == 'froglet/v1'
                    and e.get('workload_kind') == 'compute.execution.v1'
                    and e.get('runtime') == 'wasm' and e.get('package_kind') == 'inline_module'
                    and e.get('entrypoint') == {'kind': 'module', 'value': profile['entrypoint']}
                    and e.get('contract_version') == 'froglet.wasm.host_json.v1'
                    and e.get('input_format') == 'application/json+jcs'
                    and hash_string(e.get('input_hash'))
                    and e.get('module_hash') == profile['binding_hash']
                    and e.get('security') == {'mode': 'standard', 'service_id': profile['service_id']}
                    and e.get('requested_access') == ['net.http.operation.' + profile['operation_hash']]
                    and len(encoded(e.get('input'))) <= MAX_INPUT)
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


def allowed_post(path, body, provider_id, approved_services=None):
    approved_services = approved_services or {}
    if not allowed_work(body, approved_services):
        return False
    profile = approved_work_profile(body, approved_services)
    if path == '/v1/provider/quotes':
        offer = profile['offer_id'] if profile is not None else ('execute.compute' if body['kind'] == 'wasm' else CATALOG)
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
    if body['kind'] == 'execution' and profile is None:
        bounds['max_input_bytes'] = bounds['max_output_bytes'] = 1048576
    return (p.get('provider_id') == dp.get('provider_id') == provider_id
            and p.get('workload_kind') == ('compute.execution.v1' if profile is not None else ('compute.wasm.v1' if body['kind'] == 'wasm' else CATALOG))
            and (profile is None or (q.get('signer') == provider_id and p.get('offer_hash') == profile['offer_hash']))
            and isinstance(terms, dict) and terms.get('method') == 'none'
            and type(terms.get('base_fee_msat')) is int and terms['base_fee_msat'] == 0
            and type(terms.get('success_fee_msat')) is int and terms['success_fee_msat'] == 0
            and p.get('capabilities_granted', []) == ([] if profile is None else ['net.http.operation.' + profile['operation_hash']]) and isinstance(limits, dict)
            and all(exact_int(limits.get(k), 1 if k in {'max_runtime_ms', 'max_input_bytes', 'max_output_bytes'} else 0, n) for k, n in bounds.items()))


def approved_quote_valid(quote, provider_id, profile):
    if not isinstance(quote, dict) or not safe_json(quote):
        return False
    payload = quote.get('payload')
    if not isinstance(payload, dict):
        return False
    terms, limits = payload.get('settlement_terms'), payload.get('execution_limits')
    return (quote.get('artifact_type') == 'quote' and quote.get('signer') == provider_id
            and payload.get('provider_id') == provider_id
            and payload.get('offer_hash') == profile['offer_hash']
            and payload.get('workload_kind') == 'compute.execution.v1'
            and payload.get('capabilities_granted') == ['net.http.operation.' + profile['operation_hash']]
            and isinstance(terms, dict) and terms.get('method') == 'none'
            and type(terms.get('base_fee_msat')) is int and terms['base_fee_msat'] == 0
            and type(terms.get('success_fee_msat')) is int and terms['success_fee_msat'] == 0
            and isinstance(limits, dict)
            and all(exact_int(limits.get(k), 1 if k in {'max_runtime_ms', 'max_input_bytes', 'max_output_bytes'} else 0, n) for k, n in LIMITS.items()))


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

    def reserve(self, request_bytes, response_limit=MAX_RESPONSE):
        with self.connect() as c:
            c.execute('BEGIN IMMEDIATE')
            requests, reserved = c.execute('SELECT requests,bytes FROM traffic WHERE id=1').fetchone()
            amount = request_bytes + response_limit
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


def selected_offer_documents(metadata, provider_id, approved_services, *, include_compute=False):
    """Filter the unsigned envelope without modifying any signed Offer."""
    if not isinstance(metadata, dict) or not isinstance(metadata.get('offers'), list):
        raise RuntimeError('public-offers-invalid')
    selected = {CATALOG: None, **approved_services}
    if include_compute:
        selected['execute.compute'] = None
    offers = {}
    for offer in metadata['offers']:
        payload = offer.get('payload') if isinstance(offer, dict) else None
        if (not isinstance(payload, dict) or offer.get('artifact_type') != 'offer'
                or payload.get('provider_id') != provider_id
                or not isinstance(payload.get('offer_id'), str)
                or payload.get('offer_id') not in selected):
            continue
        offer_id = payload['offer_id']
        if offer_id in offers:
            raise RuntimeError('ambiguous-public-offer')
        profile = selected[offer_id]
        if profile is not None and (offer.get('hash') != profile['offer_hash']
                                    or payload.get('descriptor_hash') != profile['descriptor_hash']):
            raise RuntimeError('approved-public-offer-mismatch')
        execution, price = payload.get('execution_profile'), payload.get('price_schedule')
        bounds = dict(LIMITS)
        if offer_id == CATALOG:
            kind, runtime, package, contract, capabilities = CATALOG, 'builtin', 'builtin', 'froglet.builtin.data_query.json.v1', []
            bounds.update(max_input_bytes=1048576, max_output_bytes=1048576, max_memory_bytes=0, fuel_limit=0)
        elif profile is not None:
            kind, runtime, package, contract = 'compute.execution.v1', 'wasm', 'inline_module', 'froglet.wasm.host_json.v1'
            capabilities = ['net.http.operation.' + profile['operation_hash']]
        else:
            kind, runtime, package, contract, capabilities = 'compute.wasm.v1', 'wasm', 'inline_module', 'froglet.wasm.run_json.v1', []
        if (not safe_json(offer) or not hash_string(offer.get('hash'))
                or not hash_string(payload.get('descriptor_hash')) or offer.get('signer') != provider_id
                or payload.get('offer_kind') != kind or payload.get('settlement_method') != 'none'
                or not isinstance(price, dict)
                or not all(type(price.get(k)) is int and price[k] == 0 for k in ('base_fee_msat', 'success_fee_msat'))
                or not isinstance(execution, dict) or execution.get('runtime') != runtime
                or execution.get('package_kind') != package
                or execution.get('contract_version') != contract or execution.get('abi_version') != contract
                or execution.get('capabilities', []) != capabilities
                or execution.get('access_handles', []) != capabilities
                or not all(exact_int(execution.get(k), 1 if k in {'max_runtime_ms', 'max_input_bytes', 'max_output_bytes'} else 0, n) for k, n in bounds.items())):
            raise RuntimeError('selected-public-offer-profile-mismatch')
        offers[offer_id] = offer
    if not set(approved_services).issubset(offers):
        raise RuntimeError('approved-public-offer-unavailable')
    return list(offers.values())


def publication_records():
    """Selected catalog and pinned offers; never fetch the visitor ledger feed."""
    status, raw = native('GET', '/v1/provider/offers')
    if status != 200:
        raise RuntimeError('public-offers-unavailable')
    offers = selected_offer_documents(json.loads(raw), Handler.provider_id, Handler.approved_services)
    records = []
    seen = set()
    for offer in offers:
        hashes = [offer['payload']['descriptor_hash'], offer['hash']]
        for kind, artifact_hash in zip(('descriptor', 'offer'), hashes):
            if not hash_string(artifact_hash):
                raise RuntimeError('invalid-public-artifact-hash')
            if artifact_hash in seen:
                if kind != 'descriptor':
                    raise RuntimeError('public-artifact-hash-collision')
                continue
            status, raw = native('GET', '/v1/artifacts/' + artifact_hash)
            if status != 200:
                raise RuntimeError('public-artifact-unavailable')
            record = json.loads(raw)
            document = record.get('document') if isinstance(record, dict) else None
            payload = document.get('payload') if isinstance(document, dict) else None
            if (not isinstance(payload, dict) or record.get('hash') != artifact_hash
                    or record.get('kind') != kind or record.get('actor_id') != Handler.provider_id
                    or not exact_int(record.get('cursor'), 1, 9223372036854775807)
                    or document.get('hash') != artifact_hash
                    or document.get('artifact_type') != kind
                    or payload.get('provider_id') != Handler.provider_id
                    or (kind == 'offer' and document != offer)):
                raise RuntimeError('public-artifact-binding-mismatch')
            records.append(record)
            seen.add(artifact_hash)
    return sorted(records, key=lambda r: r['cursor']), [offer['hash'] for offer in offers]


def feed_query(path):
    """Strict finite query grammar; no unknown, repeated or encoded selectors."""
    if path == '/v1/feed':
        return 0, 50
    if not path.startswith('/v1/feed?'):
        return None
    fields = path.split('?', 1)[1].split('&')
    values = {}
    for field in fields:
        match = re.fullmatch('(cursor|limit)=([0-9]{1,19})', field)
        if not match or match[1] in values:
            return None
        values[match[1]] = int(match[2])
    cursor, limit = values.get('cursor', 0), values.get('limit', 50)
    return (cursor, limit) if cursor <= 9223372036854775807 and 1 <= limit <= 100 else None


def canary_request_valid(path, value):
    if (not only(value, {'schema_version', 'revision_hash', 'offer_hash', 'challenge', 'input'})
            or value.get('schema_version') != 'froglet.publication-canary-request.v1'
            or not all(hash_string(value.get(k)) for k in ('revision_hash', 'offer_hash', 'challenge'))
            or path != '/v1/publications/' + value['revision_hash'] + '/canary'
            or not safe_json(value.get('input'))):
        return False
    return True


def approved_service_metadata_valid(metadata, profile, provider_id):
    if not isinstance(metadata, dict) or not safe_json(metadata):
        return False
    service, revision = metadata.get('service'), metadata.get('publication_revision')
    if not isinstance(service, dict) or not isinstance(revision, dict):
        return False
    payload = revision.get('payload')
    if not isinstance(payload, dict):
        return False
    interface, limits, price = payload.get('service'), payload.get('limits'), payload.get('price')
    capability = ['net.http.operation.' + profile['operation_hash']]
    return (service.get('service_id') == payload.get('service_id') == profile['service_id']
            and service.get('offer_id') == payload.get('offer_id') == profile['offer_id']
            and service.get('provider_id') == payload.get('provider_id') == revision.get('signer_pubkey') == provider_id
            and service.get('offer_kind') == 'compute.execution.v1'
            and service.get('publication_state') == 'active'
            and service.get('runtime') == payload.get('runtime') == 'wasm'
            and service.get('package_kind') == payload.get('package_kind') == 'inline_module'
            and service.get('module_hash') == service.get('binding_hash') == payload.get('binding_hash') == payload.get('package_digest') == profile['binding_hash']
            and revision.get('revision_hash') == profile['revision_hash']
            and payload.get('offer_hash') == profile['offer_hash']
            and isinstance(interface, dict)
            and service.get('entrypoint_kind') == interface.get('entrypoint_kind') == 'module'
            and service.get('entrypoint') == interface.get('entrypoint') == profile['entrypoint']
            and service.get('contract_version') == interface.get('contract_version') == 'froglet.wasm.host_json.v1'
            and service.get('capabilities', []) == interface.get('capabilities', []) == capability
            and service.get('mounts', []) == interface.get('mounts', []) == []
            and service.get('starter') == interface.get('starter') and isinstance(service.get('starter'), str)
            and not {'module_bytes_hex', 'inline_source', 'python_bundle', 'source_path'}.intersection(service)
            and service.get('settlement_method') == 'none'
            and all(type(service.get(k)) is int and service[k] == 0 for k in ('price_sats', 'base_fee_msat', 'success_fee_msat'))
            and isinstance(price, dict) and price.get('settlement_method') == price.get('offer_settlement_method') == 'none'
            and all(type(price.get(k)) is int and price[k] == 0 for k in ('base_amount_minor', 'success_amount_minor'))
            and isinstance(limits, dict)
            and all(exact_int(limits.get(k), 1 if k in {'max_runtime_ms', 'max_input_bytes', 'max_output_bytes'} else 0, n) for k, n in LIMITS.items()))


def allowed_canary(path, value):
    if not canary_request_valid(path, value):
        return False
    profile = next((p for p in Handler.approved_services.values()
                    if p['offer_hash'] == value['offer_hash'] or p['revision_hash'] == value['revision_hash']), None)
    if profile is not None and (profile['offer_hash'] != value['offer_hash'] or profile['revision_hash'] != value['revision_hash']):
        return False
    service_id = CATALOG if profile is None else profile['service_id']
    status, raw = native('GET', '/v1/provider/services/' + service_id)
    if status != 200:
        return False
    metadata = json.loads(raw)
    if profile is not None and not approved_service_metadata_valid(metadata, profile, Handler.provider_id):
        return False
    revision = metadata['publication_revision']
    # Only the selected service's already-public starter can enter the canary
    # endpoint; arbitrary programs, private services and other inputs cannot.
    starter = json.loads(metadata['service']['starter'])
    return (safe_json(starter) and len(encoded(starter)) <= MAX_INPUT
            and value['revision_hash'] == revision['revision_hash']
            and value['offer_hash'] == revision['payload']['offer_hash']
            and json.dumps(value['input'], sort_keys=True, allow_nan=False)
            == json.dumps(starter, sort_keys=True, allow_nan=False))


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
    approved_services = {}

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
        feed = feed_query(path)
        artifact = re.fullmatch('/v1/artifacts/([0-9a-f]{64})', path)
        canary = re.fullmatch('/v1/publications/([0-9a-f]{64})/canary', path)
        service_profile = next((p for p in self.approved_services.values()
                                if path == '/v1/provider/services/' + p['service_id']), None)
        metadata = feed is not None or artifact or path == '/v1/node/capabilities' or service_profile is not None
        known_get = metadata or path in {'/health', '/demo/status', '/v1/provider/descriptor', '/v1/provider/offers', '/v1/provider/services/' + CATALOG} or re.fullmatch('/v1/provider/deals/[a-zA-Z0-9_-]{1,128}', path)
        if (post and path not in {'/v1/provider/quotes', '/v1/provider/deals'} and not canary) or (not post and not known_get):
            self.answer(404, {'error': 'This operation is not exposed by the public demo.'})
            return
        if not self.slots.acquire(blocking=False):
            self.answer(429, {'error': 'The demo is busy. Retry the same job.'})
            return
        try:
            self.connection.settimeout(10)
            body = None
            size = 0
            response_limit = MAX_METADATA_RESPONSE if metadata else MAX_RESPONSE
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
                    allowed = canary_request_valid(path, value) if canary else allowed_post(path, value, self.provider_id, self.approved_services)
                except (ValueError, RecursionError, TypeError, OverflowError):
                    allowed = False
                if not allowed:
                    self.answer(400, {'error': 'Only free Wasm jobs and selected public services are accepted.'})
                    return
                body = raw
            if path != '/health' and not self.ledger.reserve(size, response_limit):
                self.answer(429, {'error': 'The shared beta traffic allowance is spent. Existing usage is preserved.'})
                return
            if canary and not allowed_canary(path, value):
                self.answer(400, {'error': 'Only a current selected service verification fixture is accepted.'})
                return
            if feed is not None or artifact:
                records, hashes = publication_records()
                if artifact:
                    match = next((r for r in records if r['hash'] == artifact[1]), None)
                    if match is None:
                        self.answer(404, {'error': 'Only current selected publication artifacts are exposed.'})
                        return
                    raw = encoded(match)
                else:
                    cursor, limit = feed
                    remaining = [r for r in records if r['cursor'] > cursor]
                    page = remaining[:limit]
                    raw = encoded({'artifacts': page, 'active_offer_hashes': hashes,
                                   'cursor_type': 'artifact_sequence', 'cursor_semantics': 'exclusive_after',
                                   'applied_cursor': cursor, 'page_size': limit,
                                   'has_more': len(remaining) > limit,
                                   'next_cursor': page[-1]['cursor'] if page else None})
                if len(raw) > response_limit:
                    raise RuntimeError('public-metadata-too-large')
                self.answer(200, raw)
            elif path == '/demo/status':
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
                if not post and path == '/v1/provider/offers' and status == 200:
                    raw = encoded({'offers': selected_offer_documents(json.loads(raw), self.provider_id, self.approved_services, include_compute=True)})
                if service_profile is not None and 200 <= status < 300 and not approved_service_metadata_valid(json.loads(raw), service_profile, self.provider_id):
                    raise RuntimeError('approved-native-service-mismatch')
                if post and path == '/v1/provider/quotes' and 200 <= status < 300:
                    profile = approved_work_profile(value, self.approved_services)
                    if profile is not None and not approved_quote_valid(json.loads(raw), self.provider_id, profile):
                        raise RuntimeError('approved-native-quote-mismatch')
                if len(raw) > response_limit:
                    raise RuntimeError('public-metadata-too-large')
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
    profile_path = os.environ.get('FROGLET_PUBLIC_DEMO_PROFILE_PATH')
    profile_sha256 = os.environ.get('FROGLET_PUBLIC_DEMO_PROFILE_SHA256')
    if profile_path is None and profile_sha256 is None:
        Handler.approved_services = {}
    elif not profile_path or not profile_sha256:
        raise RuntimeError('approved-profile-configuration-incomplete')
    else:
        Handler.approved_services = load_approved_services(profile_path, profile_sha256, provider, root=root)
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
