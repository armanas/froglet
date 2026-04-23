#!/usr/bin/env bash
# Release-candidate gate for the public Froglet repo.
#
# One entrypoint that runs the current release-gate line items in sequence,
# captures each step's stdout+stderr to a per-step log file under an evidence
# directory, and prints a pass/fail summary. A candidate release is PASS if no
# step is FAIL.
#
# See docs/RELEASE.md "Release Candidate Gate" for the mapping between these
# steps and the release-gate rows.
set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

run_compose=0
run_lnd=0
run_tor=0
run_package=0
run_install_smoke=0
evidence_dir=""
package_version=""
package_platform=""
package_arch=""
declare -a skip_ids=()

usage() {
  cat <<'EOF'
Usage: scripts/release_gate.sh [options]

Runs the current release-candidate gate for this repo. Each step writes its
output to a log file under the evidence directory, and the summary table lists
every step with status and log path.

Options:
  --compose                       Run the compose-backed OpenClaw+MCP smoke
                                  (sets FROGLET_RUN_COMPOSE_SMOKE=1 inside
                                  strict_checks.sh). Requires docker.
  --lnd-regtest                   Run the LND regtest integration inside
                                  strict_checks.sh.
  --tor                           Run the Tor integration inside
                                  strict_checks.sh.
  --package-assets                Run release-asset packaging and verification.
                                  Requires --version, --platform, --arch.
  --install-smoke                 Run the installer-path smoke. Implies
                                  --package-assets and requires the packaged
                                  target to match the current host platform
                                  and architecture.
  --version <tag>                 Release version for packaging + install smoke
                                  (e.g. v0.1.0-alpha.1).
  --platform <linux|darwin>       Packaging target platform.
  --arch <x86_64|arm64>           Packaging target architecture.
  --evidence-dir <path>           Override the evidence directory. Default is
                                  _tmp/release_gate/<UTC-timestamp>/.
  --skip <id>                     Skip a step by id (repeatable). See the
                                  STEPS section below for valid ids.
  -h, --help                      Show this help and exit.

STEPS
  secrets         Publication secret scan (scripts/gitleaks_gate.sh).
  strict          Repo strict checks (scripts/strict_checks.sh).
  docs-build      Docs-site build (npm --prefix docs-site run build).
  docs-test       Docs-site unit tests (npm --prefix docs-site test).
  package         Release asset packaging + verification (opt-in).
  install-smoke   Installer-path smoke from packaged assets (opt-in).
EXIT CODES
  0  All selected steps are PASS or SKIP.
  1  At least one step FAILed.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --compose)        run_compose=1; shift ;;
    --lnd-regtest)    run_lnd=1; shift ;;
    --tor)            run_tor=1; shift ;;
    --package-assets) run_package=1; shift ;;
    --install-smoke)  run_install_smoke=1; run_package=1; shift ;;
    --version)        package_version="$2"; shift 2 ;;
    --platform)       package_platform="$2"; shift 2 ;;
    --arch)           package_arch="$2"; shift 2 ;;
    --evidence-dir)   evidence_dir="$2"; shift 2 ;;
    --skip)           skip_ids+=("$2"); shift 2 ;;
    -h|--help)        usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 1 ;;
  esac
done

if [[ $run_package == 1 && ( -z "$package_version" || -z "$package_platform" || -z "$package_arch" ) ]]; then
  echo "release_gate: --package-assets requires --version, --platform, --arch" >&2
  exit 1
fi

ts="$(date -u +%Y%m%dT%H%M%SZ)"
evidence_dir="${evidence_dir:-_tmp/release_gate/${ts}}"
mkdir -p "$evidence_dir"

# status for each step: "id|label|status|detail"
#   status: PASS | FAIL | SKIP | PENDING
#   detail: log-file path for PASS/FAIL, human-readable reason for SKIP/PENDING
declare -a results=()
any_fail=0

skipped() {
  local id="$1"
  for s in "${skip_ids[@]:-}"; do
    [[ "$s" == "$id" ]] && return 0
  done
  return 1
}

record() {
  # record <id> <status> <label> <detail>
  results+=("$1|$3|$2|$4")
  printf '[%s] %s — %s\n' "$2" "$1" "$3"
}

