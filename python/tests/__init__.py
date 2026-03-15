import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
PYTHON_ROOT = REPO_ROOT / "python"
TESTS_ROOT = PYTHON_ROOT / "tests"

for path in (PYTHON_ROOT, TESTS_ROOT, REPO_ROOT):
    text = str(path)
    if text not in sys.path:
        sys.path.insert(0, text)
