# Froglet Docs Site

Astro + Starlight source for the public Froglet documentation site.

## Local development

Run from the `docs-site/` directory:

| Command | Action |
| :------ | :----- |
| `npm install` | Install site dependencies |
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

- Build command: `npx astro build`
- Deploy command: `npx wrangler deploy`

Attach both `froglet.dev` and `docs.froglet.dev` to the same Worker deployment
so the apex remains canonical and `docs.froglet.dev` stays a mirror.

## Content

- [src/content/docs/learn/quickstart.mdx](./src/content/docs/learn/quickstart.mdx): install and first-run guide
- [src/pages/index.astro](./src/pages/index.astro): homepage and landing copy
- [src/content/docs/spec/kernel.md](./src/content/docs/spec/kernel.md): protocol/kernel reference

The docs site should stay aligned with the repo-level [README.md](../README.md) quickstart and [docs/RELEASE.md](../docs/RELEASE.md) release process.
