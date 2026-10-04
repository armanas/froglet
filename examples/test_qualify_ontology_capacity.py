"""Meaningful denominator, concurrency and response-binding checks for qualification."""
import argparse
import threading
import time
import unittest

from examples.qualify_ontology_capacity import (
    concurrency_list, percentile_nearest_rank, response_summary, run_phase, summarize,
    worker_index_from_key,
    failed_invocation_task,
    verified_failed_task,
)


class CapacityQualificationTests(unittest.TestCase):
    def test_successful_retrieval_of_a_failed_task_does_not_mark_the_tool_as_failed(self):
        response = {"isError": False, "structuredContent": {"status": "failed",
            "deal_id": "task", "deal_hash": "task",
            "receipt_verification": {"verified": True, "receipt_hash": "signed-failure"}}}
        self.assertEqual(verified_failed_task(response, "task")["status"], "failed")
        with self.assertRaises(RuntimeError):
            verified_failed_task(response, "different")
        response["structuredContent"]["receipt_verification"]["verified"] = False
        with self.assertRaises(RuntimeError):
            verified_failed_task(response, "task")
    def test_failure_task_parser_only_accepts_actual_terminal_invocation_shape(self):
        task = "a" * 64
        self.assertEqual(failed_invocation_task(
            f'invocation {task} ended in status "failed": process execution concurrency limit exhausted'), task)
        for error in ("daemon: HTTP409", f'invocation {task} ended in status "running": slow',
                      f'invocation {task[:-1]} ended in status "failed": refused'):
            self.assertIsNone(failed_invocation_task(error))
    def test_worker_identity_comes_from_the_actual_scheduled_worker(self):
        self.assertEqual(worker_index_from_key("froglet_selected_data_and_compute-c32-w17-i1"), 17)
    def test_nearest_rank_uses_tail_and_rejects_invalid_fraction(self):
        self.assertEqual(percentile_nearest_rank([40, 10, 30, 20], 0.50), 20)
        self.assertEqual(percentile_nearest_rank([40, 10, 30, 20], 0.95), 40)
        for values, fraction in (([], 0.5), ([1], 0), ([1], 1.1)):
            with self.assertRaises(RuntimeError):
                percentile_nearest_rank(values, fraction)

    def test_summary_keeps_failures_in_latency_and_attempt_denominator(self):
        result = summarize([{"status": "succeeded", "latency_ms": 10},
                            {"status": "error", "latency_ms": 90}], 2, 2, 2)
        self.assertEqual((result["attempted"], result["succeeded"], result["errors"]), (2, 1, 1))
        self.assertEqual(result["successful_workflows_per_second"], 0.5)
        self.assertEqual(result["latency_ms"]["p95"], 90)

    def test_concurrency_parser_rejects_accidental_duplicate_or_excessive_load(self):
        self.assertEqual(concurrency_list("1,2,8,32"), [1, 2, 8, 32])
        for value in ("", "2,1", "1,1", "0", "129", "one"):
            with self.subTest(value=value), self.assertRaises(argparse.ArgumentTypeError):
                concurrency_list(value)

    def test_phase_runs_exactly_finite_unique_work_and_records_transport_error(self):
        seen = set()
        lock = threading.Lock()

        def operation(key):
            with lock:
                self.assertNotIn(key, seen)
                seen.add(key)
            time.sleep(0.005)
            if key.endswith("w0-i0"):
                raise OSError("deliberate transport refusal")
            return {"status": "succeeded"}

        phase = run_phase(operation, 3, 2, "test")
        self.assertEqual((phase["attempted"], phase["succeeded"], phase["errors"]), (6, 5, 1))
        self.assertEqual(phase["peak_active_workflows"], 3)
        self.assertEqual(len(seen), 6)
        failed = [sample for sample in phase["samples"] if sample["status"] == "error"]
        self.assertEqual(failed[0]["error_type"], "OSError")
        self.assertGreater(failed[0]["latency_ms"], 0)

    def test_success_must_verify_receipt_and_match_oracle(self):
        response = {"isError": False, "structuredContent": {"status": "succeeded",
            "deal_id": "same", "deal_hash": "same", "result_hash": "result",
            "result": {"mapped_terms": 2},
            "receipt_verification": {"verified": True, "receipt_hash": "receipt"}}}
        self.assertTrue(response_summary(response, {"mapped_terms": 2})["receipt_verified"])
        with self.assertRaises(RuntimeError):
            response_summary(response, {"mapped_terms": 3})
        response["structuredContent"]["receipt_verification"]["verified"] = False
        with self.assertRaises(RuntimeError):
            response_summary(response, {"mapped_terms": 2})

    def test_actual_refusal_remains_an_error(self):
        summary = response_summary({"isError": True, "structuredContent": {
            "error": "provider execution allowance exhausted"}}, {})
        self.assertEqual(summary["status"], "error")
        self.assertIn("allowance exhausted", summary["error"])


if __name__ == "__main__":
    unittest.main()
