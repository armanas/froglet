#!/usr/bin/env bash
# Thin wrapper over the Cloudflare v4 DNS API for the froglet.dev zone.
#
# The token is read from the macOS Keychain (`security find-generic-password
# -a froglet -s cloudflare-dns-token -w`) and never echoed.
#
# The zone is resolved at runtime from $FROGLET_DNS_ZONE (default: froglet.dev)
# so this helper works unchanged if we ever point it at a staging zone.
#
# Subcommands:
#   verify                             — /user/tokens/verify
#   zone                               — resolve zone id + print zone status
#   list [name]                        — list all records, or records matching <name>
#   create <type> <name> <content> [ttl] [proxied]
#                                      — create a record. ttl default 300, proxied default false.
#   delete <id>                        — delete by record id
#   upsert <type> <name> <content> [ttl] [proxied]
#                                      — replace (create if absent) the single record matching
#                                        <type>+<name>. Errors if more than one matches.
#
# Record names accept short forms; "@" and the bare zone name both mean apex.

set -euo pipefail

ZONE_NAME="${FROGLET_DNS_ZONE:-froglet.dev}"
TOKEN_ACCOUNT="${FROGLET_DNS_TOKEN_ACCOUNT:-froglet}"
TOKEN_SERVICE="${FROGLET_DNS_TOKEN_SERVICE:-cloudflare-dns-token}"
API="https://api.cloudflare.com/client/v4"

die() { echo "cloudflare_dns: $*" >&2; exit 1; }

read_token() {
  security find-generic-password -a "$TOKEN_ACCOUNT" -s "$TOKEN_SERVICE" -w 2>/dev/null \
    || die "token not found in keychain (account=$TOKEN_ACCOUNT service=$TOKEN_SERVICE)"
}

api() {
  local method="$1" path="$2"
  shift 2
  local token
  token="$(read_token)"
  curl -sS -X "$method" \
    -H "Authorization: Bearer $token" \
    -H "Content-Type: application/json" \
    "${API}${path}" \
    "$@"
}

resolve_zone_id() {
  api GET "/zones?name=${ZONE_NAME}" | python3 -c "$(cat <<'PY'
import json, sys
d = json.load(sys.stdin)
if not d.get("success"):
    print(f"zone lookup failed: {d.get('errors')}", file=sys.stderr); sys.exit(1)
zones = d.get("result", [])
if not zones:
    print(f"no zone found: name={sys.argv[1]}", file=sys.stderr); sys.exit(1)
print(zones[0]["id"])
PY
)" "$ZONE_NAME"
}

fqdn() {
  local name="$1"
  case "$name" in
    @|"$ZONE_NAME") echo "$ZONE_NAME" ;;
    *."$ZONE_NAME") echo "$name" ;;
    *) echo "${name}.${ZONE_NAME}" ;;
  esac
}

cmd_verify() {
  api GET "/user/tokens/verify" | python3 -c "$(cat <<'PY'
import json, sys
d = json.load(sys.stdin)
if not d.get("success"):
    print(f"verify failed: {d.get('errors')}"); sys.exit(1)
r = d.get("result", {})
print(f"status: {r.get('status')}")
print(f"id: {r.get('id')}")
print(f"expires_on: {r.get('expires_on')}")
for m in d.get("messages", []):
    print(f"message: {m.get('message')}")
PY
)"
}

cmd_zone() {
  local id
  id="$(resolve_zone_id)"
  api GET "/zones/${id}" | python3 -c "$(cat <<'PY'
import json, sys
d = json.load(sys.stdin)
if not d.get("success"):
    print(f"zone lookup failed: {d.get('errors')}"); sys.exit(1)
z = d["result"]
print(f"zone_id: {z['id']}")
print(f"name: {z['name']}")
print(f"status: {z['status']}")
print(f"plan: {z['plan']['name']}")
print(f"nameservers: {z['name_servers']}")
print(f"activated_on: {z.get('activated_on')}")
PY
)"
}

