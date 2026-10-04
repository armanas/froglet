# Froglet Docs Site

Astro + Starlight source for the public Froglet documentation site.

## Local development

Use Node.js 22.12 or newer and Rust 1.91. Run `rustup target add wasm32-unknown-unknown` from the repository first. From `docs-site/`, run `npm ci` and `npm run build:wasm` before development or tests. It builds the browser verifier (`froglet-verify`), the playground's signing kernel (`froglet-wasm`), the Rust services in `examples/wasm-services` (the tests run them as the answer to check the editor's functions against), and the AssemblyScript compiler bundle that the playground's editor loads. Generated files are ignored and rebuilt from the locked source.

Social cards use the licensed, source-controlled TTF inputs in
[`scripts/fonts`](./scripts/fonts/README.md); building them does not download
fonts or depend on the old ignored `.fonts` cache. This does not make a first
site build offline: npm dependencies, Rust toolchains/targets, Cargo crates,
and the exact `wasm-bindgen` CLI must already be installed or downloadable.
`package-lock.json`, the Rust lockfiles and `rust-toolchain.toml` pin those
software inputs. A preinstalled `WASM_BINDGEN` is accepted only when its version
matches `Cargo.lock`.

Two test files are opt-in because they need a built node. Run `cargo build -p froglet --bin froglet-node` from the repository first (`FROGLET_NODE_BIN` points at a different binary), then, from `docs-site/`:

| Variable | Test file | What it does |
| :------- | :-------- | :----------- |
| `FROGLET_PLAYGROUND_REAL_REQUESTER=1` | `playground-real-requester.test.ts` | Starts a runtime-only `froglet-node` and has that node's requester call the playground's provider. |
| `FROGLET_PLAYGROUND_REAL_PROVIDER=1` | `playground-real-provider.test.ts` | Starts a `froglet-node` as provider and runtime, publishes modules that the editor's compiler produced, and calls them with the playground's consumer. It checks the node's answers, its receipts' module hashes, and its failures against the playground's own. |

For example, `FROGLET_PLAYGROUND_REAL_PROVIDER=1 npx vitest --run src/scripts/__tests__/playground-real-provider.test.ts`. Without its variable each file is skipped.

Run from the `docs-site/` directory:

| Command | Action |
| :------ | :----- |
| `npm ci` | Install site dependencies |
| `npm run dev` | Start the local docs site at `localhost:4321` |
| `SITE_URL=https://froglet.dev npm run build` | Build the production site into `./dist` |
| `npm run preview` | Preview the production build locally |
| `npm run preview:workers` | Build and preview the Cloudflare Workers deployment locally |
| `npm run preview:maintenance` | Preview the temporary pause page locally |
| `npm run deploy` | Build and publish the full site |
| `npm run deploy:site` | Build and publish the full site (same target) |
| `npm run deploy:maintenance` | Explicitly replace the full site with the temporary pause page |

## Production deploy

The public docs site is configured for Cloudflare Workers, not GitHub Pages.
The site was restored on 1 October 2026. `npm run deploy` now builds and
publishes [`wrangler.jsonc`](./wrangler.jsonc); maintenance deployment requires
`npm run deploy:maintenance` explicitly. The default no longer restores the
September pause page. The 3 October publication restored the Alithea Bio
footer attribution and published the functionality matrix and qualification
disclosures at version `1a07cc84-eeb2-4c1f-8951-471ac6676401`. A later update that
day published tested terminal-recovery guidance at
`c4faedb2-caf2-44da-ab03-ceab37f736f3`, retaining the preceding version for rollback.
Six public pages and the logo returned HTTP 200 and matched the prepared build
bytes at that checkpoint. Historical response checks remain in ignored launch
evidence. These dated versions are not a claim about the current deployment;
check `npx wrangler deployments list --config wrangler.jsonc` and record the
current version and rollback target before another publication.

Both full-site publication scripts build, run the dependency/deployment guard,
and publish only if it passes. A clean package audit does not bypass the
reviewed cache-behavior boundary. Website publication remains separate from
Node release or compute deployment.

For a full-site Cloudflare dashboard-backed build:

