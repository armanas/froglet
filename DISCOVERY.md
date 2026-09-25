# DISCOVERY — evidence-layer repositioning, Phase 0

Status: complete for the protocol repo; partial for `froglet-services` (read-only survey, no build run there).
Date: 2026-07-31. Baseline commit `aaeb375`, surveyed against the working tree (138 dirty files — see §5).

Findings are ordered by severity. Everything below was observed by reading source or running commands; inferences are labelled.

---

## 1. Findings

### F1 — The site claims a capability the code did not have (now fixed)

`docs-site/src/content/docs/learn/comparison.mdx:36-39` claims a third party can verify a receipt chain
"offline, with no API and no trust in any platform". Before this program there was **no offline verifier**:
the only verification surfaces were three HTTP POST endpoints on a running node, and
`docs-site/src/pages/verify-receipt.astro:39-43` — the page a reader would naturally reach for — explicitly
disclaims signature, hash, identity, and settlement verification.

Resolved by the `froglet-verify` crate and CLI (§3). The browser page still needs the WASM upgrade before
the claim is true *on the site*; until then the claim is substantiated only for CLI users.

### F2 — x402 could not produce a kernel-valid receipt (now fixed)

`PAYMENT_METHOD_X402_USDC` existed at `froglet-protocol/src/protocol/kernel.rs:31` as a descriptor
*advertisement* string only. There was no x402 settlement method: `validate_receipt_artifact` had branches
for exactly four methods and rejected everything else, so **an x402-settled deal could not produce a valid
receipt at all**. The driver at `src/settlement/x402.rs` returned a driver-internal `PaymentReceipt`, never a
signed artifact. This was a structural gap, not a coverage gap.

Resolved for the kernel half by `x402.eip3009.v1` (§3). The publish path and driver remain blocked on the
uncommitted WIP (§5).

### F3 — Homepage stated an unimplemented mechanism in the present tense (now fixed)

`docs-site/src/pages/index.astro:213` said "Cheating burns the stake", with game-theory moves asserting
"Stake slashed" and "reputation zeroed" — describing staking that
`docs-site/src/content/docs/learn/economics.mdx:95-100` labels "Status: designed, not live". The repo already
knew this was an overclaim: `demo-truth.test.ts:33` banned the exact string, but only for demo copy.

Fixed. The section is reframed around attributability — which is what the evidence chain actually provides —
and the stake threshold plot is labelled "Design preview / Not live". `positioning-truth.test.ts` now applies
the banned-string list to `index.astro` and `README.md`, so the gap that let this ship is closed, not just the
instance. Verified on the rendered page: zero banned strings present.

### F4 — Four divergent verification implementations across the two repos

- `packages/froglet-adapter` (froglet-services) — the intended seam, delegates correctly.
- `services/marketplace-node/src/verify.rs:8` — **hand-rolls envelope verification** from raw JSON with
  `.unwrap_or("")` / `.unwrap_or(0)` defaults, so a missing `schema_version` / `artifact_type` / `created_at`
  silently becomes empty-or-zero instead of an error. Highest-risk function in either repo for a signature change.
- `services/marketplace-api/src/registration.rs` — its own exact-revision evidence machinery.
- `docs-site/src/pages/verify-receipt.astro` — structural only (F1).

Plus two literally duplicated helpers (`ensure_signed_artifact_matches_document`, the feed-contract validator)
in `services/indexer/src/lib.rs` and `services/marketplace-api/src/registration.rs`.
**Not yet fixed** — consolidation onto `froglet-verify` is WS1.7, sequenced after the WIP lands.

### F5 — `chain.rs` had zero production callers

The five pairwise chain validators and their 27 stable issue codes were exercised **only by their own test
module**. Nothing in the node, publish engine, or MCP layer called them; `/v1/invoice-bundles/verify` used a
*second*, independently-written validator in `src/settlement/lightning.rs:944` with a non-matching issue-code
vocabulary. There was also no `validate_full_chain` — "verify the chain" meant composing five calls by hand.

Partly resolved: `validate_full_chain` now exists and `froglet-verify` is a real caller. The lightning.rs
duplicate still exists and should be folded into `chain.rs` (not yet scheduled).

### F6 — `extension_refs` is signed but unspecified

`extension_refs: Vec<String>` is present on Quote (`kernel.rs:220`), Deal (`:268`), and Receipt (`:358`),
is inside the signed bytes, and is **never validated and never mentioned in `docs/KERNEL.md`**. Same for
`authority_ref`, `acceptance_ref`. This is the additive seam foreign evidence (AP2 mandates, x402 proofs)
should ride on — but shipping signed-but-unspecified fields is itself a defect: two implementations can
disagree about them today with no conformance vector to arbitrate. **Not yet fixed** — WS3.

