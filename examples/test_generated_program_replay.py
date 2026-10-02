"""Focused replay input/export safety checks; real nodes are exercised by CLI."""
from pathlib import Path
import sqlite3
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import json

from examples import generated_program_replay as replay


class ReplayInputTests(unittest.TestCase):
    def test_successful_task_read_of_failed_execution_is_not_a_failed_tool_call(self):
        report = {"status": "failed", "deal_id": "opaque-operation-id", "deal_hash": "a" * 64,
                  "receipt_verification": {"verified": True}}
        response = {"isError": False, "structuredContent": report}
        self.assertEqual(replay.verified_native_response(response, "failed", read=True), report)
        with self.assertRaises(RuntimeError):
            replay.verified_native_response(response, "failed")
        self.assertEqual(replay.verified_native_response({**response, "isError": True}, "failed"), report)

    def test_opaque_operation_id_does_not_replace_signed_deal_hash_or_verification(self):
        report = {"status": "succeeded", "deal_id": "abc123", "deal_hash": "b" * 64,
                  "receipt_verification": {"verified": True}}
        self.assertEqual(replay.verified_native_response({"isError": False, "structuredContent": report}, "succeeded"), report)
        for changed in ({**report, "deal_hash": "abc123"}, {**report, "receipt_verification": {"verified": False}}):
            with self.assertRaises(RuntimeError):
                replay.verified_native_response({"isError": False, "structuredContent": changed}, "succeeded")

    def test_expected_answers_preserve_json_numeric_types(self):
        self.assertFalse(replay.exact_json({"count": 1}, {"count": True}))
        self.assertFalse(replay.exact_json({"count": 1}, {"count": 1.0}))
        self.assertTrue(replay.exact_json({"a": 1, "b": 2}, {"b": 2, "a": 1}))

    def test_original_and_held_out_cases_use_supplied_values_without_computation(self):
        actual = replay.cases_from_files({"rows": []}, {"expected_result": {"batches": []}},
            {"cases": [{"label": "empty", "input": None}]},
            {"cases": [{"label": "empty", "expected_result": [1, 2]}]})
        self.assertEqual(actual, [{"label": "original", "input": {"rows": []}, "expected_result": {"batches": []}},
                                  {"label": "empty", "input": None, "expected_result": [1, 2]}])

    def test_unsafe_duplicate_reserved_or_missing_labels_are_refused(self):
        for label in ("../escape", "original", "a/b", "", "UPPER"):
            with self.subTest(label=label), self.assertRaises(RuntimeError):
                replay.cases_from_files(None, None, {"cases": [{"label": label, "input": None}]},
                    {"cases": [{"label": label, "expected_result": None}]})
        with self.assertRaises(RuntimeError):
            replay.cases_from_files(None, None, {"cases": [{"label": "one", "input": None}]}, {"cases": []})
        with self.assertRaises(RuntimeError):
            replay.cases_from_files(None, None, {"cases": []},
                {"cases": [{"label": "a", "expected_result": None}, {"label": "a", "expected_result": None}]})

    def test_selected_projection_preserves_identifiers_and_repeated_values(self):
        row = {"record_id": "0001", "batch_id": "A", "signal_milliunits": 0, "qc_status": "pass", "operator_note": "private"}
        normalized, selected = replay.selected_source({"metadata": {"private": True}, "assay_readouts": [row, dict(row, record_id="0002")]})
        self.assertEqual(len(selected), 2)
        self.assertEqual([item["record_id"] for item in selected], ["0001", "0002"])
        self.assertTrue(all(set(item) == set(replay.FIELDS) for item in selected))
        self.assertEqual(set(normalized), {"assay_readouts"})
        self.assertEqual(normalized["assay_readouts"][0]["operator_note"], "private")

    def test_duplicate_keys_and_nonfinite_numbers_are_refused(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "input.json"
            for text in ('{"a":1,"a":2}', '{"a":NaN}', '{"a":Infinity}'):
                with self.subTest(text=text), self.assertRaises(ValueError):
                    path.write_text(text)
                    replay.load_json(path)

    def test_pin_mismatch_and_symlink_are_refused(self):
        with tempfile.TemporaryDirectory() as root:
            path = Path(root) / "module.wasm"
            path.write_bytes(b"\0asm\x01\0\0\0")
            with self.assertRaises(RuntimeError):
                replay.pinned_file(path, "0" * 64, "Module", 256)
            link = Path(root) / "link.wasm"
            link.symlink_to(path)
            with self.assertRaises(RuntimeError):
                replay.pinned_file(link, replay.sha(path), "Module", 256)
            self.assertEqual(replay.pinned_file(path, replay.sha(path), "Module", 256), path.resolve())


class ReplayExportTests(unittest.TestCase):
    def test_provider_operation_ids_are_indexed_by_distinct_signed_hash(self):
        values = [{"deal_id": "opaque-http-operation", "deal_artifact": {"hash": "signed-kernel-hash"}}]
        self.assertEqual(replay.by_signed_deal_hash(values)["signed-kernel-hash"]["deal_id"], "opaque-http-operation")
        with self.assertRaises(RuntimeError):
            replay.by_signed_deal_hash(values + values)

    def make_db(self, data):
        db = sqlite3.connect(data / "node.db")
        db.execute("CREATE TABLE deals(deal_id,idempotency_key,service_id,workload_hash,spec_json,quote_json,deal_artifact_json,status,result_json,result_hash,error,receipt_artifact_json,created_at,updated_at)")
        for index, (deal_id, requester) in enumerate((("canary", "other-requester"), ("alice", "alice-requester"))):
            quote = {"hash": "q-" + deal_id, "payload": {"provider_id": "bob", "requester_id": requester,
                "workload_kind": "compute.wasm.v1", "descriptor_hash": "descriptor", "offer_hash": "offer"}}
            db.execute("INSERT INTO deals VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)", (deal_id, deal_id, "service", "workload", '{"kind":"wasm"}', json.dumps(quote),
                json.dumps({"hash": deal_id}), "succeeded", '{}', "result", None, json.dumps({"hash": "r-" + deal_id}), index, index))
        db.commit()
        db.close()

    def test_all_sql_rows_saved_before_fetch_and_other_requester_chain_exports(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            data, output = root / "data", root / "output"
            data.mkdir(); output.mkdir()
            self.make_db(data)
            def get(url):
                self.assertTrue(all((output / "deals" / (name + ".json")).is_file() for name in ("canary", "alice")))
                return {"document": {"hash": url.rsplit("/", 1)[1]}}
            with patch.object(replay.helpers, "get_json", side_effect=get), patch.object(replay.helpers, "provider_record", side_effect=AssertionError("A2A must not be needed")):
                index = replay.export_deals(SimpleNamespace(data=data, url="http://127.0.0.1:1"), output)
            self.assertEqual({item["requester_id"] for item in index}, {"other-requester", "alice-requester"})
            self.assertTrue(all(item["export_status"]["full_signed_chain_exported"] for item in index))
            self.assertTrue(all(replay.sha(item["path"]) == item["sha256"] for item in index))

    def test_root_failure_preserves_each_sql_record_and_continues(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            data, output = root / "data", root / "output"
            data.mkdir(); output.mkdir()
            self.make_db(data)
            calls = 0
            def get(url):
                nonlocal calls
                calls += 1
                if calls == 1:
                    raise RuntimeError("Fixture unavailable root")
                return {"document": {"hash": url.rsplit("/", 1)[1]}}
            with patch.object(replay.helpers, "get_json", side_effect=get):
                index = replay.export_deals(SimpleNamespace(data=data, url="http://127.0.0.1:1"), output)
            self.assertFalse(index[0]["export_status"]["full_signed_chain_exported"])
            self.assertTrue(index[1]["export_status"]["full_signed_chain_exported"])
            failed = replay.load_json(index[0]["path"])
            self.assertIn("spec", failed)
            self.assertIn("export_error", failed)
            self.assertEqual(len(replay.load_json(output / "deal-index.json")), 2)


if __name__ == "__main__":
    unittest.main()
