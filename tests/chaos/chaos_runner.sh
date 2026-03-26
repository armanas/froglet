#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Chaos testing — injects failures into the Docker Compose stack and
# verifies resilience and recovery.
#
# Prerequisites: running compose stack (docker compose up -d --wait)
# Usage:        ./tests/chaos/chaos_runner.sh [scenario ...]
#               With no arguments, runs all scenarios.
# ---------------------------------------------------------------------------
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

# Color helpers
if [[ -z "${NO_COLOR:-}" ]] && [[ -t 1 ]]; then
  RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'
  BLUE='\033[0;34m'; BOLD='\033[1m'; RESET='\033[0m'
else
  RED='' GREEN='' YELLOW='' BLUE='' BOLD='' RESET=''
fi

FAILURES=()

banner() { echo -e "\n${BLUE}${BOLD}--- chaos: $1 ---${RESET}"; }
pass()   { echo -e "  ${GREEN}[pass]${RESET} $1"; }
fail()   { echo -e "  ${RED}[FAIL]${RESET} $1"; FAILURES+=("$1"); }

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
wait_healthy() {
  local port=$1 label=$2 retries=${3:-20}
  for i in $(seq 1 "$retries"); do
    if curl -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  return 1
}

assert_healthy() {
  local port=$1 label=$2
  if wait_healthy "$port" "$label" 5; then
    pass "$label healthy"
  else
    fail "$label NOT healthy"
  fi
}

assert_unhealthy() {
  local port=$1 label=$2
  if ! curl -fsS "http://127.0.0.1:$port/health" >/dev/null 2>&1; then
    pass "$label correctly unreachable"
  else
    fail "$label still healthy (expected down)"
  fi
}

# ---------------------------------------------------------------------------
# Scenario: kill_provider
# ---------------------------------------------------------------------------
chaos_kill_provider() {
  banner "kill_provider"

  # Verify provider is up
  assert_healthy 8080 "provider (pre-kill)"

  # Kill the provider container
  docker compose kill provider 2>/dev/null || true
  sleep 2

  # Provider should be unreachable
  assert_unhealthy 8080 "provider (post-kill)"

  # Discovery and runtime should still be up
  assert_healthy 9090 "discovery (during provider down)"
  assert_healthy 8081 "runtime (during provider down)"

  # Restart provider
  docker compose start provider
  if wait_healthy 8080 "provider" 30; then
    pass "provider recovered after restart"
  else
    fail "provider did not recover after restart"
  fi
}

# ---------------------------------------------------------------------------
# Scenario: kill_runtime
# ---------------------------------------------------------------------------
chaos_kill_runtime() {
  banner "kill_runtime"

  assert_healthy 8081 "runtime (pre-kill)"

  docker compose kill runtime 2>/dev/null || true
  sleep 2

  assert_unhealthy 8081 "runtime (post-kill)"
  assert_healthy 8080 "provider (during runtime down)"
  assert_healthy 9090 "discovery (during runtime down)"

  docker compose start runtime
  if wait_healthy 8081 "runtime" 30; then
    pass "runtime recovered after restart"
  else
    fail "runtime did not recover after restart"
  fi
}

# ---------------------------------------------------------------------------
# Scenario: kill_discovery
# ---------------------------------------------------------------------------
chaos_kill_discovery() {
  banner "kill_discovery"

  assert_healthy 9090 "discovery (pre-kill)"

  docker compose kill discovery 2>/dev/null || true
  sleep 2

  assert_unhealthy 9090 "discovery (post-kill)"

  # Provider and runtime should still serve requests
  assert_healthy 8080 "provider (during discovery down)"
  assert_healthy 8081 "runtime (during discovery down)"

  docker compose start discovery
  if wait_healthy 9090 "discovery" 30; then
    pass "discovery recovered after restart"
  else
    fail "discovery did not recover after restart"
  fi
}