### F7 — Documentation inconsistencies in load-bearing copy

- `comparison.mdx:25` said "**Signed six-artifact chain**" then listed five (InvoiceBundle omitted).
  **Fixed**, with a test that recounts the list against the claimed number.
- `docs/KERNEL.md:1` says "the six artifact types" while `kernel.rs:13-15` also defines `curated_list`,
  `confidential_profile`, `confidential_session`.
- The brief that initiated this work asserted froglet.dev uses the word "explores" — **it does not**; a sweep
  of `README.md`, `docs-site/src/`, and `docs/` found no such hedge. The brief's own claim was unsubstantiated.

### F8 — Market claims in the initiating brief are partly unverifiable

The brief cites Google AP2's April 2026 FIDO donation with 60+ organisations, UnionPay APOP, and OKX APP.
x402, AP2, ACP, and Stripe/Tempo are verifiable; the others post-date or sit at the edge of what could be
checked here. `comparison.mdx:103` already carries a "current as of June 2026" dated-sources note — the right
pattern. Do not import unverified competitor claims into site copy.

---

## 2. Baseline test results (working tree, before any change)

| Suite | Command | Result |
|---|---|---|
| Rust workspace | `cargo test --workspace` | **pass**, exit 0 |
| docs-site | `npm --prefix docs-site test` | **13 files / 96 tests pass**, 1.87s |

Post-change re-run (see §3), all green:

| Suite | Result |
|---|---|
| Rust workspace, `RUSTFLAGS="-D warnings"` | **786 passing**, 0 failed |
| Clippy (`froglet-protocol`, `froglet-verify`) `-D warnings` | 0 warnings |
| `cargo fmt --all --check` | clean |
| `froglet-protocol` `--no-default-features --target wasm32-unknown-unknown` | builds |
| Python verifier package | **215 passing**; `mypy --strict` clean |
| Python conformance runner, both fixtures | **90/90 checks**, exit 0 |
| docs-site vitest | **111 passing** (was 96) |
| `conformance/kernel_v1.json` | byte-identical |

---

## 3. What changed in this phase

| Change | Files | Verified by |
|---|---|---|
| `froglet-protocol` feature-gated (`generate`/`manifest`/`publication`/`managed`); `rand`/`toml`/`url` optional; wasm32 shim for k256's getrandom link | `froglet-protocol/Cargo.toml`, `src/lib.rs`, `src/crypto.rs`, `src/protocol/kernel.rs` | `cargo check -p froglet-protocol --no-default-features --target wasm32-unknown-unknown` |
| Quote/Deal/InvoiceBundle semantic validators promoted from private (`chain.rs`) to public (`kernel.rs`); settlement-method strings lifted to public consts | `froglet-protocol/src/protocol/{kernel,chain}.rs` | 137 protocol tests |
| `verify_typed_document` extended to all six chain kinds; `VerifiedArtifact` gained Quote/Deal/InvoiceBundle | `kernel.rs` | 137 protocol tests |
| `validate_full_chain` + `FullChain`/`FullChainReport`; `warnings` added to `ChainValidationReport` | `chain.rs` | 3 new chain tests (paid ±bundle, free, cross-chain mismatch) |
| **`froglet-verify` crate**: envelope-first JSON API, standalone CLI, WASM binding surface | `froglet-verify/**` | 7 crate tests + live CLI runs |
| **`x402.eip3009.v1`** settlement method: receipt branch, quote terms, paid-method registry | `kernel.rs`, `chain.rs` | 5 x402 conformance tests, 6-case accept/reject table |
| **`conformance/x402_v1.json`** — new additive fixture + generator + runner | `tests/generate_x402_vectors.rs`, `tests/x402_conformance_vectors.rs` | byte-reproduction test |
| **`docs/SPEC.md`** — umbrella spec: verification algorithm, chain validation + 26-code issue registry, settlement-method registry (cryptographic vs attested per rail), conformance, non-goals, selective-disclosure deferral | `docs/SPEC.md`, `conformance/README.md` | `tests/spec_normative_rules.rs` (10 tests) + `tests/spec_coverage.rs` CI gate: every anchor must have a claiming test, and every claim must match a live anchor |
| **Python verifier package** — pure stdlib, zero deps, vendored BIP-340 + RFC 8785 | `python/froglet-verify/**` | 215 tests; `mypy --strict` clean; conformance runner 90/90 over both fixtures, exit 1 on a tampered fixture |
| **Site repositioning** — hero, non-goals section, game-theory reframe, README | `docs-site/src/pages/index.astro`, `README.md`, `docs-site/src/styles/index-page.css` | live browser verification + `positioning-truth.test.ts` |
| **Drift-proof maturity labels** — statuses render from a single evidence-backed data source | `docs-site/src/data/maturity.ts` | `maturity-data.test.ts`: every evidence path must exist on disk; no rail may claim production without a transcript |

