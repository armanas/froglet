"""Public ingress boundaries; no real cloud or runtime is mocked as qualified."""
import copy
from contextlib import closing
import hashlib
import http.client
import importlib.util
import json
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch

MODULE = Path(__file__).resolve().parents[2] / 'integrations/public-demo/provider_proxy.py'
spec = importlib.util.spec_from_file_location('public_demo_proxy', MODULE)
demo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(demo)
PROVIDER = 'a' * 64


def approved_profile():
    services = []
    for index, service_id in enumerate(('marketplace-search', 'marketplace-provider', 'marketplace-receipts'), 1):
        services.append({'service_id': service_id, 'offer_id': service_id,
                         'offer_hash': str(index) * 64, 'descriptor_hash': '6' * 64,
                         'revision_hash': str(index + 3) * 64,
                         'module_hash': '7' * 64, 'binding_hash': '8' * 64,
                         'entrypoint': 'operation.wasm', 'operation_hash': str(index + 6) * 64})
    return {'schema_version': 'froglet.public-demo-approved-services.v1',
            'provider_id': PROVIDER, 'services': services}


class PublicDemoApprovedProfileTests(unittest.TestCase):
    def test_unknown_partial_duplicate_or_private_profile_fields_are_refused(self):
        invalid = []
        for edit in ('partial', 'duplicate', 'unknown', 'missing', 'private', 'provider', 'binding', 'entrypoint', 'list-id', 'dict-id'):
            value = approved_profile()
            if edit == 'partial': value['services'].pop()
            if edit == 'duplicate': value['services'][1] = copy.deepcopy(value['services'][0])
            if edit == 'unknown': value['services'][0]['service_id'] = 'events.query'
            if edit == 'missing': del value['services'][0]['binding_hash']
            if edit == 'private': value['services'][0]['owner_token'] = 'test-only'
            if edit == 'provider': value['provider_id'] = '0' * 64
            if edit == 'binding': value['services'][0]['binding_hash'] = 'invalid'
            if edit == 'entrypoint': value['services'][0]['entrypoint'] = 'run\nprivate'
            if edit == 'list-id': value['services'][0]['service_id'] = []
            if edit == 'dict-id': value['services'][0]['service_id'] = {}
            invalid.append((edit, demo.encoded(value)))
        invalid.append(('duplicate-json-field', b'{"provider_id":"' + PROVIDER.encode() + b'",' + demo.encoded(approved_profile())[1:]))
        invalid.append(('oversized', b' ' * (demo.MAX_METADATA_RESPONSE + 1)))
        with tempfile.TemporaryDirectory(prefix='froglet-profile-invalid-test-') as root:
            path = Path(root) / 'approved-services.json'
            for label, raw in invalid:
                path.write_bytes(raw)
                path.chmod(0o600)
                with self.subTest(label=label), self.assertRaisesRegex(RuntimeError, 'approved-profile'):
                    demo.load_approved_services(path, hashlib.sha256(raw).hexdigest(), PROVIDER, root=Path(root))

    def test_malformed_native_offer_identifiers_have_a_controlled_refusal(self):
        services = {s['service_id']: s for s in approved_profile()['services']}
        for offer_id in ([], {}):
            value = {'offers': [{'artifact_type': 'offer', 'payload': {
                'provider_id': PROVIDER, 'offer_id': offer_id}}]}
            with self.subTest(offer_id=offer_id), patch.object(demo.Handler, 'approved_services', services), \
                    patch.object(demo.Handler, 'provider_id', PROVIDER), \
                    patch.object(demo, 'native', return_value=(200, demo.encoded(value))), \
                    self.assertRaisesRegex(RuntimeError, 'public-offer'):
                demo.publication_records()

    def test_symlinks_nonregular_files_and_writable_volume_or_file_are_refused(self):
        with tempfile.TemporaryDirectory(prefix='froglet-profile-path-test-') as root:
            root = Path(root)
            path = root / 'approved-services.json'
            raw = demo.encoded(approved_profile())
            digest = hashlib.sha256(raw).hexdigest()
            path.write_bytes(raw)
            path.chmod(0o600)
            link = root / 'profile-link.json'
            link.symlink_to(path)
            fifo = root / 'profile-fifo'
            demo.os.mkfifo(fifo)
            for unsafe in (link, fifo, root):
                with self.subTest(path=unsafe), self.assertRaisesRegex(RuntimeError, 'approved-profile'):
                    demo.load_approved_services(unsafe, digest, PROVIDER, root=root)
            path.chmod(0o622)
            with self.assertRaisesRegex(RuntimeError, 'approved-profile'):
                demo.load_approved_services(path, digest, PROVIDER, root=root)
            path.chmod(0o600)
            root.chmod(0o722)
            try:
                with self.assertRaisesRegex(RuntimeError, 'approved-profile'):
                    demo.load_approved_services(path, digest, PROVIDER, root=root)
            finally:
                root.chmod(0o700)

    def test_only_the_exact_protected_profile_is_loaded_and_empty_is_disabled(self):
        with tempfile.TemporaryDirectory(prefix='froglet-approved-profile-test-') as root:
            path = Path(root) / 'approved-services.json'
            raw = demo.encoded(approved_profile())
            path.write_bytes(raw)
            path.chmod(0o600)
            digest = hashlib.sha256(raw).hexdigest()
            services = demo.load_approved_services(path, digest, PROVIDER, root=Path(root))
            self.assertEqual(set(services), {'marketplace-search', 'marketplace-provider', 'marketplace-receipts'})
            with self.assertRaisesRegex(RuntimeError, 'approved-profile'):
                demo.load_approved_services(path, '0' * 64, PROVIDER, root=Path(root))
            empty = dict(approved_profile(), services=[])
            raw = demo.encoded(empty)
            path.write_bytes(raw)
            self.assertEqual(demo.load_approved_services(path, hashlib.sha256(raw).hexdigest(), PROVIDER, root=Path(root)), {})

    def test_startup_loads_once_and_refuses_incomplete_profile_configuration(self):
        with tempfile.TemporaryDirectory(prefix='froglet-profile-startup-test-') as root:
            path = Path(root) / 'approved-services.json'
            raw = demo.encoded(approved_profile())
            path.write_bytes(raw)
            path.chmod(0o600)
            environment = {'FROGLET_DATA_ROOT': str(Path(root) / 'node'),
                           'FROGLET_PUBLIC_DEMO_PROFILE_PATH': str(path),
                           'FROGLET_PUBLIC_DEMO_PROFILE_SHA256': hashlib.sha256(raw).hexdigest()}
            with patch.dict(demo.os.environ, environment, clear=True), \
                    patch.object(demo, 'native', return_value=(200, demo.encoded({'payload': {'provider_id': PROVIDER}}))), \
                    patch.object(demo, 'Server') as server, \
                    patch.object(demo.Handler, 'approved_services', {}), \
                    patch.object(demo.Handler, 'ledger', None), \
                    patch.object(demo.Handler, 'provider_id', ''):
                demo.main()
                self.assertEqual(set(demo.Handler.approved_services),
                                 {'marketplace-search', 'marketplace-provider', 'marketplace-receipts'})
                server.return_value.serve_forever.assert_called_once()
                del demo.os.environ['FROGLET_PUBLIC_DEMO_PROFILE_SHA256']
                with self.assertRaisesRegex(RuntimeError, 'approved-profile'):
                    demo.main()
                del demo.os.environ['FROGLET_PUBLIC_DEMO_PROFILE_PATH']
                demo.main()
                self.assertEqual(demo.Handler.approved_services, {})


