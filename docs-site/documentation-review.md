# Website documentation review — 27 September 2026

## Findings and corrections

1. **P1 — Identity documentation overstated what signatures establish.** `src/content/docs/learn/identity.mdx:8`, `:42`, `:73` previously described signing as locking/unlocking, provided an incomplete BIP340 signing recipe, and claimed Nostr publication could not be deleted. A reader could confuse authenticity with secrecy or implement incompatible signing. Replaced with key-control and integrity explanations, explicit trust limits, and primary references. Checked against `froglet-protocol/src/crypto.rs`, `src/identity.rs`, `src/nostr.rs`, [BIP340](https://bips.dev/340/), and [NIP-01](https://github.com/nostr-protocol/nips/blob/master/01.md). Canonical signing bytes and implementation were not changed.

2. **P1 — The proposed optimal fee contradicted its failure break-even claim.** `src/content/docs/learn/economics.mdx:236` formerly proposed `b = c / (1 + q)` and claimed failed executions broke even. At `c = 2`, `q = 0.5`, it gives `b = 1.333…`, so a failed attempt loses `0.666…`. Now distinguishes `b >= c` per failed attempt from `b + qf >= c` in expectation, with a numerical example. Participation constraints (`:182`), discovery return, and expected welfare now use the same probabilistic model. Removed unsupported claims of a unique equilibrium and guaranteed cost-level prices. Existing `profit-chart.ts` payoff functions and tests agree with the corrected arithmetic. Identity cost remains explicitly undeployed and is not described as a refundable bond or proof of quality.

3. **P1 — The settlement guide generalized one payment method to all paid deals.** `src/content/docs/learn/settlement.mdx:27` formerly said every paid deal had two Lightning legs and that the base fee merely locked on admission. The guide now explicitly scopes this model to `lightning.base_fee_plus_success_fee.v1`, states that the base fee settles before admission, and separates prepaid and other rails. Evidence: `docs/KERNEL.md:233` and `:469`; the latter requires a settled base leg and an accepted/settled success leg before execution. The maturity note follows the local payment matrix and separates regtest evidence from mainnet proof.

4. **P2 — Beginner paths contradicted the new website and the documented hosted outage.** `astro.config.mjs:36`, `src/content/docs/learn/introduction.mdx:19`, and the entry pages listed below led with cryptographic terminology, advanced provider setup, or a hosted trial already marked unavailable. The introduction now uses Bob's terminology table and Alice's lookup, explains five technical terms, and states current controls and limits. Navigation leads to the supported sharing guide; hosted links are labeled status/recheck paths. The pre-existing 24 September outage observation is preserved as dated evidence, not represented as a fresh availability test. Operator and package instructions remain available under Reference.

5. **P2 — The comparison blurred open protocols and hosted product restrictions.** `src/content/docs/learn/comparison.mdx:14` formerly described ACP through Stripe/ChatGPT onboarding, reduced competing protocols to moving money, and included unsupported broad claims and volatile adoption counts. Replaced with a narrower responsibilities comparison and primary links: [ACP](https://www.agenticcommerce.dev/), [AP2](https://ap2-protocol.org/), [x402](https://x402.org/), and [L402](https://docs.lightning.engineering/the-lightning-network/l402). Possible combinations are explicitly not claims of tested interoperability.

6. **P2 — Mathematical prose and the identity diagram were difficult to read.** `src/styles/starlight-overrides.css:27` now keeps inline MathJax SVGs inline. The compiled Starlight rule `.sl-markdown-content :is(img,picture,video,canvas,svg,iframe)` applied `display:block` to inline equations; live computed SVG display changed from `block` to `inline`. `src/components/IdentityDiagramInit.astro` and `src/scripts/identity-diagram.ts:31` replace a canvas whose minimum box widths clipped the third node on a 390px viewport with responsive native buttons. The full diagram is visible on mobile; Enter changes the explanation and `aria-pressed` state. A regression test covers selection and the rendered explanation.

7. **P2 — The shared review illustration had a third arm.** Both sizes of `public/illustrations/03-walkthrough-preview-{640,1254}.webp` now use the repaired artwork. Built-in Imagegen removed the extra holding arm, hand, and floating card while preserving the original composition and palette. The PNG master, gallery exports, manifests, prompt record, and downloadable ZIP were updated too. Live evidence shows the corrected anatomy on `/` and `/publish/`. The earlier design review missed this defect; its blanket “No findings” conclusion was too broad.

## Verification

- `npm run build`: passed, 37 pages; `../_tmp/website-review-2026-09-27/build.log`.
- `npm test -- --reporter=dot`: 173 passed across 23 files; `../_tmp/website-review-2026-09-27/test.log`.
- `git diff --check`: passed.
- Live production build served by the local Worker at `http://127.0.0.1:4323/`.
- Rendered documentation inspected: introduction, identity, economics, settlement, comparison, legacy learn index, hosted trial status, agent connection, plugin distribution, payment rails, and sharing guide.
- Focused desktop screenshots at 1440 × 1000 and mobile at 390 × 844; illustration additionally inspected at the user's 602px width. Introduction, diagram, and economic prose fit the 390px viewport without document overflow. The identity explanation was selected through keyboard Enter, and exactly one button reported `aria-pressed=true`.
- Browser warning/error logs were empty in the inspected final states. MathJax reported no error elements on the economics page.
- Website and asset-pack WebP bytes match; the replacement ZIP member matches the repaired gallery export.

Evidence directory: `../_tmp/website-review-2026-09-27/evidence/`. Key captures: `home-repaired-desktop.png`, `publish-repaired-desktop.png`, `docs-introduction-desktop.png`, `docs-introduction-mobile.png`, `identity-mobile-before.png`, `identity-mobile-after.png`, `economics-fees-after.png`, `economics-mobile.png`, `settlement-desktop.png`, and `comparison-desktop.png`.

## Exact website files changed in this follow-up

- `astro.config.mjs`
- `src/content/docs/learn/index.mdx`
- `src/content/docs/learn/introduction.mdx`
- `src/content/docs/learn/share-services.mdx`
- `src/content/docs/learn/cloud-trial.mdx`
- `src/content/docs/learn/agents.mdx`
- `src/content/docs/learn/plugin-distribution.mdx`
- `src/content/docs/learn/payment-rails.mdx`
- `src/content/docs/learn/identity.mdx`
- `src/content/docs/learn/economics.mdx`
- `src/content/docs/learn/settlement.mdx`
- `src/content/docs/learn/comparison.mdx`
- `src/components/IdentityDiagramInit.astro`
- `src/scripts/identity-diagram.ts`
- `src/scripts/__tests__/identity-diagram.test.ts`
- `src/styles/starlight-overrides.css`
- `public/illustrations/03-walkthrough-preview-640.webp`
- `public/illustrations/03-walkthrough-preview-1254.webp`
- `design-qa.md` and this report

Asset-generation provenance and the exact repair prompt are in `../_tmp/website-review-2026-09-27/image-repair.md`. Pre-edit learning docs and sidebar configuration were copied to `../_tmp/website-review-2026-09-27/before/`.

## Remaining scope and uncertainty

The listed findings are corrected and verified in the local website. This is a review of the website's onboarding and concept documentation, not certification of every repository document or backend implementation. Installation commands, paid transactions, arbitrary SLA enforcement, external marketplace code, and current hosted service uptime were not exercised. No protocol, kernel, billing, or deployment behavior changed. User comprehension has not been tested with nontechnical participants. The changes have not been deployed.


## Deployment follow-up — 27 September 2026

The reviewed website was subsequently deployed to [froglet.dev](https://froglet.dev/), version `5e066d2e-ef42-433f-98f2-06186618f42a`. Seventeen production pages/assets matched the built files byte for byte; the marketplace API passed and the live homepage, repaired illustration, introduction, and identity interaction were inspected. See [deployment evidence](../_tmp/website-deploy-2026-09-27/deployment.md). The earlier “not deployed” statements describe the review-time state.
