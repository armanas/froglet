# Reviewed setup for optional A2A counterparties

The native HTTP invitation flow already issues expiring, revocable admission
credentials. Use it directly when A2A is unnecessary. An optional A2A
counterparty additionally needs a separate bearer bound to the recipient's
signing key and exact Offer hashes. The source helper below prepares those
settings without manually editing credential JSON:

```sh
python3 scripts/setup_a2a_counterparty.py --request /absolute/bob-request.json --plan
python3 scripts/setup_a2a_counterparty.py --request /absolute/bob-request.json \
  --approved-plan-sha256 SHA_FROM_THE_REVIEWED_PLAN
```

This is operator configuration, not a public invitation link or a new protocol.
The plan is read-only and redacted. Apply requires its exact SHA-256 and refuses
changed requests, changed input files, reused destinations, symbolic links and
public credential files. The helper publishes a new private directory containing
a merged configuration; the previous configuration and the running Node are
unchanged. Existing unrelated counterparties are preserved. Existing entries for
the same requester or provider origin require a separate rotation review rather
than being silently replaced or widened.

Known operator/runtime token filenames and credential values from the current
operator environment/configured token files are refused as invitation material.
Run the helper in the same operator environment as the Node so those paths refer
to the correct installation. These checks prevent common credential mix-ups;
they do not prove that an arbitrary supplied token is an issued invitation.

