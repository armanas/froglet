#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

provider_addr="${FROGLET_GPU_SMOKE_PROVIDER_ADDR:-127.0.0.1:18080}"
runtime_addr="${FROGLET_GPU_SMOKE_RUNTIME_ADDR:-127.0.0.1:18081}"
registry_port="${FROGLET_GPU_SMOKE_REGISTRY_PORT:-15000}"
expected_gpu="${FROGLET_GPU_SMOKE_EXPECTED_GPU:-}"
work_root="${FROGLET_GPU_SMOKE_ROOT:-$(mktemp -d "${TMPDIR:-/tmp}/froglet-gpu-smoke-XXXXXX")}"
data_root="$work_root/node-data"
image_dir="$work_root/image"
wrapper_dir="$work_root/docker-wrapper"
evidence_dir="$work_root/evidence"
registry_name="froglet-gpu-smoke-registry"
docker_bin="${DOCKER_BIN:-}"
node_pid=""

cleanup() {
  if [[ -n "$node_pid" ]]; then
    kill "$node_pid" >/dev/null 2>&1 || true
    wait "$node_pid" >/dev/null 2>&1 || true
  fi
  "$docker_bin" rm -f "$registry_name" >/dev/null 2>&1 || true
}
trap cleanup EXIT

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "missing required command: $1" >&2
    exit 1
  fi
}

wait_http() {
  local url="$1"
  for _ in $(seq 1 120); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.25
  done
  return 1
}

jcs_sha256() {
  python3 - "$1" <<'PY'
import hashlib
import json
import sys

value = json.loads(sys.argv[1])
encoded = json.dumps(value, sort_keys=True, separators=(",", ":")).encode()
print(hashlib.sha256(encoded).hexdigest())
PY
}

need cargo
need curl
need jq
need python3
if [[ -z "$docker_bin" ]]; then
  docker_bin="$(command -v docker || true)"
fi
test -n "$docker_bin" || { echo "missing required command: docker" >&2; exit 1; }

mkdir -p "$image_dir" "$wrapper_dir" "$evidence_dir"

echo "[gpu-smoke] work root: $work_root"
echo "[gpu-smoke] verifying Docker GPU access"
"$docker_bin" run --rm --gpus all nvidia/cuda:12.9.1-base-ubuntu22.04 \
  nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader \
  | tee "$evidence_dir/docker-nvidia-smi.txt"

observed_gpu="$(cut -d, -f1 < "$evidence_dir/docker-nvidia-smi.txt" | head -1 | xargs)"
if [[ -n "$expected_gpu" && "$observed_gpu" != "$expected_gpu" ]]; then
  echo "expected GPU '$expected_gpu', observed '$observed_gpu'" >&2
  exit 1
fi

echo "[gpu-smoke] starting local registry on port $registry_port"
"$docker_bin" rm -f "$registry_name" >/dev/null 2>&1 || true
"$docker_bin" run -d --name "$registry_name" -p "$registry_port:5000" registry:2 >/dev/null

