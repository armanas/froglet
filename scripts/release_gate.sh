#!/usr/bin/env bash
# Release-candidate gate for the public Froglet repo.
#
# One entrypoint that runs the current release-gate line items in sequence,
# captures each step's stdout+stderr to a per-step log file under an evidence
# directory, and prints a software-check summary. PASS requires every selected
# step to pass; skipped required steps make the gate INCOMPLETE. This does not
# replace the separately documented hosted/client/distribution launch gates.
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
                                  Builds the current Cargo version for the
                                  native host; requires --version, --platform,
                                  --arch. Cross-target labels are rejected.
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
  0  All required and requested steps are PASS (software checks only).
  1  At least one step FAILed.
  2  No step FAILed, but a required or requested step was skipped.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version|--platform|--arch|--evidence-dir|--skip)
      if [[ $# -lt 2 || -z "$2" || "$2" == --* ]]; then
        echo "release_gate: $1 requires a value" >&2
        exit 1
      fi
      ;;
  esac
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

for id in "${skip_ids[@]:-}"; do
  [[ -z "$id" ]] && continue
  case "$id" in
    secrets|strict|docs-build|docs-test|package|install-smoke) ;;
    *) echo "release_gate: unknown step for --skip: $id" >&2; exit 1 ;;
  esac
done

if [[ $run_package == 1 && ( -z "$package_version" || -z "$package_platform" || -z "$package_arch" ) ]]; then
  echo "release_gate: --package-assets requires --version, --platform, --arch" >&2
  exit 1
fi

if [[ $run_package == 1 ]]; then
  [[ "$package_version" == v* ]] || package_version="v$package_version"
  if [[ ! "$package_version" =~ ^v[0-9A-Za-z][0-9A-Za-z.+-]*$ ]]; then
    echo "release_gate: invalid release version" >&2
    exit 1
  fi
  case "$package_platform:$package_arch" in
    linux:x86_64|linux:arm64|darwin:arm64) ;;
    *) echo "release_gate: unsupported packaging target" >&2; exit 1 ;;
  esac
  for required_tool in cargo rustc python3 uname; do
    command -v "$required_tool" >/dev/null 2>&1 || {
      echo "release_gate: packaging requires $required_tool" >&2
      exit 1
    }
  done
  case "$(uname -s):$(uname -m)" in
    Linux:x86_64) host_platform=linux; host_arch=x86_64 ;;
    Linux:aarch64|Linux:arm64) host_platform=linux; host_arch=arm64 ;;
    Darwin:arm64) host_platform=darwin; host_arch=arm64 ;;
    *) echo "release_gate: unsupported native packaging host" >&2; exit 1 ;;
  esac
  if [[ "$package_platform:$package_arch" != "$host_platform:$host_arch" ]]; then
    echo "release_gate: local packaging requires native target $host_platform:$host_arch; use native CI builds for other targets" >&2
    exit 1
  fi
  rustc_details="$(rustc -vV)" || exit 1
  host_target=""
  while IFS= read -r line; do
    [[ "$line" != host:\ * ]] || host_target="${line#host: }"
  done <<<"$rustc_details"
  case "$host_target:$host_platform:$host_arch" in
    x86_64-unknown-linux-gnu:linux:x86_64|x86_64-unknown-linux-musl:linux:x86_64|aarch64-unknown-linux-gnu:linux:arm64|aarch64-unknown-linux-musl:linux:arm64|aarch64-apple-darwin:darwin:arm64) ;;
    *) echo "release_gate: Rust compiler host does not match native packaging target" >&2; exit 1 ;;
  esac
  cargo_version="$(python3 - "$repo_root/Cargo.toml" <<'PY'
from pathlib import Path
import sys
try:
    import tomllib
except ModuleNotFoundError:
    import tomli as tomllib
print(tomllib.loads(Path(sys.argv[1]).read_text())["package"]["version"])
PY
  )" || exit 1
  if [[ "$package_version" != "v$cargo_version" ]]; then
    echo "release_gate: supplied version does not match Cargo package version v$cargo_version" >&2
    exit 1
  fi
  # Explicit Cargo arguments override CARGO_BUILD_TARGET/build.target and
  # CARGO_TARGET_DIR/build.target-dir. The archive receives only this output.
  package_target_dir="$repo_root/target"
  package_binary="$package_target_dir/$host_target/release/froglet-node"
fi

ts="$(date -u +%Y%m%dT%H%M%SZ)"
evidence_dir="${evidence_dir:-_tmp/release_gate/${ts}}"
mkdir -p "$evidence_dir" || exit 1

# status for each step: "id|label|status|detail"
#   status: PASS | FAIL | SKIP | PENDING
#   detail: log-file path for PASS/FAIL, human-readable reason for SKIP/PENDING
declare -a results=()
any_fail=0
any_incomplete=0
package_passed=0

skipped() {
  local id="$1"
  for s in "${skip_ids[@]:-}"; do
    [[ "$s" == "$id" ]] && return 0
  done
  return 1
}

