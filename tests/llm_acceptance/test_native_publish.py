"""Ensure the agent acceptance gate rejects plausible but false success."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from run_native_publish import CATALOG, verify_package


class NativeAcceptanceValidationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        original = json.dumps(CATALOG).encode()
        (self.root / 'catalog.json').write_bytes(original)
        self.original_hash = hashlib.sha256(original).hexdigest()
        self.project = self.root / 'prepared'
        (self.project / '.froglet').mkdir(parents=True)
        self.snapshot = {
            'metadata': [CATALOG['metadata']],
            'items': [{k: row[k] for k in ('id', 'name', 'stock')} for row in CATALOG['items']],
        }
        self.write_snapshot()
        (self.project / 'froglet-service.toml').write_text('''
[data]
path = "snapshot.json"
[price]
sats = 0
[settlement]
method = "none"
[verification.input]
op = "select"
collection = "items"
limit = 3
''')

    def write_snapshot(self):
        raw = json.dumps(self.snapshot).encode()
        (self.project / 'snapshot.json').write_bytes(raw)
        (self.project / '.froglet/preparation.json').write_text(json.dumps({
            'snapshot_file': 'snapshot.json', 'snapshot_sha256': hashlib.sha256(raw).hexdigest(),
        }))

    def test_selected_values_and_identifiers_are_preserved(self):
        verify_package(self.root, self.original_hash)

    def test_rejects_excluded_data_even_with_matching_hash(self):
        self.snapshot['items'][0]['notes'] = CATALOG['items'][0]['notes']
        self.write_snapshot()
        with self.assertRaisesRegex(AssertionError, 'Private notes leaked'):
            verify_package(self.root, self.original_hash)

    def test_rejects_normalization_that_loses_identifier_zeros(self):
        self.snapshot['items'][0]['id'] = 1
        self.write_snapshot()
        with self.assertRaisesRegex(AssertionError, 'Rows changed'):
            verify_package(self.root, self.original_hash)

    def test_rejects_manifest_pointing_at_original_data(self):
        path = self.project / 'froglet-service.toml'
        path.write_text(path.read_text().replace('snapshot.json', '../catalog.json'))
        with self.assertRaisesRegex(AssertionError, 'Manifest serves different bytes'):
            verify_package(self.root, self.original_hash)


if __name__ == '__main__':
    unittest.main()