cat > "$image_dir/Dockerfile" <<'DOCKER'
FROM nvidia/cuda:12.9.1-base-ubuntu22.04
RUN apt-get update \
  && apt-get install -y --no-install-recommends python3 \
  && rm -rf /var/lib/apt/lists/*
COPY gpu_probe.py /usr/local/bin/gpu_probe.py
ENTRYPOINT ["python3", "/usr/local/bin/gpu_probe.py"]
DOCKER

cat > "$image_dir/gpu_probe.py" <<'PY'
import json
import os
import subprocess
import sys
import time

payload = json.load(sys.stdin)
query = [
    "nvidia-smi",
    "--query-gpu=name,memory.total,driver_version",
    "--format=csv,noheader,nounits",
]
try:
    raw = subprocess.check_output(query, text=True).strip().splitlines()[0]
    name, memory, driver = [part.strip() for part in raw.split(",")]
    gpu = {
        "visible": True,
        "name": name,
        "memory_mb": int(memory),
        "driver_version": driver,
    }
except Exception as exc:
    gpu = {"visible": False, "error": str(exc)}

print(json.dumps({
    "ok": gpu.get("visible") is True,
    "gpu": gpu,
    "input": payload,
    "froglet_gpu_capabilities": json.loads(os.environ.get("FROGLET_GPU_CAPABILITIES", "[]")),
    "context": json.loads(os.environ.get("FROGLET_CONTEXT", "{}")),
    "oci_digest_env": os.environ.get("FROGLET_OCI_DIGEST"),
    "observed_at_ms": int(time.time() * 1000),
}, sort_keys=True, separators=(",", ":")))
PY

image_repo="localhost:${registry_port}/froglet-gpu-smoke"
echo "[gpu-smoke] building and pushing probe image $image_repo"
"$docker_bin" build -t "$image_repo:latest" "$image_dir" > "$evidence_dir/docker-build.log"
"$docker_bin" push "$image_repo:latest" > "$evidence_dir/docker-push.log"
image_digest="$("$docker_bin" image inspect "$image_repo:latest" --format '{{index .RepoDigests 0}}' | sed 's/^.*@//; s/^sha256://')"
test -n "$image_digest"
printf '%s\n' "$image_digest" > "$evidence_dir/image-digest.txt"

echo "[gpu-smoke] building froglet-node"
cargo build --release -p froglet --bin froglet-node

cat > "$wrapper_dir/docker" <<SH
#!/usr/bin/env bash
printf '%s argv=' "\$(date -u +%FT%TZ)" >> "$evidence_dir/docker-wrapper.log"
for arg in "\$@"; do printf ' [%s]' "\$arg" >> "$evidence_dir/docker-wrapper.log"; done
printf ' env_FROGLET_CONTEXT=%s env_FROGLET_GPU_CAPABILITIES=%s env_FROGLET_OCI_DIGEST=%s\n' "\${FROGLET_CONTEXT-<unset>}" "\${FROGLET_GPU_CAPABILITIES-<unset>}" "\${FROGLET_OCI_DIGEST-<unset>}" >> "$evidence_dir/docker-wrapper.log"
exec "$docker_bin" "\$@"
SH
chmod +x "$wrapper_dir/docker"
: > "$evidence_dir/docker-wrapper.log"

echo "[gpu-smoke] starting froglet-node with GPU enabled"
PATH="$wrapper_dir:$PATH" \
RUST_LOG=info \
FROGLET_DATA_ROOT="$data_root" \
FROGLET_LISTEN_ADDR="$provider_addr" \
FROGLET_RUNTIME_LISTEN_ADDR="$runtime_addr" \
FROGLET_PUBLIC_BASE_URL="http://$provider_addr" \
FROGLET_RUNTIME_PROVIDER_BASE_URL="http://$provider_addr" \
FROGLET_EXECUTION_TIMEOUT_SECS="${FROGLET_GPU_SMOKE_TIMEOUT_SECS:-180}" \
FROGLET_GPU_ENABLED=1 \
FROGLET_GPU_COUNT="${FROGLET_GPU_COUNT:-1}" \
FROGLET_GPU_VENDOR="${FROGLET_GPU_VENDOR:-nvidia}" \
FROGLET_GPU_MODEL="${FROGLET_GPU_MODEL:-$observed_gpu}" \
FROGLET_GPU_MEMORY_MB="${FROGLET_GPU_MEMORY_MB:-15360}" \
FROGLET_GPU_CONTAINER_RUNTIME=docker \
./target/release/froglet-node > "$evidence_dir/node.log" 2>&1 &
node_pid="$!"

wait_http "http://$provider_addr/v1/node/capabilities" || {
  tail -200 "$evidence_dir/node.log" >&2 || true
  exit 1
}

curl -fsS "http://$provider_addr/v1/node/capabilities" > "$evidence_dir/capabilities.json"
curl -fsS "http://$provider_addr/v1/provider/offers" > "$evidence_dir/offers.json"

provider_id="$(jq -r '.identity.node_id' "$evidence_dir/capabilities.json")"
token="$(cat "$data_root/runtime/auth.token")"
input='{"probe":"froglet-gpu-smoke"}'
input_hash="$(jcs_sha256 "$input")"

cat > "$evidence_dir/deal-body.json" <<JSON
{"provider":{"provider_id":"$provider_id","provider_url":"http://$provider_addr"},"offer_id":"execute.compute.generic","kind":"execution","execution":{"schema_version":"froglet/v1","workload_kind":"compute.execution.v1","runtime":"container","package_kind":"oci_image","entrypoint":{"kind":"handler","value":"run"},"contract_version":"froglet.container.stdin_json.v1","input_format":"application/json+jcs","input_hash":"$input_hash","requested_access":["compute.gpu"],"security":{"mode":"standard"},"mounts":[],"input":{"probe":"froglet-gpu-smoke"},"module_hash":"$image_digest","oci_reference":"$image_repo","oci_digest":"$image_digest"}}
JSON

echo "[gpu-smoke] creating GPU deal"
curl -fsS \
  -H "Authorization: Bearer $token" \
  -H "content-type: application/json" \
  --data @"$evidence_dir/deal-body.json" \
  "http://$runtime_addr/v1/runtime/deals" > "$evidence_dir/deal-create.json"

deal_id="$(jq -r '.deal.deal_id' "$evidence_dir/deal-create.json")"
for _ in $(seq 1 180); do
  curl -fsS \
    -H "Authorization: Bearer $token" \
    "http://$runtime_addr/v1/runtime/deals/$deal_id" > "$evidence_dir/deal-result.json"
  deal_status="$(jq -r '.deal.status' "$evidence_dir/deal-result.json")"
  case "$deal_status" in
    succeeded) break ;;
    failed|rejected) cat "$evidence_dir/deal-result.json" >&2; exit 1 ;;
  esac
  sleep 0.5
done

jq -e \
  --arg observed_gpu "$observed_gpu" \
  '.deal.status == "succeeded"
   and .deal.result.gpu.visible == true
   and .deal.result.gpu.name == $observed_gpu
   and .deal.result.froglet_gpu_capabilities == ["compute.gpu"]
   and (.deal.receipt != null)' \
  "$evidence_dir/deal-result.json" >/dev/null
grep -q -- "--gpus" "$evidence_dir/docker-wrapper.log"
grep -q -- 'env_FROGLET_GPU_CAPABILITIES=\["compute.gpu"\]' "$evidence_dir/docker-wrapper.log"
jq -e \
  '.offers[] | select(.payload.offer_id == "execute.compute.generic")
   | .payload.execution_profile.capabilities | index("compute.gpu")' \
  "$evidence_dir/offers.json" >/dev/null

jq -n \
  --arg work_root "$work_root" \
  --arg deal_id "$deal_id" \
  --arg gpu "$observed_gpu" \
  --arg image_digest "$image_digest" \
  '{status:"succeeded", work_root:$work_root, deal_id:$deal_id, gpu:$gpu, image_digest:$image_digest}' \
  | tee "$evidence_dir/summary.json"

echo "[gpu-smoke] evidence: $evidence_dir"
