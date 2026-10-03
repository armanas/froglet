# Website dependency applicability review — 2026-10-03

`http-cache-semantics` **4.2.0 remains vulnerable** to
[GHSA-ch52-4w7c-c8xp](https://github.com/advisories/GHSA-ch52-4w7c-c8xp).
The reviewed versions have no published upstream fix as of this review.
[Upstream issue 56](https://github.com/kornelski/http-cache-semantics/issues/56)
describes shared-cache responses with security-zeroed lifetimes being reused
when a different client supplies `Cache-Control: max-stale`, potentially
disclosing another user's `Set-Cookie` credentials.
[Proposed fix 58](https://github.com/kornelski/http-cache-semantics/pull/58) and
[cumulative fix 60](https://github.com/kornelski/http-cache-semantics/pull/60)
are open proposals, not released or maintainer-approved security fixes.

The docs CI policy applies one temporary **applicability exception** to this
advisory and its known Astro dependency effects. It does not patch or hide the
package finding. All other high/critical findings, audit/registry errors, or
failed applicability checks block the job. The policy expires at
**2026-10-17 00:00 UTC**, or earlier when a newer stable version is published.
An updated dependency or changed advisory requires a fresh review; a clean
audit passes without applying an exception.

## Reviewed boundaries and evidence

- The exact lockfile consumer is `astro` **7.3.5**, requiring
  `http-cache-semantics` `^4.2.0`. The guard requires the installed and locked
  versions to match, and rejects additional normal, optional, peer, development,
  or aliased consumers.
- Astro's reviewed
  [`assets/build/remote.ts`](https://github.com/withastro/astro/blob/astro%407.3.5/packages/astro/src/assets/build/remote.ts)
  uses `storable()` and `timeToLive()` to calculate build-image expiry. It
  creates its own empty/conditional request headers and does not invoke
  `evaluateRequest()` or `satisfiesWithoutRevalidation()`. The installed
  compiled file's exact SHA-256 is guarded. This does not certify all remote
  image caching behavior; that caller also owns expiry/error fallbacks.
- Project production sources have no `astro:assets`, `astro/assets`, or
  `http-cache-semantics` imports. The guard scans executable source variants
  outside tests, rejects source symlinks and production references into test
  modules, decodes module-string escapes and line continuations, and
  conservatively rejects even prose/comment references until reviewed.
  Existing test-evidence links in `maturity.ts` are admitted only at the exact
  reviewed file hash; this does not allow importing a module from tests.
- The reviewed Astro configuration uses its default static output with no
  SSR adapter. `wrangler.jsonc` uploads that static `dist` and the separate
  `src/worker.ts` entrypoint. Both configuration files are pinned by hash.
- A fresh Wrangler **4.143.1** production dry-run emitted a Worker containing
  `qrcode-generator`, Froglet's service-page/data code, and generated verifier
  glue. Its source map matched all 11 current source files exactly. Neither
  Astro nor this cache library nor the vulnerable reuse methods appear in
  the deployment JavaScript. All source hashes, the source set, and JavaScript
  bundle bytes are checked again on every excepted audit. Only the content
  hash in the generated verifier Wasm filename is normalized; JavaScript
  changes remain blocking. The separate Wasm asset must exist.
- The local preview emulator contains a bundled older cache-policy copy,
  but its cache caller computes TTL, discards request Cache-Control, and
  rejects stored Set-Cookie responses. It does not call the vulnerable
  reuse method. Miniflare is not uploaded as part of this custom Worker;
  this review makes no claim about Cloudflare's internal cache implementation.
- A synthetic, network-free reproduction on 4.2.0 confirmed `maxAge() === 0`,
  ordinary reuse rejected, and `max-stale` reuse accepted with a fixture
  Set-Cookie header. Passing the applicability guard does **not** mean the
  library defect was disproven or repaired.

Run the full audit with the reviewed guard after the website build:

```sh
npm run build --prefix docs-site
node docs-site/scripts/audit-dependencies.mjs
```

The helper fetches the full npm audit, checks the official npm version list,
and runs `wrangler deploy --dry-run` locally to inspect its actual output.
It never deploys or changes dependencies. The raw command
`npm audit --prefix docs-site --audit-level=high` still reports the unresolved
advisory until upstream publishes a fix and dependencies are updated.

Changing versions, configuration, reviewed Worker code/dependencies, bundle
output, or adding asset/cache imports invalidates this exception. Reassess
before introducing SSR, private remote image data, an incoming-header cache,
or another affected consumer. The CI helper deliberately fails closed instead
of automatically broadening this review.
