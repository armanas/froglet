"""Public ingress boundaries; no real cloud or runtime is mocked as qualified."""
import copy
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


class PublicDemoPolicyTests(unittest.TestCase):
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


if __name__ == '__main__':
    unittest.main()
