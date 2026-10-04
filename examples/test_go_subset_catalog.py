"""Offline parser/refusal tests; no upstream downloads or scientific claims."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from examples import go_subset_catalog


def fixture():
    return ("?x\t?label\n" + "".join(f'<http://purl.obolibrary.org/obo/GO_{number:07d}>\t'
        + json.dumps(f"Synthetic term {number}") + "\n" for number in range(140))).encode()


class GoSubsetCatalogTests(unittest.TestCase):
    def test_unreviewed_source_digest_refused_before_writing(self):
        with tempfile.TemporaryDirectory() as root:
            destination = Path(root) / "output"
            with self.assertRaisesRegex(ValueError, "digest differs"):
                go_subset_catalog.create_preparation(fixture(), destination)
            self.assertFalse(destination.exists())

    def test_pinned_fixture_preserves_labels_and_identifier_forms(self):
        raw = fixture()
        with patch.object(go_subset_catalog, "SOURCE_SHA256", hashlib.sha256(raw).hexdigest()):
            rows = go_subset_catalog.parse_catalog(raw)["go_terms"]
        self.assertEqual(len(rows), 140)
        self.assertEqual(rows[0], {"id": "GO:0000000", "name": "Synthetic term 0",
            "term_uri": "http://purl.obolibrary.org/obo/GO_0000000"})

    def test_duplicate_terms_and_invalid_uris_refused(self):
        for changed in [fixture().replace(b"GO_0000139", b"GO_0000000"),
                        fixture().replace(b"GO_0000000", b"OTHER_0000000")]:
            with self.subTest(changed=changed[:80]), patch.object(
                    go_subset_catalog, "SOURCE_SHA256", hashlib.sha256(changed).hexdigest()), self.assertRaises(ValueError):
                go_subset_catalog.parse_catalog(changed)

    def test_existing_destination_is_not_overwritten(self):
        raw = fixture()
        with tempfile.TemporaryDirectory() as root, patch.object(
                go_subset_catalog, "SOURCE_SHA256", hashlib.sha256(raw).hexdigest()):
            path = Path(root)
            marker = path / "existing.txt"
            marker.write_text("keep")
            with self.assertRaises(FileExistsError):
                go_subset_catalog.create_preparation(raw, path)
            self.assertEqual(marker.read_text(), "keep")
            self.assertFalse((path / "go-terms.json").exists())


if __name__ == "__main__":
    unittest.main()
