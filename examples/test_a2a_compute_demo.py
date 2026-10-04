"""Focused independent-oracle checks; real transport checks live in the demo."""
import copy
import unittest

from examples.a2a_compute_demo import (
    PRIVATE_SENTINEL, mapping_oracle, ontology_cases, ontology_source, verified_report,
)


class NativeReportDiagnosticsTests(unittest.TestCase):
    def test_unexpected_signed_timeout_names_status_code_and_budget(self):
        # Same boundary as a successful-case assertion receiving a terminal
        # execution_timed_out Receipt during the cold ontology computation.
        response = {"isError": True, "structuredContent": {
            "status": "failed", "stage": "requester_execution",
            "error": "PRIVATE_ERROR_VALUE", "result": "PRIVATE_RESULT_VALUE",
            "execution_limits": {"max_runtime_ms": 2000},
            "receipt_verification": {"verified": True,
                "failure_code": "execution_timed_out"}}}
        with self.assertRaises(RuntimeError) as raised:
            verified_report(response, "succeeded")
        message = str(raised.exception)
        for expected in ("expected=succeeded", "status=failed", "stage=requester_execution",
                         "failure_code=execution_timed_out", "max_runtime_ms=2000"):
            self.assertIn(expected, message)
        self.assertNotIn("PRIVATE_ERROR_VALUE", message)
        self.assertNotIn("PRIVATE_RESULT_VALUE", message)

    def test_pending_response_is_distinguished_from_terminal_failure(self):
        with self.assertRaises(RuntimeError) as raised:
            verified_report({"isError": False, "structuredContent": {
                "status": "pending", "receipt_verification": {"verified": False}}}, "succeeded")
        message = str(raised.exception)
        self.assertIn("status=pending", message)
        self.assertIn("receipt_verified=False", message)
        self.assertNotIn("failure_code=execution_timed_out", message)

    def test_missing_or_nonobject_structured_report_has_clear_error(self):
        for response in ({}, {"structuredContent": []}, {"structuredContent": None}):
            with self.subTest(response=response), self.assertRaisesRegex(
                    RuntimeError, "Native MCP response lacks a structured execution report"):
                verified_report(response, "succeeded")


class OntologyOracleTests(unittest.TestCase):
    def test_exact_strings_and_identical_duplicates(self):
        result = mapping_oracle({"mappings": [
            {"source": "term", "target": "DEMO:target"},
            {"source": "term", "target": "DEMO:target"}],
            "observed_terms": ["term", "term", "Term", " term", "term "]})
        self.assertEqual(result, {"mappings_consistent": True, "conflicts": [],
                                  "unmapped_terms": [" term", "Term", "term "], "mapped_terms": 1})

    def test_global_conflict_retains_all_targets_and_excludes_ambiguous_observation(self):
        result = mapping_oracle({"mappings": [
            {"source": "unused", "target": "DEMO:z"},
            {"source": "unused", "target": "DEMO:a"},
            {"source": "seen", "target": "DEMO:a"},
            {"source": "seen", "target": "DEMO:b"}], "observed_terms": ["seen", "absent"]})
        self.assertEqual(result, {"mappings_consistent": False, "conflicts": [
            {"source": "seen", "targets": ["DEMO:a", "DEMO:b"]},
            {"source": "unused", "targets": ["DEMO:a", "DEMO:z"]}],
            "unmapped_terms": ["absent"], "mapped_terms": 0})

    def test_unmapped_observation_does_not_make_mapping_table_inconsistent(self):
        self.assertEqual(mapping_oracle({"mappings": [], "observed_terms": ["missing", "missing"]}),
                         {"mappings_consistent": True, "conflicts": [],
                          "unmapped_terms": ["missing"], "mapped_terms": 0})

    def test_invalid_schema_is_refused(self):
        for request in [[], {}, {"mappings": [], "observed_terms": [], "extra": True},
                        {"mappings": [["term", "target"]], "observed_terms": []},
                        {"mappings": [{"source": "term", "target": ""}], "observed_terms": []},
                        {"mappings": [{"source": "term", "target": "target", "private": "x"}],
                         "observed_terms": []}, {"mappings": [], "observed_terms": [None]}]:
            with self.subTest(request=request), self.assertRaises(RuntimeError):
                mapping_oracle(request)

    def test_expected_four_example_results(self):
        rows = [{key: row[key] for key in ("source", "target")}
                for row in ontology_source()["terminology"]]
        actual = {case["name"]: mapping_oracle(case["input"]) for case in ontology_cases(rows)}
        conflict = [{"source": "DEMO:sample.blood", "targets": [
            "DEMO:specimen.plasma", "DEMO:specimen.whole_blood"]}]
        self.assertEqual(actual["consistent"], {"mappings_consistent": True, "conflicts": [],
                                               "unmapped_terms": [], "mapped_terms": 3})
        self.assertEqual(actual["conflicting_targets"], {"mappings_consistent": False,
            "conflicts": conflict, "unmapped_terms": [], "mapped_terms": 2})
        self.assertEqual(actual["unmapped_term"], {"mappings_consistent": True, "conflicts": [],
            "unmapped_terms": ["DEMO:sample.saliva"], "mapped_terms": 3})
        self.assertEqual(actual["conflict_and_unmapped"], {"mappings_consistent": False,
            "conflicts": conflict, "unmapped_terms": ["DEMO:sample.saliva"], "mapped_terms": 2})

    def test_cases_use_returned_rows_and_do_not_mutate_them(self):
        # Distinct labels prove this is derived data, not the in-process source fixture.
        returned = [{"source": "DEMO:return.a", "target": "DEMO:return.one"},
                    {"source": "DEMO:return.a", "target": "DEMO:return.two"},
                    {"source": "DEMO:return.b", "target": "DEMO:return.three"}]
        original = copy.deepcopy(returned)
        cases = ontology_cases(returned)
        self.assertEqual(returned, original)
        combined = cases[-1]
        self.assertEqual(combined["input"]["mappings"], returned)
        self.assertEqual(mapping_oracle(combined["input"]), {"mappings_consistent": False,
            "conflicts": [{"source": "DEMO:return.a", "targets": ["DEMO:return.one", "DEMO:return.two"]}],
            "unmapped_terms": ["DEMO:sample.saliva"], "mapped_terms": 1})
        cases[0]["input"]["mappings"][0]["target"] = "DEMO:changed"
        self.assertEqual(returned, original)

    def test_altered_input_changes_oracle(self):
        request = {"mappings": [{"source": "DEMO:a", "target": "DEMO:b"}],
                   "observed_terms": ["DEMO:a"]}
        expected = mapping_oracle(request)
        changed = copy.deepcopy(request)
        changed["mappings"].append({"source": "DEMO:a", "target": "DEMO:c"})
        self.assertNotEqual(mapping_oracle(changed), expected)
        changed = copy.deepcopy(request)
        changed["observed_terms"].append("DEMO:missing")
        self.assertNotEqual(mapping_oracle(changed), expected)

    def test_source_is_synthetic_and_private_column_is_explicit(self):
        for row in ontology_source()["terminology"]:
            self.assertTrue(row["source"].startswith("DEMO:"))
            self.assertTrue(row["target"].startswith("DEMO:"))
            self.assertEqual(row["curator_note"], PRIVATE_SENTINEL)


if __name__ == "__main__":
    unittest.main()