- Build command: `npm run build`
- Build environment: Rust 1.91, the `wasm32-unknown-unknown` target, and Cargo. The build installs the exact wasm-bindgen CLI version from Cargo.lock into the Cargo target directory.
- Run `rustup target add wasm32-unknown-unknown` once before building locally.
- Deploy command: `npm run deploy:site` (includes the required guard)

That direct command publishes the full site. Dashboard build
settings are external to this repository; they were not changed during cleanup.

Attach `froglet.dev` to the Worker deployment. `docs.froglet.dev` previously
mirrored the same build, but the apex is now the only advertised public docs
host to keep launch copy and monitoring simple.

### Temporary pause (2026-09-28)

The separate `wrangler.maintenance.jsonc` deploys only `src/maintenance-worker.ts`
to the production Worker. It serves “Coming back soon” on every site route,
including old assets and APIs, with HTTP 503, `Retry-After`, and no caching.
The favicon is the only 200 response. It makes no upstream requests and does
not build or publish the current working copy of the full site.

```sh
npx wrangler dev --config wrangler.maintenance.jsonc
npx wrangler whoami
npx wrangler deploy --config wrangler.maintenance.jsonc --message "Temporary coming-back-soon page"
```

Before the pause, production served version
`213923ee-0ab4-4fe6-8493-114a5f5aae0a` at 100%. Restore that exact deployment,
including its assets. The historical rollback command at that checkpoint was:

```sh
npx wrangler rollback 213923ee-0ab4-4fe6-8493-114a5f5aae0a --config wrangler.jsonc --message "Restore site after temporary pause"
```

At the September checkpoint, the default `npm run deploy` preserved this
page. Since the October restoration it publishes the full site; use the
explicit maintenance command above for a new pause. The candidate site,
provider services, relay, and marketplace backend are separate deployments and
are not stopped by this website pause.

Published pause version: `e2e1b8a7-75bd-49c6-b6f4-1efc61d066d6`
(subtitle update; live browser verified).
Verified live on 2026-09-28: homepage, marketplace, documentation, an existing
shared-service link, the site snapshot API, and an old JavaScript asset all
returned the notice with 503/no-store/Retry-After. HEAD returned no body; the
favicon returned SVG/200. Desktop and mobile browser rendering were inspected.

## Content

- [src/pages/publish.astro](./src/pages/publish.astro): primary publishing journey
- [src/pages/service.astro](./src/pages/service.astro): identity-bound sharing and recipient prompt
- [src/content/docs/learn/share-services.mdx](./src/content/docs/learn/share-services.mdx): matching native agent instructions. The build emits this same guide at `/publish/agent.md` and Quickstart at `/install/agent.md` using [src/pages/[journey]/agent.md.ts](./src/pages/[journey]/agent.md.ts); do not maintain a second copy of the commands. `/llms.txt` routes by user intent and keeps the optional hosted proof separate.
- [src/content/docs/learn/quickstart.mdx](./src/content/docs/learn/quickstart.mdx): install and first-run guide
- [src/pages/index.astro](./src/pages/index.astro): homepage and landing copy
- [src/content/docs/spec/kernel.md](./src/content/docs/spec/kernel.md): protocol/kernel reference

The docs site should stay aligned with the repo-level [README.md](../README.md) quickstart and [docs/RELEASE.md](../docs/RELEASE.md) release process.

The marketplace and shared-service APIs belong to `src/worker.ts`. Test them with `npm run preview:workers`; Astro preview alone serves static pages and cannot verify these runtime routes. No API snapshot is emitted at build time. `/s/{provider_id}/{service_id}` serves initial HTML, a JSON manifest, and `/agent.md` through that Worker. It reads only the identity-derived first-party relay origin and verifies the active signed evidence before describing an offer as available. Bind a separate `SERVICE_LINK_CACHE` Cloudflare KV namespace to each deployed environment to retain the last verified public description for up to 30 days; cached descriptions are labeled stale and never callable. Without that binding, offline links show an unknown state. `/api/shared-service` remains for old `/service/` links. Outages and unknown lifecycle states are reported separately from requester execution.