# ---------------------------------------------------------------------------
# Scenario: restart_all
# ---------------------------------------------------------------------------
chaos_restart_all() {
  banner "restart_all"

  docker compose restart
  sleep 5

  local all_ok=true
  for port_label in "9090:discovery" "8080:provider" "8081:runtime" "9191:operator"; do
    local port="${port_label%%:*}"
    local label="${port_label##*:}"
    if wait_healthy "$port" "$label" 30; then
      pass "$label recovered after full restart"
    else
      fail "$label did not recover after full restart"
      all_ok=false
    fi
  done

  if $all_ok; then
    # Verify data persistence — query should still work
    local status
    status=$(curl -s -o /dev/null -w "%{http_code}" \
      -X POST http://127.0.0.1:8080/v1/node/events/query \
      -H "Content-Type: application/json" \
      -d '{"kinds":["chaos.test"],"limit":1}')
    if [[ "$status" == "200" ]]; then
      pass "query endpoint responsive after full restart"
    else
      fail "query endpoint returned $status after full restart"
    fi
  fi
}

# ---------------------------------------------------------------------------
# Scenario: network_partition (provider ↔ discovery)
# ---------------------------------------------------------------------------
chaos_network_partition() {
  banner "network_partition"

  # Get the compose network name
  local network
  network=$(docker compose config --format json 2>/dev/null | python3 -c "
import json, sys
config = json.load(sys.stdin)
nets = config.get('networks', {})
for name in nets:
    print(name)
    break
" 2>/dev/null || echo "default")

  local full_network="${PWD##*/}_${network}"

  # Disconnect provider from the network (simulates partition)
  local provider_container
  provider_container=$(docker compose ps -q provider 2>/dev/null || true)

  if [[ -z "$provider_container" ]]; then
    fail "could not find provider container"
    return
  fi

  docker network disconnect "$full_network" "$provider_container" 2>/dev/null || {
    # Try with the compose project name prefix
    local project
    project=$(docker compose config --format json 2>/dev/null | python3 -c "
import json, sys; print(json.load(sys.stdin).get('name', 'froglet'))" 2>/dev/null || echo "froglet")
    full_network="${project}_${network}"
    docker network disconnect "$full_network" "$provider_container" 2>/dev/null || {
      echo -e "  ${YELLOW}[skip]${RESET} could not disconnect provider from network"
      return
    }
  }

  sleep 2

  # Discovery should still be up
  assert_healthy 9090 "discovery (during partition)"

  # Reconnect
  docker network connect "$full_network" "$provider_container" 2>/dev/null || true
  sleep 3

  if wait_healthy 8080 "provider" 15; then
    pass "provider recovered after network reconnect"
  else
    fail "provider did not recover after network reconnect"
  fi
}

# ---------------------------------------------------------------------------
# Scenario: rapid_restarts
# ---------------------------------------------------------------------------
chaos_rapid_restarts() {
  banner "rapid_restarts"

  for i in 1 2 3; do
    docker compose restart provider --timeout 2
    sleep 2
  done

  if wait_healthy 8080 "provider" 30; then
    pass "provider stable after 3 rapid restarts"
  else
    fail "provider unstable after rapid restarts"
  fi
}

# ---------------------------------------------------------------------------
# Dispatcher
# ---------------------------------------------------------------------------
ALL_SCENARIOS=(kill_provider kill_runtime kill_discovery restart_all network_partition rapid_restarts)

run_scenario() {
  local name="$1"
  case "$name" in
    kill_provider|kill_runtime|kill_discovery|restart_all|network_partition|rapid_restarts)
      "chaos_$name"
      ;;
    *)
      echo -e "${RED}Unknown chaos scenario: $name${RESET}" >&2
      exit 1
      ;;
  esac
}

# Verify compose stack is running
if ! docker compose ps --status running 2>/dev/null | grep -q "running"; then
  echo -e "${RED}ERROR: Docker Compose stack must be running.${RESET}" >&2
  echo "Start it with: docker compose up -d --wait" >&2
  exit 1
fi

SCENARIOS=("$@")
if [[ ${#SCENARIOS[@]} -eq 0 ]]; then
  SCENARIOS=("${ALL_SCENARIOS[@]}")
fi

for scenario in "${SCENARIOS[@]}"; do
  run_scenario "$scenario"
done

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
if [[ ${#FAILURES[@]} -gt 0 ]]; then
  echo -e "${RED}${BOLD}CHAOS FAILURES: ${FAILURES[*]}${RESET}"
  exit 1
else
  echo -e "${GREEN}${BOLD}All chaos scenarios passed.${RESET}"
  exit 0
fi