run_step() {
  # run_step <id> <label> <cmd...>
  local id="$1"
  local label="$2"
  shift 2
  local log="$evidence_dir/${id}.log"

  if skipped "$id"; then
    record "$id" "SKIP" "$label" "skipped via --skip ${id}"
    return 0
  fi

  printf '\n[run]  %s — %s\n' "$id" "$label"
  printf '       log: %s\n' "$log"

  local rc=0
  ( "$@" ) >"$log" 2>&1 || rc=$?

  if [[ $rc -eq 0 ]]; then
    record "$id" "PASS" "$label" "$log"
  else
    any_fail=1
    record "$id" "FAIL" "$label" "$log (rc=${rc})"
  fi
  return 0
}

# --- Step 1: publication secret scan -----------------------------------------
run_step "secrets" "Publication secret scan (tracked tree + visible history)" \
  ./scripts/gitleaks_gate.sh --evidence-dir "$evidence_dir/gitleaks"

# --- Step 2: strict checks ---------------------------------------------------
run_step "strict" "Repo strict checks (cargo + python + node)" \
  env \
    FROGLET_SKIP_GITLEAKS=1 \
    FROGLET_RUN_COMPOSE_SMOKE="$run_compose" \
    FROGLET_RUN_LND_REGTEST="$run_lnd" \
    FROGLET_RUN_TOR_INTEGRATION="$run_tor" \
  ./scripts/strict_checks.sh

# --- Step 3: docs-site build -------------------------------------------------
if command -v npm >/dev/null 2>&1; then
  run_step "docs-build" "Docs-site build (astro)" \
    npm --prefix docs-site run build

  run_step "docs-test" "Docs-site tests (vitest)" \
    npm --prefix docs-site test
else
  record "docs-build" "SKIP" "Docs-site build (astro)" "npm not installed"
  record "docs-test"  "SKIP" "Docs-site tests (vitest)" "npm not installed"
fi

# --- Step 4: release-asset packaging + verification (opt-in) -----------------
if [[ $run_package == 1 ]]; then
  assets_dir="$evidence_dir/release-assets"
  mkdir -p "$assets_dir"
  run_step "package" "Release asset packaging + verification" \
    bash -c "
      set -euo pipefail
      scripts/package_release_assets.sh \\
        --version '$package_version' \\
        --platform '$package_platform' \\
        --arch '$package_arch' \\
        --out-dir '$assets_dir'
      asset_name='froglet-node-${package_version}-${package_platform}-${package_arch}.tar.gz'
      if command -v sha256sum >/dev/null 2>&1; then
        (cd '$assets_dir' && sha256sum \"\$asset_name\" > SHA256SUMS)
      elif command -v shasum >/dev/null 2>&1; then
        (cd '$assets_dir' && shasum -a 256 \"\$asset_name\" > SHA256SUMS)
      else
        echo 'missing required checksum tool: sha256sum or shasum' >&2
        exit 1
      fi
      scripts/verify_release_assets.sh \\
        --dir '$assets_dir' \\
        --version '$package_version' \\
        --target '$package_platform:$package_arch'
    "
else
  record "package" "SKIP" "Release asset packaging" \
    "not requested (pass --package-assets)"
fi

# --- Step 5: installer-path smoke (opt-in, depends on package) ---------------
if [[ $run_install_smoke == 1 ]]; then
  run_step "install-smoke" "Installer-path smoke from packaged assets" \
    scripts/smoke_install_from_assets.sh \
      --assets-dir "$evidence_dir/release-assets" \
      --version "$package_version"
else
  record "install-smoke" "SKIP" "Installer-path smoke" \
    "not requested (pass --install-smoke)"
fi

# --- Summary -----------------------------------------------------------------
summary_file="$evidence_dir/summary.tsv"
printf 'id\tstatus\tlabel\tdetail\n' >"$summary_file"
for row in "${results[@]}"; do
  IFS='|' read -r id label status detail <<<"$row"
  printf '%s\t%s\t%s\t%s\n' "$id" "$status" "$label" "$detail" >>"$summary_file"
done

printf '\n============================================================\n'
printf 'Release gate summary (evidence: %s)\n' "$evidence_dir"
printf '============================================================\n'
printf '%-14s %-8s %s\n' "ID" "STATUS" "DETAIL"
printf '%-14s %-8s %s\n' "--" "------" "------"
for row in "${results[@]}"; do
  IFS='|' read -r id label status detail <<<"$row"
  printf '%-14s %-8s %s\n' "$id" "$status" "$detail"
done
printf '\nSummary file: %s\n' "$summary_file"

# --- Exit --------------------------------------------------------------------
if [[ $any_fail -ne 0 ]]; then
  printf 'Result: FAIL\n' >&2
  exit 1
fi
printf 'Result: PASS\n'
exit 0