def quote_request():
    return {
        'offer_id': 'execute.compute', 'requester_id': 'b' * 64,
        'kind': 'wasm', 'max_price_sats': 0,
        'submission': {
            'schema_version': 'froglet/v1', 'submission_type': 'wasm_submission',
            'module_bytes_hex': '0061736d01000000', 'input': {'a': 6, 'b': 7},
            'workload': {'schema_version': 'froglet/v1', 'workload_kind': 'compute.wasm.v1',
                         'abi_version': 'froglet.wasm.run_json.v1', 'module_format': 'application/wasm',
                         'module_hash': 'c' * 64, 'input_format': 'application/json+jcs',
                         'input_hash': 'd' * 64, 'requested_capabilities': []},
        },
    }


def deal_request():
    body = quote_request()
    for key in ('offer_id', 'requester_id', 'max_price_sats'):
        del body[key]
    body.update({
        'idempotency_key': 'alice-first-job', 'payment': None,
        'quote': {'artifact_type': 'quote', 'payload': {
            'provider_id': PROVIDER, 'workload_kind': 'compute.wasm.v1',
            'settlement_terms': {'method': 'none', 'base_fee_msat': 0, 'success_fee_msat': 0},
            'execution_limits': dict(demo.LIMITS),
        }},
        'deal': {'artifact_type': 'deal', 'payload': {'provider_id': PROVIDER}},
    })
    return body


def named_quote_request(service):
    return {'offer_id': service['offer_id'], 'requester_id': 'b' * 64,
            'kind': 'execution', 'max_price_sats': 0,
            'execution': {'schema_version': 'froglet/v1', 'workload_kind': 'compute.execution.v1',
                          'runtime': 'wasm', 'package_kind': 'inline_module',
                          'entrypoint': {'kind': 'module', 'value': service['entrypoint']},
                          'contract_version': 'froglet.wasm.host_json.v1',
                          'input_format': 'application/json+jcs', 'input_hash': 'a' * 64,
                          'module_hash': service['binding_hash'],
                          'security': {'mode': 'standard', 'service_id': service['service_id']},
                          'requested_access': ['net.http.operation.' + service['operation_hash']],
                          'input': {'query': 'public fixture'}}}


def named_quote_artifact(service):
    return {'artifact_type': 'quote', 'signer': PROVIDER, 'payload': {
        'provider_id': PROVIDER, 'offer_hash': service['offer_hash'],
        'workload_kind': 'compute.execution.v1',
        'capabilities_granted': ['net.http.operation.' + service['operation_hash']],
        'settlement_terms': {'method': 'none', 'base_fee_msat': 0, 'success_fee_msat': 0},
        'execution_limits': dict(demo.LIMITS)}}


