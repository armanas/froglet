#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

readonly GITLEAKS_IMAGE="ghcr.io/gitleaks/gitleaks:v8.30.0@sha256:691af3c7c5a48b16f187ce3446d5f194838f91238f27270ed36eef6359a574d9"
readonly DEFAULT_VISIBLE_REFS=(
  "origin/main"
  "refs/tags/v0.1.0-alpha.0"
  "refs/tags/v0.1.0-alpha.1"
  "refs/tags/v0.1.0-alpha.2"
)

evidence_dir=""

usage() {
  cat <<'EOF'
Usage: scripts/gitleaks_gate.sh [--evidence-dir PATH]

Runs the publication secret-scan gate in two passes:
  1. current tracked tree (excludes untracked local-only files)
  2. GitHub-visible history (origin/main + current public alpha tags by default)

Evidence is written under _tmp/gitleaks_gate/<UTC-timestamp>/ unless
--evidence-dir overrides it.

Override the history scope with:
  FROGLET_GITLEAKS_VISIBLE_REFS="origin/main refs/tags/v0.1.0-alpha.0 ..."
EOF
}

die() {
  echo "gitleaks_gate: $*" >&2
  exit 1
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --evidence-dir)
      [[ $# -ge 2 ]] || die "--evidence-dir requires a value"
      evidence_dir="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

ts="$(date -u +%Y%m%dT%H%M%SZ)"
evidence_dir="${evidence_dir:-_tmp/gitleaks_gate/${ts}}"
mkdir -p "$evidence_dir"

config_path=".gitleaks.toml"
[[ -f "$config_path" ]] || die "missing config: $config_path"

if [[ -n "${FROGLET_GITLEAKS_VISIBLE_REFS:-}" ]]; then
  # shellcheck disable=SC2206
  visible_refs=(${FROGLET_GITLEAKS_VISIBLE_REFS})
else
  visible_refs=("${DEFAULT_VISIBLE_REFS[@]}")
fi

for ref in "${visible_refs[@]}"; do
  git rev-parse --verify --quiet "$ref" >/dev/null \
    || die "missing git ref required for GitHub-visible history scan: $ref"
done

printf '%s\n' "${visible_refs[@]}" > "${evidence_dir}/visible-refs.txt"

run_gitleaks() {
  if command -v gitleaks >/dev/null 2>&1; then
    gitleaks "$@"
    return
  fi

  if command -v docker >/dev/null 2>&1; then
    docker run --rm \
      -v "$repo_root:/repo" \
      -w /repo \
      "$GITLEAKS_IMAGE" \
      "$@"
    return
  fi

  die "neither gitleaks nor docker is available"
}

make_tracked_snapshot() {
  local snapshot_dir="$1"
  mkdir -p "$snapshot_dir"
  while IFS= read -r -d '' path; do
    local src="$repo_root/$path"
    local dest="$snapshot_dir/$path"
    if [[ -e "$src" || -L "$src" ]]; then
      mkdir -p "$(dirname "$dest")"
      cp -pR "$src" "$dest"
    fi
  done < <(git ls-files -z)
}

run_scan() {
  local id="$1"
  shift
  local log="${evidence_dir}/${id}.log"
  local report="${evidence_dir}/${id}.json"
  local rc=0

  if run_gitleaks "$@" --redact --report-format json --report-path "$report" >"$log" 2>&1; then
    echo "[PASS] ${id}"
    return 0
  fi

  rc=$?
  if [[ $rc -eq 1 ]]; then
    echo "[FAIL] ${id} (findings)" >&2
  else
    echo "[FAIL] ${id} (rc=${rc})" >&2
  fi
  return "$rc"
}

snapshot_dir="${evidence_dir}/tracked-tree"
make_tracked_snapshot "$snapshot_dir"

tree_rc=0
history_rc=0

run_scan "current-tree" dir "$snapshot_dir" || tree_rc=$?
run_scan "visible-history" git --log-opts="${visible_refs[*]}" . || history_rc=$?

summary="${evidence_dir}/summary.txt"
{
  echo "current-tree rc=${tree_rc}"
  echo "visible-history rc=${history_rc}"
  echo "visible-refs:"
  printf '  %s\n' "${visible_refs[@]}"
} >"$summary"

echo "gitleaks evidence: $evidence_dir"

if [[ $tree_rc -ne 0 || $history_rc -ne 0 ]]; then
  exit 1
fi
