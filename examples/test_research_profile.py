"""Consumer refusal tests for the optional research service annotation."""
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from examples.research_profile import demo_profile, demo_requirements, preflight, validate_profile, validate_rows


class ResearchProfileTests(unittest.TestCase):
    def test_explicit_demo_requirements_match_without_scientific_claim(self):
        result = preflight(demo_profile(), demo_requirements())
        self.assertTrue(result["compatible"])
        self.assertFalse(result["scientific_truth_verified"])

    def test_namespace_version_unit_and_mapping_policy_differences_are_explained(self):
        for path, value in [
            (("collections", "terminology", "fields", "source", "identifier", "namespace"), "urn:other"),
            (("collections", "terminology", "fields", "source", "identifier", "version"), "2"),
            (("collections", "terminology", "fields", "source", "unit"), "mg/L"),
            (("mapping_assumptions", "cardinality"), "many_targets_per_source"),
            (("provenance", "version"), "new-release")]:
            with self.subTest(path=path):
                requirements = demo_requirements()
                node = requirements
                for key in path[:-1]:
                    node = node[key]
                node[path[-1]] = value
                result = preflight(demo_profile(), requirements)
                self.assertFalse(result["compatible"])
                self.assertEqual(len(result["mismatches"]), 1)
                self.assertEqual(result["mismatches"][0]["path"], "research_profile." + ".".join(path))
                self.assertEqual(result["mismatches"][0]["expected"], value)

    def test_missing_profile_and_empty_requirements_fail_closed(self):
        for profile, requirements in [(None, demo_requirements()), (demo_profile(), {}),
                (demo_profile(), {"schema_version": demo_profile()["schema_version"]}),
                (demo_profile(), dict(demo_requirements(), unknown=True))]:
            with self.subTest(profile=profile, requirements=requirements), self.assertRaises(ValueError):
                preflight(profile, requirements)

    def test_missing_keys_cannot_satisfy_null_or_unknown_requirements(self):
        version = demo_profile()["schema_version"]
        for requirement in [
            {"schema_version": version, "collections": {"missing_collection": None}},
            {"schema_version": version, "collections": {"terminology": {"fields": {"excluded_private_field": None}}}},
            {"schema_version": version, "provenance": {"not_a_profile_key": None}},
            {"schema_version": version, "provenance": {"version": None}},
            {"schema_version": version, "collections": {"terminology": {"fields": {"source": {"nullable": None}}}}},
        ]:
            with self.subTest(requirement=requirement), self.assertRaises(ValueError):
                preflight(demo_profile(), requirement)
        missing = {"schema_version": version, "collections": {
            "terminology": {"fields": {"excluded_private_field": {"type": "string"}}}}}
        report = preflight(demo_profile(), missing)
        self.assertFalse(report["compatible"])
        self.assertEqual(report["mismatches"][0]["reason"], "Required declaration is absent")
        self.assertTrue(report["mismatches"][0]["path"].endswith("excluded_private_field"))

    def test_every_returned_row_type_prefix_and_extra_fields_checked(self):
        rows = [{"source": "DEMO:sample.plasma", "target": "DEMO:specimen.plasma"}]
        self.assertEqual(validate_rows(demo_profile(), "terminology", rows), rows)
        for altered in [{"source": "DEMO:sample.plasma", "target": "OTHER:plasma"},
                        {"source": None, "target": "DEMO:specimen.plasma"},
                        {"source": "DEMO:sample.plasma", "target": "DEMO:specimen.plasma", "private": "secret"}]:
            with self.subTest(altered=altered), self.assertRaises(ValueError):
                validate_rows(demo_profile(), "terminology", rows + [altered])

    def test_bool_is_not_an_integer_and_nonfinite_numbers_are_refused(self):
        profile = demo_profile()
        fields = profile["collections"]["terminology"]["fields"]
        fields["source"] = {"type": "integer", "nullable": False, "unit": "1"}
        profile.pop("mapping_assumptions")
        with self.assertRaises(ValueError):
            validate_rows(profile, "terminology", [{"source": True, "target": "DEMO:specimen.plasma"}])
        fields["source"]["type"] = "number"
        with self.assertRaises(ValueError):
            validate_rows(profile, "terminology", [{"source": float("inf"), "target": "DEMO:specimen.plasma"}])

    def test_invalid_profiles_are_refused(self):
        for change in [lambda p: p.update(schema_version="future"),
                       lambda p: p["provenance"].update(private_path="/private"),
                       lambda p: p["mapping_assumptions"].update(target_field="missing"),
                       lambda p: p["collections"]["terminology"]["fields"]["source"].update(nullable="false"),
                       lambda p: p["provenance"].update(version="\nsecret")]:
            profile = copy.deepcopy(demo_profile())
            change(profile)
            with self.assertRaises(ValueError):
                validate_profile(profile)

    def test_preflight_does_not_mutate_profile_or_requirements(self):
        profile, requirements = demo_profile(), demo_requirements()
        before = copy.deepcopy((profile, requirements))
        preflight(profile, requirements)
        self.assertEqual((profile, requirements), before)

    def test_malformed_inspection_cli_returns_invalid_json_with_exit_two(self):
        with tempfile.TemporaryDirectory() as root:
            requirements = Path(root) / "requirements.json"
            requirements.write_text(json.dumps(demo_requirements()))
            inspection = Path(root) / "inspection.json"
            for value in [[], {"verification": []},
                          {"verification": {"status": "verified"}, "output_schema": []}]:
                inspection.write_text(json.dumps(value))
                result = subprocess.run([sys.executable, "-W", "error", "-m", "examples.research_profile",
                    "--inspection", str(inspection), "--requirements", str(requirements)],
                    capture_output=True, text=True, check=False)
                with self.subTest(value=value):
                    self.assertEqual(result.returncode, 2)
                    self.assertEqual(json.loads(result.stdout)["status"], "invalid")
                    self.assertEqual(result.stderr, "")


if __name__ == "__main__":
    unittest.main()