def named_service_metadata(profile):
    starter = {'provider_id': '0' * 64}
    service = {'service_id': profile['service_id'], 'offer_id': profile['offer_id'],
               'provider_id': PROVIDER, 'offer_kind': 'compute.execution.v1',
               'runtime': 'wasm', 'package_kind': 'inline_module',
               'entrypoint_kind': 'module', 'entrypoint': profile['entrypoint'],
               'contract_version': 'froglet.wasm.host_json.v1', 'publication_state': 'active',
               'binding_hash': profile['binding_hash'], 'module_hash': profile['binding_hash'],
               'capabilities': ['net.http.operation.' + profile['operation_hash']],
               'mounts': [], 'price_sats': 0, 'base_fee_msat': 0, 'success_fee_msat': 0,
               'settlement_method': 'none', 'starter': json.dumps(starter)}
    interface = {k: service[k] for k in ('entrypoint_kind', 'entrypoint', 'contract_version', 'capabilities', 'mounts', 'starter')}
    revision = {'revision_hash': profile['revision_hash'], 'signer_pubkey': PROVIDER,
                'payload': {'provider_id': PROVIDER, 'service_id': profile['service_id'],
                            'offer_id': profile['offer_id'], 'offer_hash': profile['offer_hash'],
                            'binding_hash': profile['binding_hash'], 'package_digest': profile['binding_hash'],
                            'runtime': 'wasm', 'package_kind': 'inline_module', 'service': interface,
                            'limits': dict(demo.LIMITS), 'price': {'settlement_method': 'none',
                            'offer_settlement_method': 'none', 'base_amount_minor': 0, 'success_amount_minor': 0}}}
    return {'service': service, 'publication_revision': revision}, starter


def public_offer_document(offer_id, artifact_hash, descriptor_hash, profile=None):
    limits = dict(demo.LIMITS)
    if offer_id == demo.CATALOG:
        limits.update(max_input_bytes=1048576, max_output_bytes=1048576, max_memory_bytes=0, fuel_limit=0)
        kind, runtime, package, contract, capabilities = demo.CATALOG, 'builtin', 'builtin', 'froglet.builtin.data_query.json.v1', []
    elif profile is not None:
        kind, runtime, package, contract = 'compute.execution.v1', 'wasm', 'inline_module', 'froglet.wasm.host_json.v1'
        capabilities = ['net.http.operation.' + profile['operation_hash']]
    else:
        kind, runtime, package, contract, capabilities = 'compute.wasm.v1', 'wasm', 'inline_module', 'froglet.wasm.run_json.v1', []
    return {'artifact_type': 'offer', 'hash': artifact_hash, 'signer': PROVIDER,
            'signature': 'unchanged-test-offer-signature', 'payload': {
                'provider_id': PROVIDER, 'offer_id': offer_id, 'descriptor_hash': descriptor_hash,
                'offer_kind': kind, 'settlement_method': 'none',
                'price_schedule': {'base_fee_msat': 0, 'success_fee_msat': 0},
                'execution_profile': dict(limits, runtime=runtime, package_kind=package,
                                          abi_version=contract, contract_version=contract,
                                          capabilities=capabilities, access_handles=capabilities)}}


