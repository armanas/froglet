# Froglet Docs Site

Astro + Starlight source for the public Froglet documentation site.

## Local development

Use Node.js 22.12 or newer and Rust 1.91. Run `rustup target add wasm32-unknown-unknown` from the repository first. From `docs-site/`, run `npm ci` and `npm run build:verifier` before development or tests. Generated WASM bindings are ignored and rebuilt from the locked Rust source.

Run from the `docs-site/` directory:

| Command | Action |
| :------ | :----- |
| `npm ci` | Install site dependencies |
| `npm run dev` | Start the local docs site at `localhost:4321` |
| `SITE_URL=https://froglet.dev npm run build` | Build the production site into `./dist` |
| `npm run preview` | Preview the production build locally |
| `npm run preview:workers` | Build and preview the Cloudflare Workers deployment locally |
| `npm run preview:maintenance` | Preview the temporary pause page locally |
| `npm run deploy` | Deploy only the maintenance page while the site is paused |
| `npm run deploy:site` | Explicitly build and publish the full site, ending the pause |

## Production deploy

The public docs site is configured for Cloudflare Workers, not GitHub Pages.
While the site is paused, `npm run deploy` uses
[`wrangler.maintenance.jsonc`](./wrangler.maintenance.jsonc).
[`wrangler.jsonc`](./wrangler.jsonc) remains the full-site configuration for
local development and a deliberate future relaunch.

For a future full-site Cloudflare dashboard-backed build:

- Build command: `npm run build`
- Build environment: Rust 1.91, the `wasm32-unknown-unknown` target, and Cargo. The build installs the exact wasm-bindgen CLI version from Cargo.lock into the Cargo target directory.
- Run `rustup target add wasm32-unknown-unknown` once before building locally.
- Deploy command: `npx wrangler deploy`

That direct command publishes the full site and ends the pause. Dashboard build
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
including its assets, when the pause is over:

```sh
npx wrangler rollback 213923ee-0ab4-4fe6-8493-114a5f5aae0a --config wrangler.jsonc --message "Restore site after temporary pause"
```

`npm run deploy:site` and direct full-site Wrangler deployments replace this
temporary page; the default `npm run deploy` preserves it. The candidate site,
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
