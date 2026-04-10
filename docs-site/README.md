# Froglet Docs Site

Astro + Starlight source for the public Froglet documentation site.

## Local development

Run from [docs-site](/Users/armanas/Projects/github.com/armanas/froglet/docs-site):

| Command | Action |
| :------ | :----- |
| `npm install` | Install site dependencies |
| `npm run dev` | Start the local docs site at `localhost:4321` |
| `npm run build` | Build the production site into `./dist` |
| `npm run preview` | Preview the production build locally |

## Content

- [src/content/docs/learn/quickstart.mdx](/Users/armanas/Projects/github.com/armanas/froglet/docs-site/src/content/docs/learn/quickstart.mdx): install and first-run guide
- [src/pages/index.astro](/Users/armanas/Projects/github.com/armanas/froglet/docs-site/src/pages/index.astro): homepage and landing copy
- [src/content/docs/spec/kernel.md](/Users/armanas/Projects/github.com/armanas/froglet/docs-site/src/content/docs/spec/kernel.md): protocol/kernel reference

The docs site should stay aligned with the repo-level [README.md](/Users/armanas/Projects/github.com/armanas/froglet/README.md) quickstart and [docs/RELEASE.md](/Users/armanas/Projects/github.com/armanas/froglet/docs/RELEASE.md) release process.