class PublicDemoPolicyTests(unittest.TestCase):
    def test_named_deal_requires_exact_offer_capability_and_original_resource_ceiling(self):
        services = {s['service_id']: s for s in approved_profile()['services']}
        profile = services['marketplace-receipts']
        body = named_quote_request(profile)
        for key in ('offer_id', 'requester_id', 'max_price_sats'):
            del body[key]
        body.update({'quote': named_quote_artifact(profile),
                     'deal': {'artifact_type': 'deal', 'payload': {'provider_id': PROVIDER}},
                     'idempotency_key': 'named-first-job', 'payment': None})
        self.assertTrue(demo.allowed_post('/v1/provider/deals', body, PROVIDER, services))
        for edit in ('hash', 'operation', 'signer', 'fee', 'runtime', 'memory', 'output', 'duplicate-capability'):
            invalid = copy.deepcopy(body)
            payload = invalid['quote']['payload']
            if edit == 'hash': payload['offer_hash'] = services['marketplace-search']['offer_hash']
            if edit == 'operation': payload['capabilities_granted'] = ['net.http.operation.' + '0' * 64]
            if edit == 'signer': invalid['quote']['signer'] = '0' * 64
            if edit == 'fee': payload['settlement_terms']['success_fee_msat'] = 1
            if edit == 'runtime': payload['execution_limits']['max_runtime_ms'] = 2001
            if edit == 'memory': payload['execution_limits']['max_memory_bytes'] = 8388609
            if edit == 'output': payload['execution_limits']['max_output_bytes'] = demo.MAX_INPUT + 1
            if edit == 'duplicate-capability': payload['capabilities_granted'] *= 2
            self.assertFalse(demo.allowed_post('/v1/provider/deals', invalid, PROVIDER, services), edit)

    def test_named_http_service_requires_its_exact_operator_profile(self):
        services = {s['service_id']: s for s in approved_profile()['services']}
        service = services['marketplace-search']
        body = named_quote_request(service)
        self.assertFalse(demo.allowed_work(body))
        self.assertTrue(demo.allowed_post('/v1/provider/quotes', body, PROVIDER, services))
        for field, value in (('module_hash', service['module_hash']),
                             ('entrypoint', {'kind': 'module', 'value': 'run'}),
                             ('security', {'mode': 'standard', 'service_id': 'events.query'}),
                             ('requested_access', ['net.http.fetch']),
                             ('module_bytes_hex', '0061736d01000000'),
                             ('mounts', []), ('owner_token', 'test-only')):
            invalid = copy.deepcopy(body)
            invalid['execution'][field] = value
            self.assertFalse(demo.allowed_post('/v1/provider/quotes', invalid, PROVIDER, services), field)

    def test_free_pure_wasm_transport_accepts_browser_and_native_retry_keys(self):
        self.assertTrue(demo.allowed_post('/v1/provider/quotes', quote_request(), PROVIDER))
        self.assertTrue(demo.allowed_post('/v1/provider/deals', deal_request(), PROVIDER))

    def test_paid_quotes_or_bools_cannot_pass_zero_price_check(self):
        for price in (1, True, False, None, '0'):
            body = quote_request()
            body['max_price_sats'] = price
            self.assertFalse(demo.allowed_post('/v1/provider/quotes', body, PROVIDER))

    def test_host_capabilities_cannot_pass_even_with_valid_syntax(self):
        for capability in ('net.http.fetch', 'db.sqlite.query.read.private', 'net.http.operation.owner'):
            body = quote_request()
            body['submission']['workload']['requested_capabilities'] = [capability]
            self.assertFalse(demo.allowed_work(body))

    def test_other_runtimes_and_private_service_selectors_are_refused(self):
        for kind in ('oci_wasm', 'events_query', 'confidential_service', 'attested_wasm'):
            body = quote_request()
            body['kind'] = kind
            self.assertFalse(demo.allowed_work(body))
        for runtime in ('python', 'container', 'builtin'):
            self.assertFalse(demo.allowed_work({'kind': 'execution', 'execution': {'runtime': runtime}}))

    def test_fee_terms_wrong_provider_or_enlarged_resources_are_refused(self):
        for field, value in (('max_runtime_ms', 2001), ('max_memory_bytes', 8388609), ('fuel_limit', 50000001), ('max_input_bytes', 131073), ('max_output_bytes', -1)):
            body = deal_request()
            body['quote']['payload']['execution_limits'][field] = value
            self.assertFalse(demo.allowed_post('/v1/provider/deals', body, PROVIDER))
        for edit in ('provider', 'fee', 'method', 'payment', 'capability'):
            body = deal_request()
            if edit == 'provider': body['quote']['payload']['provider_id'] = 'e' * 64
            if edit == 'fee': body['quote']['payload']['settlement_terms']['base_fee_msat'] = 1
            if edit == 'method': body['quote']['payload']['settlement_terms']['method'] = 'lightning.prepaid.v1'
            if edit == 'payment': body['payment'] = {'token': 'never-forward'}
            if edit == 'capability': body['quote']['payload']['capabilities_granted'] = ['net.http.fetch']
            self.assertFalse(demo.allowed_post('/v1/provider/deals', body, PROVIDER))

    def test_invalid_module_or_large_input_is_refused_before_the_node(self):
        for module in ('source text', '0061736d010000001', '0061736d01000000' + '00' * 262145):
            body = quote_request()
            body['submission']['module_bytes_hex'] = module
            self.assertFalse(demo.allowed_work(body))
        body = quote_request()
        body['submission']['input'] = 'x' * 131073
        self.assertFalse(demo.allowed_work(body))

    def test_unsafe_numbers_deep_json_and_extra_fields_are_refused(self):
        for value in (float('inf'), 9007199254740992):
            body = quote_request()
            body['submission']['input'] = value
            self.assertFalse(demo.allowed_work(body))
        value = 0
        for _ in range(65): value = [value]
        self.assertFalse(demo.safe_json(value))
        body = quote_request()
        body['submission']['workload']['owner_token'] = 'never-forward'
        self.assertFalse(demo.allowed_work(body))

    def test_synthetic_catalog_only_accepts_its_exact_runtime_and_entrypoint(self):
        body = {'kind': 'execution', 'execution': {
            'schema_version': 'froglet/v1', 'workload_kind': demo.CATALOG, 'builtin_name': demo.CATALOG,
            'runtime': 'builtin', 'package_kind': 'builtin',
            'entrypoint': {'kind': 'builtin', 'value': demo.CATALOG},
            'contract_version': 'froglet.builtin.data_query.json.v1',
            'input_format': 'application/json+jcs', 'input_hash': 'a' * 64,
            'module_hash': 'b' * 64, 'security': {'mode': 'standard', 'service_id': demo.CATALOG},
            'input': {'op': 'select', 'collection': 'terminology'},
        }}
        self.assertTrue(demo.allowed_work(body))
        for field in ('entrypoint', 'security'):
            broken = copy.deepcopy(body)
            broken['execution'][field] = {'kind': 'command', 'value': '/bin/sh'}
            self.assertFalse(demo.allowed_work(broken))


class PublicDemoTrafficTests(unittest.TestCase):
    def test_missing_existing_ledger_never_silently_refills(self):
        with tempfile.TemporaryDirectory(prefix='froglet-public-ledger-test-') as root:
            with self.assertRaisesRegex(RuntimeError, 'existing-traffic-ledger'):
                demo.TrafficLedger(Path(root) / 'traffic.db')

    def test_restart_preserves_consumed_capacity_and_denies_next_request(self):
        with tempfile.TemporaryDirectory(prefix='froglet-public-ledger-test-') as root, patch.object(demo, 'MAX_HTTP_REQUESTS', 2):
            path = Path(root) / 'traffic.db'
            ledger = demo.TrafficLedger(path, first_boot=True)
            self.assertTrue(ledger.reserve(10))
            self.assertTrue(ledger.reserve(20))
            restarted = demo.TrafficLedger(path)
            self.assertFalse(restarted.reserve(0))
            with restarted.connect() as c:
                self.assertEqual(c.execute('SELECT requests,bytes FROM traffic').fetchone(), (2, 30 + 2 * demo.MAX_RESPONSE))

    def test_concurrent_admission_never_exceeds_the_cumulative_limit(self):
        with tempfile.TemporaryDirectory(prefix='froglet-public-ledger-test-') as root, patch.object(demo, 'MAX_HTTP_REQUESTS', 8):
            ledger = demo.TrafficLedger(Path(root) / 'traffic.db', first_boot=True)
            answers = []
            threads = [threading.Thread(target=lambda: answers.append(ledger.reserve(1))) for _ in range(24)]
            for thread in threads: thread.start()
            for thread in threads: thread.join()
            self.assertEqual(sum(answers), 8)
            with ledger.connect() as c:
                self.assertEqual(c.execute('SELECT requests FROM traffic').fetchone()[0], 8)

    def test_symlink_is_never_accepted_as_the_usage_ledger(self):
        with tempfile.TemporaryDirectory(prefix='froglet-public-ledger-test-') as root:
            path = Path(root)
            target = path / 'original.db'
            demo.TrafficLedger(target, first_boot=True)
            (path / 'link.db').symlink_to(target)
            with self.assertRaisesRegex(RuntimeError, 'existing-traffic-ledger'):
                demo.TrafficLedger(path / 'link.db', first_boot=True)