`conformance/kernel_v1.json` is **byte-identical** (`git diff --exit-code` clean). `froglet-protocol` bumped
0.4.0 → 0.5.0 because `VerifiedArtifact` gained variants (source-breaking for exhaustive matchers).

Live proof that the offline claim now holds for the CLI:

```
$ froglet-verify conformance-chain.json
[ok ] descriptor     31f6f743b5c0  envelope + semantics verified
[ok ] offer          52c13488237a  envelope + semantics verified
[ok ] quote          8d5ab0fa8271  envelope + semantics verified
[ok ] deal           0b72debc6361  envelope + semantics verified
[ok ] receipt        12a529ade220  envelope + semantics verified
chain descriptor_offer: ok
chain offer_quote: ok
chain quote_deal: ok
chain quote_deal_receipt: ok
result: VALID
```

A tampered receipt exits 1 with `signature verification failed`.

---

## 4. Claims audit

Each row: the public claim, where it is made, and what substantiates it **now**.

| # | Claim | Location | Substantiation |
|---|---|---|---|
| 1 | Chain is verifiable "offline, with no API and no trust in any platform" | `comparison.mdx:36-39` | **Now substantiated for CLI/library**: `froglet-verify` + `froglet-verify/tests/conformance.rs`. **Not yet on the website** — verify-receipt page is still structural-only. |
| 2 | "Signed six-artifact chain … offline-verifiable by anyone" | `comparison.mdx:25` | Substantiated as above, but the sentence **lists five artifacts** (F7). Copy fix pending. |
| 3 | "Signed `froglet/v1` artifacts verify forever" | `docs/VERSIONING.md:9-12` | Substantiated: `conformance/kernel_v1.json` is frozen and re-verified byte-for-byte by `tests/kernel_conformance_vectors.rs` + this phase's changes left it untouched. |
| 4 | Vectors are the arbiter of conformance; 14 accept/reject + 5 invoice-bundle cases | `spec/conformance.md:6-28` | Substantiated and counted: 14 and 5 exactly. Now joined by 6 x402 cases in `x402_v1.json`. |
| 5 | "Tamper with any link and the chain breaks" | `deal-flow.mdx:15` | Substantiated: `chain.rs` link checks + `froglet-verify` tamper test + `mixing_chains_reports_hash_mismatch`. |
| 6 | Receipt proves signer, unbroken chain, committed result hash, and (Lightning) preimage-attested settlement | `economics.mdx:60-63` | Substantiated per clause: `validate_receipt_artifact` (signer binding), `chain.rs` (links), prepaid preimage check at `kernel.rs:728-747`. |
| 7 | A receipt does **not** prove the result is correct | `economics.mdx:65-71` | Accurate and important — this is the honest boundary the new positioning must preserve. |
| 8 | Lightning proven on regtest 2026-05-15; mainnet not proven | `settlement.mdx:10`, `PAYMENT_MATRIX.md:198-237` | Substantiated by `python/tests/test_lnd_regtest.py` + run log. Mainnet log is empty — correctly labelled. |
| 9 | "Only Lightning currently extends into the standardized signed quote/deal/invoice-bundle flow; Stripe and x402 are local runtime settlement adapters" | `README.md:86-94` | **Now stale in the right direction**: x402 has a kernel method and conformance vectors, but no publish path or live transcript yet. Update only when WS4.5-4.7 land. |
| 10 | x402 is "experimental … not covered by the kernel conformance vectors" | `payment-x402.mdx:6-16` | **Now partly false**: x402 *is* covered by conformance vectors. Still true that it is not on the publish path. Copy must be updated precisely, not wholesale. |
| 11 | "Cheating burns the stake" / stake slashed / reputation zeroed | `index.astro:213,225-232` | **Unsubstantiated.** Contradicted by `economics.mdx:95-100`. Banned by `demo-truth.test.ts:33` for demo copy but not homepage. |
| 12 | "The protocol's incentives are a closed system" | `index.astro:213` | **Unsubstantiated** — depends on staking (see 11). |
| 13 | Two-call install/publish is non-mutating then drift-checked | `quickstart.mdx:59-64`, `provider-onboarding.mdx` | Substantiated by `python/tests/test_install_script.py`; `AGENT_FIRST_PUBLICATION_PLAN.md:144` records the live v0.4.0 caveat. |
| 14 | "A Froglet provider is a keypair and a binary … no account, no approval" | `comparison.mdx:52-56` | Substantiated by `python/tests/test_tor_integration.py`. |
| 15 | Hosted trial "does not prove" paid settlement, identity, publication | `HOSTED_TRIAL.md:163-168`, `llms.txt:41` | Accurate; model negative-claim discipline. |
| 16 | Each publication claim is separate; "none of those proves a payment" | `provider-onboarding.mdx:122-124` | Accurate; the template sentence the new copy should imitate. |
| 17 | "Open protocol that gives AI agents cryptographic identity, signed deals, executable work, and verifiable settlement evidence" | `index.astro:48` | Three of four substantiated; "verifiable settlement evidence" is rail-dependent (Lightning cryptographic, Stripe attested, x402 now cryptographic-authorization + attested-inclusion). Needs per-rail qualification. |
| 18 | Marketplace is "not a protocol root of truth" | `marketplace/overview.md`, `docs.mdx:74` | Substantiated architecturally and consistent with the new position. |

