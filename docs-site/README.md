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
| `npm run deploy` | Build and deploy to Cloudflare Workers via Wrangler |

## Production deploy

The public docs site is configured for Cloudflare Workers, not GitHub Pages.
This repo now carries [`wrangler.jsonc`](./wrangler.jsonc) as the canonical
deploy configuration for both manual deploys and Cloudflare Workers Builds.

For a Cloudflare dashboard-backed build:

- Build command: `npm run build`
- Build environment: Rust 1.91, the `wasm32-unknown-unknown` target, and Cargo. The build installs the exact wasm-bindgen CLI version from Cargo.lock into the Cargo target directory.
- Run `rustup target add wasm32-unknown-unknown` once before building locally.
- Deploy command: `npx wrangler deploy`

Attach `froglet.dev` to the Worker deployment. `docs.froglet.dev` previously
mirrored the same build, but the apex is now the only advertised public docs
host to keep launch copy and monitoring simple.

## Content

- [src/pages/publish.astro](./src/pages/publish.astro): primary publishing journey
- [src/pages/service.astro](./src/pages/service.astro): identity-bound sharing and recipient prompt
- [src/content/docs/learn/share-services.mdx](./src/content/docs/learn/share-services.mdx): matching native agent instructions. The build emits this same guide at `/publish/agent.md` and Quickstart at `/install/agent.md` using [src/pages/[journey]/agent.md.ts](./src/pages/[journey]/agent.md.ts); do not maintain a second copy of the commands. `/llms.txt` routes by user intent and keeps the optional hosted proof separate.
- [src/content/docs/learn/quickstart.mdx](./src/content/docs/learn/quickstart.mdx): install and first-run guide
- [src/pages/index.astro](./src/pages/index.astro): homepage and landing copy
- [src/content/docs/spec/kernel.md](./src/content/docs/spec/kernel.md): protocol/kernel reference

The docs site should stay aligned with the repo-level [README.md](../README.md) quickstart and [docs/RELEASE.md](../docs/RELEASE.md) release process.

The marketplace and shared-service APIs belong to `src/worker.ts`. Test them with `npm run preview:workers`; Astro preview alone serves static pages and cannot verify these runtime routes. No API snapshot is emitted at build time. `/s/{provider_id}/{service_id}` serves initial HTML, a JSON manifest, and `/agent.md` through that Worker. It reads only the identity-derived first-party relay origin and verifies the active signed evidence before describing an offer as available. Bind a separate `SERVICE_LINK_CACHE` Cloudflare KV namespace to each deployed environment to retain the last verified public description for up to 30 days; cached descriptions are labeled stale and never callable. Without that binding, offline links show an unknown state. `/api/shared-service` remains for old `/service/` links. Outages and unknown lifecycle states are reported separately from requester execution.