class PublicDemoHttpTests(unittest.TestCase):
    """Real ingress sockets; the loopback native boundary is observed, not qualified here."""

    def setUp(self):
        self.root = tempfile.TemporaryDirectory(prefix='froglet-public-http-test-')
        self.ledger = demo.TrafficLedger(Path(self.root.name) / 'traffic.db', first_boot=True)
        self.ledger_patch = patch.object(demo.Handler, 'ledger', self.ledger)
        self.provider_patch = patch.object(demo.Handler, 'provider_id', PROVIDER)
        self.ledger_patch.start()
        self.provider_patch.start()
        self.native_patch = patch.object(demo, 'native', return_value=(200, b'{"status":"ok"}'))
        self.native = self.native_patch.start()
        self.server = demo.Server(('127.0.0.1', 0), demo.Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
        self.native_patch.stop()
        self.provider_patch.stop()
        self.ledger_patch.stop()
        self.root.cleanup()

    def request(self, path, body=None, *, method='GET', headers=None):
        with closing(http.client.HTTPConnection('127.0.0.1', self.server.server_port, timeout=5)) as connection:
            connection.request(method, path, body=body, headers=headers or {})
            with closing(connection.getresponse()) as response:
                return response.status, dict(response.getheaders()), response.read()

    def usage(self):
        with self.ledger.connect() as c:
            return c.execute('SELECT requests,bytes FROM traffic').fetchone()

    def test_disabled_or_altered_named_requests_never_reach_native_or_spend_traffic(self):
        services = {s['service_id']: s for s in approved_profile()['services']}
        profile = services['marketplace-search']
        body = named_quote_request(profile)
        self.assertEqual(self.request('/v1/provider/services/' + profile['service_id'])[0], 404)
        self.assertEqual(self.request('/v1/provider/quotes', demo.encoded(body), method='POST',
                                     headers={'Content-Type': 'application/json'})[0], 400)
        with patch.object(demo.Handler, 'approved_services', services):
            for field, value in (('runtime', 'python'), ('package_kind', 'oci_image'),
                                 ('requested_access', ['net.http.operation.' + '0' * 64]),
                                 ('requested_access', body['execution']['requested_access'] * 2),
                                 ('module_hash', profile['module_hash']), ('inline_source', 'test-only'),
                                 ('security', {'mode': 'tee', 'service_id': profile['service_id']})):
                invalid = copy.deepcopy(body)
                invalid['execution'][field] = value
                self.assertEqual(self.request('/v1/provider/quotes', demo.encoded(invalid), method='POST',
                                             headers={'Content-Type': 'application/json'})[0], 400)
            invalid = copy.deepcopy(body)
            invalid['offer_id'] = 'execute.compute.generic'
            self.assertEqual(self.request('/v1/provider/quotes', demo.encoded(invalid), method='POST',
                                         headers={'Content-Type': 'application/json'})[0], 400)
        self.native.assert_not_called()
        self.assertEqual(self.usage(), (0, 0))

    def test_named_quote_socket_pins_native_offer_and_preserves_spent_traffic_on_mismatch(self):
        services = {s['service_id']: s for s in approved_profile()['services']}
        service = services['marketplace-search']
        body = named_quote_request(service)
        quote = named_quote_artifact(service)
        with patch.object(demo.Handler, 'approved_services', services, create=True):
            self.native.return_value = 201, demo.encoded(quote)
            self.assertEqual(self.request('/v1/provider/quotes', demo.encoded(body), method='POST',
                                         headers={'Content-Type': 'application/json'})[0], 201)
            self.native.assert_called_once_with('POST', '/v1/provider/quotes', demo.encoded(body))
            quote['payload']['offer_hash'] = '0' * 64
            self.native.return_value = 201, demo.encoded(quote)
            self.assertEqual(self.request('/v1/provider/quotes', demo.encoded(body), method='POST',
                                         headers={'Content-Type': 'application/json'})[0], 503)
        self.assertEqual(self.usage(), (2, 2 * (len(demo.encoded(body)) + demo.MAX_RESPONSE)))

    def test_allowed_get_strips_caller_credentials_and_is_never_cached(self):
        status, headers, _ = self.request('/v1/provider/descriptor', headers={
            'Authorization': 'Bearer test-only-not-a-credential', 'Cookie': 'test=only',
            'X-Froglet-Access-Token': 'test-only',
        })
        self.assertEqual(status, 200)
        self.assertEqual(headers['cache-control'], 'no-store')
        self.assertEqual(headers['connection'], 'close')
        self.native.assert_called_once_with('GET', '/v1/provider/descriptor', None)
        self.assertEqual(self.usage(), (1, demo.MAX_RESPONSE))

    def test_forbidden_routes_and_wrong_methods_never_reach_native(self):
        for path in ('/v1/provider/usage', '/v1/runtime/deals', '/v1/provider/artifacts/publish',
                     '/v1/provider/descriptor?token=test', '/v1/provider/%64escriptor'):
            self.assertEqual(self.request(path)[0], 404)
        self.assertEqual(self.request('/health', b'{}', method='POST')[0], 404)
        self.native.assert_not_called()
        self.assertEqual(self.usage(), (0, 0))

    def publication_fixture(self):
        descriptor = {'cursor': 3, 'hash': 'd' * 64, 'kind': 'descriptor',
                      'actor_id': PROVIDER, 'document': {'artifact_type': 'descriptor',
                      'hash': 'd' * 64, 'payload': {'provider_id': PROVIDER}}}
        offer = {'cursor': 7, 'hash': 'e' * 64, 'kind': 'offer',
                 'actor_id': PROVIDER, 'document': public_offer_document(demo.CATALOG, 'e' * 64, 'd' * 64)}
        revision = {'revision_hash': 'f' * 64, 'payload': {'offer_hash': 'e' * 64}}
        starter = {'op': 'select', 'collection': 'terminology',
                   'columns': ['source', 'target'], 'limit': 100}

        def respond(method, path, body=None):
            values = {
                '/v1/provider/offers': {'offers': [offer['document'],
                    {'artifact_type': 'offer', 'hash': '9' * 64,
                     'payload': {'offer_id': 'events.query'}}]},
                '/v1/artifacts/' + 'e' * 64: offer,
                '/v1/artifacts/' + 'd' * 64: descriptor,
                '/v1/provider/services/' + demo.CATALOG:
                    {'publication_revision': revision,
                     'service': {'starter': json.dumps(starter)}},
            }
            if method == 'POST':
                return 200, demo.encoded({'signed_canary': True})
            return 200, demo.encoded(values[path])
        self.native.side_effect = respond
        return descriptor, offer, revision, starter

    def approved_publication_fixture(self):
        descriptor, catalog, _, _ = self.publication_fixture()
        catalog_response = self.native.side_effect
        services = {s['service_id']: s for s in approved_profile()['services']}
        shared_descriptor = {'cursor': 5, 'hash': '6' * 64, 'kind': 'descriptor',
                             'actor_id': PROVIDER, 'document': {'artifact_type': 'descriptor',
                             'hash': '6' * 64, 'payload': {'provider_id': PROVIDER}}}
        records = [descriptor, shared_descriptor, catalog]
        for index, service in enumerate(services.values(), 11):
            records.append({'cursor': index, 'hash': service['offer_hash'], 'kind': 'offer',
                            'actor_id': PROVIDER, 'document': public_offer_document(
                                service['offer_id'], service['offer_hash'], service['descriptor_hash'], service)})
        by_hash = {r['hash']: r for r in records}

        def respond(method, path, body=None):
            if path == '/v1/provider/offers':
                visitor_receipt = {'artifact_type': 'receipt', 'hash': '0' * 64,
                                   'payload': {'provider_id': PROVIDER, 'offer_id': 'marketplace-receipts'}}
                return 200, demo.encoded({'offers': [r['document'] for r in records if r['kind'] == 'offer'] + [visitor_receipt]})
            if path.startswith('/v1/artifacts/') and path.rsplit('/', 1)[1] in by_hash:
                return 200, demo.encoded(by_hash[path.rsplit('/', 1)[1]])
            return catalog_response(method, path, body)
        self.native.side_effect = respond
        return services, records, by_hash

    def test_offers_response_keeps_supported_signed_documents_and_refuses_profile_drift(self):
        services, records, _ = self.approved_publication_fixture()
        selected = [r['document'] for r in records if r['kind'] == 'offer']
        compute = public_offer_document('execute.compute', 'c' * 64, 'd' * 64)
        selected.append(compute)
        excluded = []
        for offer_id in ('events.query', 'execute.compute.generic', 'execute.compute.host', 'internal-owner-service'):
            document = copy.deepcopy(compute)
            document['payload']['offer_id'] = offer_id
            excluded.append(document)
        foreign = copy.deepcopy(compute)
        foreign['signer'] = foreign['payload']['provider_id'] = '0' * 64
        receipt = {'artifact_type': 'receipt', 'payload': {'provider_id': PROVIDER, 'offer_id': demo.CATALOG}}
        envelope = {'offers': selected + excluded + [foreign, receipt]}
        before = copy.deepcopy(envelope)
        self.native.side_effect = lambda *_: (200, demo.encoded(envelope))
        with patch.object(demo.Handler, 'approved_services', services):
            status, _, raw = self.request('/v1/provider/offers')
            self.assertEqual(status, 200)
            self.assertEqual(json.loads(raw), {'offers': selected})
            self.assertEqual(envelope, before)
            self.assertEqual(self.usage(), (1, demo.MAX_RESPONSE))
            named = selected[1]
            for key, value in (('hash', '0' * 64), ('descriptor_hash', '0' * 64), ('runtime', 'any')):
                changed = named if key == 'hash' else (named['payload'] if key == 'descriptor_hash' else named['payload']['execution_profile'])
                previous = changed[key]
                changed[key] = value
                self.assertEqual(self.request('/v1/provider/offers')[0], 503)
                changed[key] = previous
        with patch.object(demo.Handler, 'approved_services', {}):
            status, _, raw = self.request('/v1/provider/offers')
            self.assertEqual(status, 200)
            self.assertEqual(json.loads(raw), {'offers': [selected[0], compute]})
        self.assertTrue(all(call.args[:2] == ('GET', '/v1/provider/offers') for call in self.native.call_args_list))

    def test_selected_feed_adds_only_exact_approved_offers_and_deduplicates_their_descriptor(self):
        services, records, by_hash = self.approved_publication_fixture()
        with patch.object(demo.Handler, 'approved_services', services):
            status, _, raw = self.request('/v1/feed?limit=100')
            self.assertEqual(status, 200)
            self.assertEqual(json.loads(raw)['artifacts'], records)
            self.assertEqual(set(json.loads(raw)['active_offer_hashes']),
                             {r['hash'] for r in records if r['kind'] == 'offer'})
            self.assertEqual(self.request('/v1/artifacts/' + '0' * 64)[0], 404)
            self.assertFalse(any(call.args[1] == '/v1/feed' or call.args[1].endswith('0' * 64)
                                 for call in self.native.call_args_list))
            by_hash[services['marketplace-search']['offer_hash']]['document']['payload']['descriptor_hash'] = '0' * 64
            self.assertEqual(self.request('/v1/feed')[0], 503)

    def test_named_canary_and_metadata_require_the_current_exact_profile_and_starter(self):
        services, _, _ = self.approved_publication_fixture()
        profile = services['marketplace-provider']
        metadata, starter = named_service_metadata(profile)
        respond = self.native.side_effect
        service_path = '/v1/provider/services/' + profile['service_id']

        def native_response(method, path, body=None):
            if path == service_path:
                return 200, demo.encoded(metadata)
            if method == 'POST':
                return 200, b'{"signed_canary":true}'
            return respond(method, path, body)

        self.native.side_effect = native_response
        path = '/v1/publications/' + profile['revision_hash'] + '/canary'
        body = {'schema_version': 'froglet.publication-canary-request.v1',
                'revision_hash': profile['revision_hash'], 'offer_hash': profile['offer_hash'],
                'challenge': 'b' * 64, 'input': starter}
        with patch.object(demo.Handler, 'approved_services', services):
            self.assertEqual(self.request(service_path)[0], 200)
            self.assertEqual(self.request(path, demo.encoded(body), method='POST',
                                         headers={'Content-Type': 'application/json'})[0], 200)
            for key, value in (('binding_hash', '0' * 64), ('capabilities', ['net.http.fetch']),
                               ('entrypoint', 'private.wasm')):
                previous = metadata['service'][key]
                metadata['service'][key] = value
                self.native.reset_mock()
                self.assertEqual(self.request(service_path)[0], 503)
                self.assertEqual(self.request(path, demo.encoded(body), method='POST',
                                             headers={'Content-Type': 'application/json'})[0], 400)
                self.assertFalse(any(call.args[0] == 'POST' for call in self.native.call_args_list))
                metadata['service'][key] = previous
            body['input'] = {'provider_id': '1' * 64}
            self.assertEqual(self.request(path, demo.encoded(body), method='POST',
                                         headers={'Content-Type': 'application/json'})[0], 400)
            body['input'] = starter
            metadata['publication_revision']['revision_hash'] = '0' * 64
            self.assertEqual(self.request(service_path)[0], 503)
            self.native.reset_mock()
            self.assertEqual(self.request(path, demo.encoded(body), method='POST',
                                         headers={'Content-Type': 'application/json'})[0], 400)
            self.assertFalse(any(call.args[0] == 'POST' for call in self.native.call_args_list))

    def test_discovery_feed_keeps_native_cursors_and_never_reads_visitor_feed(self):
        descriptor, offer, _, _ = self.publication_fixture()
        status, _, raw = self.request('/v1/feed?limit=1&cursor=0')
        self.assertEqual(status, 200)
        feed = json.loads(raw)
        self.assertEqual(feed['artifacts'], [descriptor])
        self.assertEqual(feed['active_offer_hashes'], [offer['hash']])
        self.assertTrue(feed['has_more'])
        self.assertEqual(feed['next_cursor'], 3)
        status, _, raw = self.request('/v1/feed?cursor=3&limit=100')
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(raw)['artifacts'], [offer])
        self.assertFalse(json.loads(raw)['has_more'])
        status, _, raw = self.request('/v1/feed?cursor=7&limit=100')
        self.assertEqual(status, 200)
        self.assertEqual(json.loads(raw)['artifacts'], [])
        self.assertTrue(all(not call.args[1].startswith('/v1/feed') for call in self.native.call_args_list))

    def test_discovery_cannot_fetch_receipts_or_expand_feed_queries(self):
        self.publication_fixture()
        self.assertEqual(self.request('/v1/artifacts/' + '1' * 64)[0], 404)
        self.assertFalse(any(call.args[1].endswith('1' * 64) for call in self.native.call_args_list))
        self.native.reset_mock()
        for path in ('/v1/feed?limit=0', '/v1/feed?cursor=-1',
                     '/v1/feed?limit=101', '/v1/feed?limit=1&limit=2',
                     '/v1/feed?cursor=1&owner_token=x', '/v1/feed?cursor=%31'):
            self.assertEqual(self.request(path)[0], 404)
        self.native.assert_not_called()

    def test_publication_canary_only_for_current_catalog_exact_fixture(self):
        _, offer, revision, starter = self.publication_fixture()
        path = '/v1/publications/' + revision['revision_hash'] + '/canary'
        body = {'schema_version': 'froglet.publication-canary-request.v1',
                'revision_hash': revision['revision_hash'], 'offer_hash': offer['hash'],
                'challenge': 'b' * 64, 'input': starter}
        self.assertEqual(self.request(path, json.dumps(body), method='POST',
                                    headers={'Content-Type': 'application/json'})[0], 200)
        self.assertEqual(self.native.call_args.args[:2], ('POST', path))
        for field, value in (('input', {'op': 'describe'}), ('offer_hash', '9' * 64),
                             ('challenge', 'invalid'), ('owner_token', 'never-forward')):
            invalid = dict(body, **{field: value})
            self.native.reset_mock()
            self.assertEqual(self.request(path, json.dumps(invalid), method='POST',
                                         headers={'Content-Type': 'application/json'})[0], 400)
            self.assertFalse(any(call.args[0] == 'POST' for call in self.native.call_args_list))

    def test_metadata_traffic_reserves_smaller_enforced_response_ceiling(self):
        self.publication_fixture()
        status, _, _ = self.request('/v1/feed?limit=100')
        self.assertEqual(status, 200)
        self.assertEqual(self.usage(), (1, demo.MAX_METADATA_RESPONSE))
        self.native.side_effect = None
        self.native.return_value = 200, demo.encoded({'data': 'x' * demo.MAX_METADATA_RESPONSE})
        self.assertEqual(self.request('/v1/node/capabilities')[0], 503)

    def test_malformed_utf8_json_unsafe_numbers_and_deep_bodies_do_not_reach_native(self):
        deep = quote_request()
        nested = 0
        for _ in range(70):
            nested = [nested]
        deep['submission']['input'] = nested
        unsafe = quote_request()
        unsafe['submission']['input'] = 9007199254740992
        for body in (b'\xff', b'{', b'null', b'{"x":NaN}', json.dumps(deep).encode(), json.dumps(unsafe).encode()):
            with self.subTest(body=body[:40]):
                self.assertEqual(self.request('/v1/provider/quotes', body, method='POST', headers={'Content-Type': 'application/json'})[0], 400)
        self.native.assert_not_called()
        self.assertEqual(self.usage(), (0, 0))

    def test_wrong_content_type_chunking_and_oversize_declaration_are_refused_before_body_read(self):
        for headers, expected in (({'Content-Type': 'text/plain'}, 415),
                                  ({'Content-Type': 'application/json', 'Transfer-Encoding': 'chunked'}, 415),
                                  ({'Content-Type': 'application/json', 'Content-Length': str(demo.MAX_REQUEST + 1)}, 413),
                                  ({'Content-Type': 'application/json', 'Content-Length': '-1'}, 411)):
            self.assertEqual(self.request('/v1/provider/quotes', b'', method='POST', headers=headers)[0], expected)
        self.native.assert_not_called()
        self.assertEqual(self.usage(), (0, 0))

    def test_native_failure_keeps_the_traffic_reservation_and_returns_recovery_advice(self):
        self.native.side_effect = TimeoutError('test-only native timeout')
        status, headers, body = self.request('/v1/provider/descriptor')
        self.assertEqual(status, 503)
        self.assertEqual(headers['cache-control'], 'no-store')
        self.assertIn('same job', json.loads(body)['error'])
        self.assertEqual(self.usage(), (1, demo.MAX_RESPONSE))

    def test_byte_exhaustion_refuses_next_call_without_new_native_traffic(self):
        with patch.object(demo, 'MAX_HTTP_BYTES', demo.MAX_RESPONSE):
            self.assertEqual(self.request('/v1/provider/descriptor')[0], 200)
            self.assertEqual(self.request('/v1/provider/descriptor')[0], 429)
        self.native.assert_called_once()
        self.assertEqual(self.usage(), (1, demo.MAX_RESPONSE))

    def test_health_remains_available_after_traffic_exhaustion_without_refilling_it(self):
        with patch.object(demo, 'MAX_HTTP_REQUESTS', 0):
            self.assertEqual(self.request('/v1/provider/descriptor')[0], 429)
            self.assertEqual(self.request('/health')[0], 200)
        self.native.assert_called_once_with('GET', '/health', None)
        self.assertEqual(self.usage(), (0, 0))

    def test_ninth_concurrent_operation_is_refused_before_native_admission(self):
        entered = threading.Barrier(9)
        release = threading.Event()
        responses = []
        errors = []

        def blocked_native(*_args):
            entered.wait(timeout=4)
            if not release.wait(timeout=4):
                raise TimeoutError('test-only release timeout')
            return 200, b'{"status":"ok"}'

        def request_one():
            try:
                responses.append(self.request('/v1/provider/descriptor')[0])
            except Exception as error:
                errors.append(error)

        self.native.side_effect = blocked_native
        threads = [threading.Thread(target=request_one) for _ in range(8)]
        for thread in threads:
            thread.start()
        try:
            entered.wait(timeout=4)
            self.assertEqual(self.request('/v1/provider/descriptor')[0], 429)
            self.assertEqual(self.native.call_count, 8)
        finally:
            release.set()
            for thread in threads:
                thread.join(timeout=5)
        self.assertFalse(errors)
        self.assertEqual(responses, [200] * 8)
        self.assertEqual(self.usage(), (8, 8 * demo.MAX_RESPONSE))


if __name__ == '__main__':
    unittest.main()
