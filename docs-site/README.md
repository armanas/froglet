# Froglet Docs Site

Astro + Starlight source for the public Froglet documentation site.

## Local development

Run from the `docs-site/` directory:

| Command | Action |
| :------ | :----- |
| `npm install` | Install site dependencies |
| `npm run dev` | Start the local docs site at `localhost:4321` |
| `SITE_URL=https://ai.froglet.dev npm run build` | Build the production site into `./dist` |
| `npm run preview` | Preview the production build locally |

## Content

- [src/content/docs/learn/quickstart.mdx](./src/content/docs/learn/quickstart.mdx): install and first-run guide
- [src/pages/index.astro](./src/pages/index.astro): homepage and landing copy
- [src/content/docs/spec/kernel.md](./src/content/docs/spec/kernel.md): protocol/kernel reference

The docs site should stay aligned with the repo-level [README.md](../README.md) quickstart and [docs/RELEASE.md](../docs/RELEASE.md) release process.