---

## 5. Working-tree state (both repos)

**`froglet`** — branch `main`, HEAD `aaeb375`, **138 dirty files** before this phase (119 modified + 19 untracked;
~49k insertions). Four themes: managed publication/OCI deployment (new subsystem), `src/api/mod.rs` +36.9k,
payment hardening (`stripe.rs` +1,054), install/bootstrap/release surface.

Critically for sequencing: `kernel.rs`, `chain.rs`, `crypto.rs`, `canonical_json.rs`, `identity_attestation.rs`,
`conformance/kernel_v1.json`, and the kernel test files were **all unmodified** — which is why this phase's
work could proceed in parallel without touching the in-flight change.

**`froglet-services`** — branch `main`, 34 modified + 10 untracked paths (~13.5k lines), including two entire
uncommitted services (`operator`, `oci-worker`) and `marketplace-api`'s `dns.rs` + `publication_canary.rs`.
Its release pins `FROGLET_SOURCE_REVISION=29da0b3` (one commit behind protocol main) while its CI floats on
`armanas/froglet@main` — so **CI and release test different protocol revisions**. The 0.5.0 bump will require
a coordinated rebuild there.

---

## 6. Unknowns

0. **Cross-implementation parity caught a real bug — in the runner, not the semantics.** The Python
   conformance runner reported all five x402 reject cases as passing. Root cause, verified: its
   `artifact_verification_cases` check evaluated the **envelope only**, mirroring the `kernel_v1.json`
   reference test, which calls bare `verify_artifact`. That sufficed for `kernel_v1.json`, whose 14 cases are
   all envelope-level tampering, but every x402 reject case is a **correctly signed** artifact that is
   semantically invalid — confirmed by running each through the standalone verifier: `envelope_valid=true`,
   `status=invalid` for all five. An envelope-only check therefore had to pass them. Fixed by combining
   envelope and semantic outcomes in the runner and adding the `x402.eip3009.v1` branch to the Python
   semantics.

   Two corrections to an earlier draft of this section, recorded because the discipline this repo is built on
   requires it. First, the failure was **not** a permissive fallback in the Python method dispatch — dispatch
   already rejected unknown methods; the runner simply never invoked it. Second, an earlier draft reported
   that the runner "exited 0 while printing failures." That was a **measurement error on my part**:
   `cmd | tail; echo $?` reports `tail`'s exit status, not the command's. Measured correctly the runner
   returns 1, and the agent that owns the file could not reproduce an exit-0 result at any point.
   `python/froglet-verify` is untracked, so no before-state exists to settle whether that defect was ever
   real; the honest conclusion is that it was never demonstrated.

   The episode still argues for keeping three independent runners — the Rust implementation could not have
   surfaced the envelope-only gap — and it argues just as strongly for checking exit codes without a pipe.
1. **Does the uncommitted WIP compile?** Not verified — `cargo test --workspace` passes, but that includes the
   dirty files as they stand; no assertion about the *author's intended* end state of that work.
2. **froglet-services build/test status** — surveyed read-only; no `cargo test` was run there.
3. **EIP-712 signature vectors** — `x402_v1.json` deliberately carries no payer signature. Offline recovery
   needs the evidence-refs work (WS3); fabricating a vector before the verifier exists would be untested data.
4. **µUSDC ≡ sat convention** — `validate_x402_payment_binding` (`src/settlement/x402.rs:406`) equates the USDC
   atomic amount with `price_sats`. This implicit 1:1 must become an explicit documented provider attestation
   before any receipt claims a value equivalence. Blocked behind the WIP.
5. **Live x402 transcripts** — no Base Sepolia or mainnet run has happened. Until one does, x402's maturity
   label must not claim live settlement.
6. **Arbiter** — `arbiter.froglet.dev` is live per `aaeb375`, but its complaint signing message is defined only
   in the closed services repo, with no counterpart in `froglet-protocol` and no conformance vector.