Python 3 and POSIX file permissions/locks are required. This source helper is
not included in the single-binary release assets. The destination parent must
already exist, be owned by the operator and not be writable by other users.
Use absolute paths with symlink-free parents (for example `/private/tmp/...`
rather than macOS's `/tmp` alias).

## Bob prepares the provider settings

First start Bob's provider with `FROGLET_PROVIDER_ACCESS_MODE=invite` and reviewed
finite quote, deal and runtime allowances. Publish the intended services using
Bob's operator credential. Obtain Alice's signing public key from her requester
Node and the active, signed Offer hashes from Bob's provider catalog.

Issue the admission invitation using the existing authenticated operator flow:

```sh
froglet-node safeguards invite-create --name Alice --expires-at UNIX_SECONDS \
  --max-requests 20 --token-file /absolute/issued-alice.token --json
```

Create a private mode-0600 request file. These identifiers and limits are
placeholders to replace after inspection; no credential text goes in the request:

```json
{
  "kind": "provider",
  "destination": "/absolute/new-bob-a2a-settings",
  "provider_id": "BOB_64_LOWERCASE_HEX_PUBLIC_KEY",
  "provider_url": "https://bob.example",
  "requester_id": "ALICE_64_LOWERCASE_HEX_PUBLIC_KEY",
  "offer_hashes": ["EXACT_ACTIVE_64_LOWERCASE_HEX_OFFER_HASH"],
  "admission_token_file": "/absolute/issued-alice.token",
  "allowances": {
    "max_total_quotes": 20,
    "max_total_deals": 10,
    "max_total_runtime_ms": 20000
  }
}
```

Optionally set `existing_config` to Bob's existing private A2A JSON file to
preserve its unrelated entries. Review the key, origin, Offer scope and cumulative
limits in `--plan`, then apply precisely that plan's fingerprint. A changed Offer
requires another explicit setup review. The cumulative limits apply across Bob's
provider database; preparing another directory or restarting does not reset use.

The new directory contains:

| File | Purpose |
| --- | --- |
| `a2a.json` | Private merged A2A client allowlist |
| `recipient-handoff.json` | Private recipient-only A2A and admission credentials, with identity/scope references |
| `activate.sh` | Reviewed A2A path, invite-mode policy and finite cumulative allowances |
| `setup-receipt.json` | Redacted review plan and input fingerprints |

Directories use mode 0700 and files use mode 0600. Source `activate.sh` into
Bob's operator Node environment **before restarting that Node**, or transfer the
same settings to its service manager. Preserve Bob's data directory, identity,
public origin and existing operator/runtime authentication. This fragment does
not configure ingress, wallets, upstream credentials or process supervision.

Deliver only `recipient-handoff.json` to Alice through a trusted private channel.
Never upload it to a catalog, put it in a share URL, print it, or paste it into an
agent conversation. The helper does not check whether the supplied admission
token was actually issued, is unexpired, or has remaining use; the provider's
existing admission ledger checks those facts when Alice requests work.

## Alice imports the private handoff

Alice independently checks Bob's public key and her requester Node's public key.
Her private mode-0600 request contains:

```json
{
  "kind": "requester",
  "destination": "/absolute/new-alice-a2a-settings",
  "handoff_file": "/absolute/received-private-handoff.json",
  "expected_provider_id": "BOB_64_LOWERCASE_HEX_PUBLIC_KEY",
  "expected_requester_id": "ALICE_64_LOWERCASE_HEX_PUBLIC_KEY"
}
```

Optionally set `existing_config` to preserve Alice's other configured providers.
Run the same `--plan` and `--approved-plan-sha256` commands using Alice's request.
The helper refuses a handoff with different identity references, and writes a
private requester `a2a.json`, a separate `access.token`, an `activate.sh` fragment
and a redacted receipt. Source the fragment into Alice's Node environment before
restarting the requester Node. Keep her existing signing identity and data.

For native `invoke_service` or `run_compute`, pass the new absolute
`access_token_file` path along with the service/input or compiled Wasm program,
Bob's public identity and URL, an explicit price ceiling and a stable retry key.
MCP receives the path; the local runtime forwards the admission credential
privately. The optional A2A bearer comes from Node configuration, not tool
arguments. Specifying identity strings in the setup request does not prove key
ownership; a caller with another signing key is still refused by Bob.

## What this changes and what it does not

| Scenario | Result |
| --- | --- |
| Alice has both valid credentials, her signing key and an approved active Offer | New bounded work can pass the existing admission checks, subject to quota and payment rules |
| Alice supplies a different signing key or selects an unapproved Offer | The A2A scope refuses access |
| The admission invitation expires, is revoked or runs out | New work is refused even while the A2A credential remains configured |
| Someone reads a normal public service link or Agent Card | They obtain a reference, not credentials or execution authority |
| Bob edits files but does not restart | The current Node retains its previously loaded A2A configuration |
| Bob rotates/removes an A2A entry and restarts | That static transport credential loses access, including A2A task reads; keep the old entry during recovery if required |
| Bob revokes only the admission invitation | Existing authenticated completed-task recovery remains available under the configured A2A scope |

The admission invitation is provider-wide. It does not become service-specific
because it travels beside a scoped A2A token: a holder may use the ordinary HTTP
invocation path for other services on the same provider. Use separate providers
when the recipient must never access another service. This helper neither fixes
that existing boundary nor claims universal A2A interoperability.

Remote use requires the existing HTTPS/public-address checks. For an isolated
qualification run only, both sides can explicitly approve `allow_loopback: true`
with a literal `http://127.0.0.1:PORT` or `http://[::1]:PORT` origin. This does not
permit arbitrary private-network destinations or turn loopback into a public
share endpoint. **Invitation forwarding still requires HTTPS**, including local
tests: permitting A2A over loopback HTTP does not permit sending an admission
credential over it.

The real process qualification uses a temporary loopback HTTPS proxy and a
temporary CA trusted only by its Node and Python processes. It does not alter the
OS trust store or deploy a public endpoint:

```sh
python3 -W error examples/a2a_counterparty_demo.py --no-build \
  --output /absolute/counterparty-qualification.json
```

Build `froglet-node` and the `ontology-check` Wasm example first, or omit
`--no-build` to build them. This qualification additionally requires OpenSSL. It
checks an invited requester-supplied computation, wrong requester/Offer refusal,
missing invitation refusal, exact retry, cumulative quota exhaustion, immediate
invitation revocation, completed recovery without the invitation file and
provider restart recovery. The output contains signed evidence and public
references, with credentials withheld. Independent signing/receipt verification
establishes the transaction evidence; these synthetic terminology labels do not
establish scientific truth or a publicly deployed service.
