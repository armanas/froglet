"""Real protected-file checks for optional public HTTP tool configuration."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch


SOURCE = Path(__file__).resolve().parents[2] / 'integrations/public-demo'


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


proxy = load('entrypoint_test_proxy', SOURCE / 'provider_proxy.py')
entrypoint = load('entrypoint_test_supervisor', SOURCE / 'entrypoint.py')


class PublicHttpStartupTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix='froglet-http-startup-')
        self.root = Path(self.directory.name)
        self.root.chmod(0o700)
        self.provider = 'a' * 64
        (self.root / 'expected-provider-id').write_text(self.provider + '\n')
        self.services = []
        for index, name in enumerate(sorted(proxy.APPROVED_SERVICE_IDS), 1):
            self.services.append({
                'service_id': name, 'offer_id': name,
                'offer_hash': hashlib.sha256((name + ':offer').encode()).hexdigest(),
                'descriptor_hash': 'c' * 64,
                'revision_hash': hashlib.sha256((name + ':revision').encode()).hexdigest(),
                'module_hash': 'e' * 64, 'binding_hash': 'f' * 64,
                'operation_hash': str(index) * 64, 'entrypoint': 'run',
            })
        self.profile = self.root / 'approved-services.json'
        self.profile.write_text(json.dumps({
            'schema_version': proxy.APPROVED_PROFILE_SCHEMA,
            'provider_id': self.provider, 'services': self.services,
        }))
        self.profile.chmod(0o600)
        self.policy = self.root / 'approved-http-policy.toml'
        self.policy_text = '''[http]
operations_only = true
operation_hashes = ["%s", "%s", "%s"]
allowed_hosts = ["froglet.dev"]
allow_private_networks = false
max_calls_per_execution = 1
max_timeout_ms = 2000
max_request_body_bytes = 2048
max_response_body_bytes = 131072
max_redirects = 0
''' % tuple(service['operation_hash'] for service in self.services)
        self.write_policy(self.policy_text)
        self.root_patch = patch.object(entrypoint, 'ROOT', self.root)
        self.module_patch = patch.dict(sys.modules, {'provider_proxy': proxy})
        self.root_patch.start()
        self.module_patch.start()

    def tearDown(self):
        self.module_patch.stop()
        self.root_patch.stop()
        self.directory.cleanup()

    def write_policy(self, text):
        self.policy.write_text(text)
        self.policy.chmod(0o600)

    def environment(self):
        return {
            'FROGLET_PUBLIC_DEMO_PROFILE_PATH': str(self.profile),
            'FROGLET_PUBLIC_DEMO_PROFILE_SHA256': hashlib.sha256(self.profile.read_bytes()).hexdigest(),
            'FROGLET_WASM_POLICY_PATH': str(self.policy),
            'FROGLET_PUBLIC_DEMO_HTTP_POLICY_SHA256': hashlib.sha256(self.policy.read_bytes()).hexdigest(),
        }

    def check(self, environment=None):
        with patch.dict(os.environ, self.environment() if environment is None else environment, clear=True):
            entrypoint.verify_approved_http_policy()

    def test_existing_demo_has_no_optional_configuration_dependency(self):
        self.profile.unlink()
        self.policy.unlink()
        self.check({})

    def test_supervisor_and_ingress_agree_on_disabled_and_incomplete_profile_settings(self):
        # A disagreement would start the native node and then crash-loop the ingress.
        unset, empty = {}, {'FROGLET_PUBLIC_DEMO_PROFILE_PATH': '', 'FROGLET_PUBLIC_DEMO_PROFILE_SHA256': ''}
        path_only = {'FROGLET_PUBLIC_DEMO_PROFILE_PATH': str(self.profile), 'FROGLET_PUBLIC_DEMO_PROFILE_SHA256': ''}
        digest_only = {'FROGLET_PUBLIC_DEMO_PROFILE_PATH': '', 'FROGLET_PUBLIC_DEMO_PROFILE_SHA256': '0' * 64}
        for environment in (unset, empty):
            with self.subTest(environment=environment), patch.dict(os.environ, environment, clear=True):
                self.assertIsNone(proxy.approved_profile_settings())
                self.assertIsNone(entrypoint.verify_approved_http_policy())
        for environment in (path_only, digest_only):
            with self.subTest(environment=environment), patch.dict(os.environ, environment, clear=True):
                with self.assertRaisesRegex(RuntimeError, 'approved-profile-configuration-incomplete'):
                    proxy.approved_profile_settings()
                with self.assertRaisesRegex(RuntimeError, 'approved-profile-configuration-incomplete'):
                    entrypoint.verify_approved_http_policy()

    def test_named_tools_require_an_existing_provider_identity(self):
        (self.root / 'expected-provider-id').unlink()
        with self.assertRaisesRegex(RuntimeError, 'approved-http-existing-identity-required'):
            self.check()

    def test_exact_protected_three_operation_policy_is_accepted_without_writes(self):
        before = {path.name: path.read_bytes() for path in self.root.iterdir()}
        self.check()
        self.assertEqual(before, {path.name: path.read_bytes() for path in self.root.iterdir()})

    def test_matching_digest_cannot_authorize_more_hosts_or_capabilities(self):
        for text in (
            self.policy_text.replace('["froglet.dev"]', '["froglet.dev", "other.example"]'),
            self.policy_text.replace('operations_only = true', 'operations_only = false'),
            self.policy_text.replace('allow_private_networks = false', 'allow_private_networks = true'),
            self.policy_text.replace('max_calls_per_execution = 1', 'max_calls_per_execution = 2'),
            self.policy_text.replace('max_timeout_ms = 2000', 'max_timeout_ms = 2001'),
            self.policy_text.replace('max_redirects = 0', 'max_redirects = 1'),
            self.policy_text + '\n[http.auth_profiles.private]\nheader_value = "test-only"\n',
        ):
            with self.subTest(text=text):
                self.write_policy(text)
                with self.assertRaisesRegex(RuntimeError, 'policy-scope-mismatch'):
                    self.check()

    def test_bool_cannot_stand_in_for_integer_limit(self):
        self.write_policy(self.policy_text.replace('max_calls_per_execution = 1', 'max_calls_per_execution = true'))
        with self.assertRaisesRegex(RuntimeError, 'policy-scope-mismatch'):
            self.check()

    def test_stale_digest_incomplete_profile_and_missing_policy_are_refused(self):
        environment = self.environment()
        environment['FROGLET_PUBLIC_DEMO_HTTP_POLICY_SHA256'] = '0' * 64
        with self.assertRaisesRegex(RuntimeError, 'policy-digest-mismatch'):
            self.check(environment)
        environment = self.environment()
        environment.pop('FROGLET_PUBLIC_DEMO_PROFILE_SHA256')
        with self.assertRaises(RuntimeError):
            self.check(environment)
        environment = self.environment()
        environment.pop('FROGLET_WASM_POLICY_PATH')
        with self.assertRaisesRegex(RuntimeError, 'exact-policy-required'):
            self.check(environment)

    def test_symlink_or_writable_policy_is_refused_before_native_start(self):
        environment = self.environment()
        target = self.root / 'policy-copy.toml'
        self.policy.rename(target)
        self.policy.symlink_to(target)
        with self.assertRaises(OSError):
            self.check(environment)
        self.policy.unlink()
        target.rename(self.policy)
        self.policy.chmod(0o666)
        with self.assertRaisesRegex(RuntimeError, 'policy-not-protected'):
            self.check(environment)


if __name__ == '__main__':
    unittest.main()