record() {
  # record <id> <status> <label> <detail>
  if [[ "$1" == package && "$2" == PASS ]]; then
    package_passed=1
  fi
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
    any_incomplete=1
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
run_step "secrets" "Publication secret scan (public source + candidate/public history)" \
  ./scripts/gitleaks_gate.sh --evidence-dir "$evidence_dir/gitleaks"

# --- Step 2: native package, before qualifying that executable --------------
if [[ $run_package == 1 ]]; then
  assets_dir="$evidence_dir/release-assets"
  mkdir -p "$assets_dir" || exit 1
  run_step "package" "Native locked release build + asset verification" \
    bash -c '
      set -euo pipefail
      cargo build --locked --release --bin froglet-node -p froglet \
        --manifest-path "$5/Cargo.toml" --target "$6" --target-dir "$7"
      scripts/package_release_assets.sh \
        --version "$1" \
        --platform "$2" \
        --arch "$3" \
        --out-dir "$4" \
        --binary "$8"
      asset_name="froglet-node-${1}-${2}-${3}.tar.gz"
      if command -v sha256sum >/dev/null 2>&1; then
        (cd "$4" && sha256sum "$asset_name" agent-bootstrap.sh > SHA256SUMS)
      elif command -v shasum >/dev/null 2>&1; then
        (cd "$4" && shasum -a 256 "$asset_name" agent-bootstrap.sh > SHA256SUMS)
      else
        echo "missing required checksum tool: sha256sum or shasum" >&2
        exit 1
      fi
      scripts/verify_release_assets.sh \
        --dir "$4" \
        --version "$1" \
        --target "$2:$3"
    ' bash "$package_version" "$package_platform" "$package_arch" "$assets_dir" \
      "$repo_root" "$host_target" "$package_target_dir" "$package_binary"
else
  record "package" "SKIP" "Release asset packaging" \
    "not requested (pass --package-assets)"
fi

# Caller-supplied candidate paths cannot substitute for a failed/skipped build.
qualification_binary=""
declare -a docs_environment=(env)
if [[ $package_passed == 1 ]]; then
  qualification_binary="$package_binary"
  docs_environment+=("FROGLET_NODE_BIN=$package_binary")
elif [[ $run_package == 1 ]]; then
  docs_environment+=(-u FROGLET_NODE_BIN)
fi

# --- Step 3: strict checks ---------------------------------------------------
run_step "strict" "Repo strict checks (cargo + python + node)" \
  env \
    FROGLET_SKIP_GITLEAKS=1 \
    FROGLET_REQUIRE_SOFTWARE_CHECKS=1 \
    FROGLET_QUALIFICATION_NODE_BIN="$qualification_binary" \
    FROGLET_RUN_COMPOSE_SMOKE="$run_compose" \
    FROGLET_RUN_LND_REGTEST="$run_lnd" \
    FROGLET_RUN_TOR_INTEGRATION="$run_tor" \
  ./scripts/strict_checks.sh

# --- Step 4: docs-site build -------------------------------------------------
if command -v npm >/dev/null 2>&1; then
  run_step "docs-build" "Docs-site build (astro)" \
    "${docs_environment[@]}" npm --prefix docs-site run build

  run_step "docs-test" "Docs-site tests (vitest)" \
    "${docs_environment[@]}" npm --prefix docs-site test
else
  any_incomplete=1
  record "docs-build" "SKIP" "Docs-site build (astro)" "npm not installed"
  record "docs-test"  "SKIP" "Docs-site tests (vitest)" "npm not installed"
fi

# --- Step 5: installer-path smoke (opt-in, depends on package) ---------------
if [[ $run_install_smoke == 1 ]]; then
  if [[ $package_passed == 1 ]]; then
    run_step "install-smoke" "Installer-path smoke from packaged assets" \
      scripts/smoke_install_from_assets.sh \
        --assets-dir "$evidence_dir/release-assets" \
        --version "$package_version"
  else
    any_incomplete=1
    record "install-smoke" "PENDING" "Installer-path smoke from packaged assets" \
      "requires successful current-source package step"
  fi
else
  record "install-smoke" "SKIP" "Installer-path smoke" \
    "not requested (pass --install-smoke)"
fi

# --- Summary -----------------------------------------------------------------
summary_file="$evidence_dir/summary.tsv"
printf 'id\tstatus\tlabel\tdetail\n' >"$summary_file" || exit 1
for row in "${results[@]}"; do
  IFS='|' read -r id label status detail <<<"$row"
  printf '%s\t%s\t%s\t%s\n' "$id" "$status" "$label" "$detail" >>"$summary_file" || exit 1
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
if [[ $any_incomplete -ne 0 ]]; then
  printf 'Result: INCOMPLETE (required or requested checks skipped)\n'
  exit 2
fi
printf 'Result: PASS (software checks only; separate launch gates remain)\n'
exit 0
