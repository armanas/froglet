"""Shared test helpers: fixture loading only. Not a test module itself (no
`test_*` prefix, so `unittest discover` will not collect it)."""

from __future__ import annotations

import copy
import json
from pathlib import Path
from typing import Any

# python/froglet-verify/tests/_helpers.py -> parents[3] is the repo root.
REPO_ROOT = Path(__file__).resolve().parents[3]
CONFORMANCE_DIR = REPO_ROOT / "conformance"
FIXTURE_PATH = CONFORMANCE_DIR / "kernel_v1.json"
X402_FIXTURE_PATH = CONFORMANCE_DIR / "x402_v1.json"

_fixture_cache: dict[str, Any] | None = None
_x402_fixture_cache: dict[str, Any] | None = None


def load_fixture() -> dict[str, Any]:
    """Load conformance/kernel_v1.json, cached (read-only use expected --
    callers that mutate must ``copy.deepcopy`` first, e.g. via
    :func:`deep_copy_artifact`)."""
    global _fixture_cache
    if _fixture_cache is None:
        _fixture_cache = json.loads(FIXTURE_PATH.read_text(encoding="utf-8"))
    return _fixture_cache


def artifact(name: str) -> dict[str, Any]:
    """A deep copy of ``fixture["artifacts"][name]["artifact"]``, safe to
    mutate for tampering tests without affecting other tests."""
    return copy.deepcopy(load_fixture()["artifacts"][name]["artifact"])


def load_x402_fixture() -> dict[str, Any]:
    """Load conformance/x402_v1.json, cached. See :func:`load_fixture`."""
    global _x402_fixture_cache
    if _x402_fixture_cache is None:
        _x402_fixture_cache = json.loads(X402_FIXTURE_PATH.read_text(encoding="utf-8"))
    return _x402_fixture_cache


def x402_artifact(name: str) -> dict[str, Any]:
    """A deep copy of ``x402_fixture["artifacts"][name]["artifact"]``."""
    return copy.deepcopy(load_x402_fixture()["artifacts"][name]["artifact"])


def x402_verification_case(name: str) -> dict[str, Any]:
    """A deep copy of one ``artifact_verification_cases`` entry from
    conformance/x402_v1.json, looked up by its ``name`` field."""
    for case in load_x402_fixture()["artifact_verification_cases"]:
        if case["name"] == name:
            return copy.deepcopy(case)
    raise KeyError(f"no x402_v1.json artifact_verification_cases entry named {name!r}")


def deep_copy(value: Any) -> Any:
    return copy.deepcopy(value)