cmd_list() {
  local id filter
  id="$(resolve_zone_id)"
  filter=""
  if [ "${1:-}" != "" ]; then
    local full; full="$(fqdn "$1")"
    filter="?name=${full}"
  fi
  api GET "/zones/${id}/dns_records${filter}" | python3 -c "$(cat <<'PY'
import json, sys
d = json.load(sys.stdin)
if not d.get("success"):
    print(f"list failed: {d.get('errors')}"); sys.exit(1)
for r in d.get("result", []):
    proxied = " [proxied]" if r.get("proxied") else ""
    rid = r["id"]
    rtype = r["type"]
    name = r["name"]
    ttl = r.get("ttl")
    content = str(r.get("content", ""))[:100]
    print(f"{rid}  {rtype:6} {name:40} ttl={ttl!s:>5}  {content}{proxied}")
PY
)"
}

cmd_create() {
  [ $# -ge 3 ] || die "usage: create <type> <name> <content> [ttl] [proxied]"
  local type="$1" name="$2" content="$3" ttl="${4:-300}" proxied="${5:-false}"
  local id full
  id="$(resolve_zone_id)"
  full="$(fqdn "$name")"
  local body
  body="$(python3 - "$type" "$full" "$content" "$ttl" "$proxied" <<'PY'
import json, sys
t, n, c, ttl, proxied = sys.argv[1:]
print(json.dumps({"type": t, "name": n, "content": c, "ttl": int(ttl), "proxied": proxied == "true"}))
PY
)"
  api POST "/zones/${id}/dns_records" --data "$body" | python3 -c "$(cat <<'PY'
import json, sys
d = json.load(sys.stdin)
if not d.get("success"):
    print(f"create failed: {d.get('errors')}"); sys.exit(1)
r = d["result"]
print(f"created  {r['id']}  {r['type']} {r['name']} → {r['content']}")
PY
)"
}

cmd_delete() {
  [ $# -ge 1 ] || die "usage: delete <id>"
  local id
  id="$(resolve_zone_id)"
  api DELETE "/zones/${id}/dns_records/${1}" | python3 -c "$(cat <<'PY'
import json, sys
d = json.load(sys.stdin)
if not d.get("success"):
    print(f"delete failed: {d.get('errors')}"); sys.exit(1)
print(f"deleted  {d['result']['id']}")
PY
)"
}

cmd_upsert() {
  [ $# -ge 3 ] || die "usage: upsert <type> <name> <content> [ttl] [proxied]"
  local type="$1" name="$2" content="$3" ttl="${4:-300}" proxied="${5:-false}"
  local id full
  id="$(resolve_zone_id)"
  full="$(fqdn "$name")"
  local existing
  existing="$(api GET "/zones/${id}/dns_records?type=${type}&name=${full}" | python3 -c "$(cat <<'PY'
import json, sys
d = json.load(sys.stdin)
if not d.get("success"):
    print(f"lookup failed: {d.get('errors')}", file=sys.stderr); sys.exit(1)
rs = d.get("result", [])
if len(rs) > 1:
    print(f"ambiguous: {len(rs)} records match", file=sys.stderr); sys.exit(2)
if rs:
    print(rs[0]["id"])
PY
)"
)"
  local body
  body="$(python3 - "$type" "$full" "$content" "$ttl" "$proxied" <<'PY'
import json, sys
t, n, c, ttl, proxied = sys.argv[1:]
print(json.dumps({"type": t, "name": n, "content": c, "ttl": int(ttl), "proxied": proxied == "true"}))
PY
)"
  if [ -z "$existing" ]; then
    api POST "/zones/${id}/dns_records" --data "$body"
  else
    api PUT "/zones/${id}/dns_records/${existing}" --data "$body"
  fi | python3 -c "$(cat <<'PY'
import json, sys
d = json.load(sys.stdin)
if not d.get("success"):
    print(f"upsert failed: {d.get('errors')}"); sys.exit(1)
r = d["result"]
print(f"upserted  {r['id']}  {r['type']} {r['name']} → {r['content']}")
PY
)"
}

help_text() {
  sed -n '3,21p' "$0" | sed 's/^# \{0,1\}//'
}

cmd="${1:-}"
if [ -z "$cmd" ]; then help_text; exit 1; fi
shift || true

case "$cmd" in
  verify) cmd_verify "$@" ;;
  zone)   cmd_zone "$@" ;;
  list)   cmd_list "$@" ;;
  create) cmd_create "$@" ;;
  delete) cmd_delete "$@" ;;
  upsert) cmd_upsert "$@" ;;
  -h|--help|help) help_text ;;
  *) die "unknown subcommand: $cmd" ;;
esac
