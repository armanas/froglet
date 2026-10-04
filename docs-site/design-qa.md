# Website rework verification — 2026-09-27

**Follow-up correction:** The original pass missed the third arm in the review illustration and did not cover the remaining learning documentation. Its blanket “No findings” conclusion was too broad. See [the follow-up review](documentation-review.md) for corrected findings and new verification.

**Original pass:** the route, interaction, and layout checks below passed within their recorded scope.

## Design intent and comparison evidence

This is an intentional redesign, not a pixel clone of the old site. The user's approved illustration language and Froglet's existing typography and color tokens are the visual references. The requested change is a simpler explanation for people and organizations, with technical setup moved to the developer path.

- Source visual: `public/illustrations/01-homepage-hero-1600.webp` (1600 × 900 pixels), copied from the previously approved asset collection in `../_tmp/froglet-website-assets-2026-09-27/web/`.
- Before: `../_tmp/website-rework/evidence/home-before.png`.
- Final desktop: `../_tmp/website-rework/evidence/home-after.png` (1440 × 1000 pixels, 1440 × 1000 CSS viewport).
- Final mobile: `../_tmp/website-rework/evidence/home-mobile.png` (390 × 844 pixels, 390 × 844 CSS viewport).
- Implementation: `http://127.0.0.1:4323/`, served by the local Worker runtime from the production build. Earlier checks used Astro at port 4321.
- State: no account or authentication; system preference resolves to dark. Light theme also checked and the preference restored.
- Density: browser captures are one image pixel per CSS pixel. The reference is a standalone illustration rather than a webpage, so its intended browser scaling and removal of blank left-side space were evaluated explicitly. No whole-page pixel equality is claimed.

The source illustration, final desktop implementation, and light-theme capture were opened together in one comparison input. Bob, Alice, the service symbol, and connecting line remain intact. The artwork retains its original charcoal, cream, green, and print texture. The left-side blank space is intentionally cropped to allow accessible HTML copy alongside it.

Focused rendered evidence is in `../_tmp/website-rework/evidence/`: `home-walkthrough-desktop.png`, `home-organizations-desktop.png`, `home-footer-desktop.png`, `home-faq-mobile.png`, `publish-prompt-desktop.png`, `publish-mobile.png`, `organizations-desktop.png`, `organizations-mobile.png`, `docs-mobile-sidebar.png`, and `marketplace-runtime.png`. These show readable content and interaction states beyond the hero comparison.

## Required visual surfaces

- **Typography:** existing JetBrains Mono headings and Inter body copy retained. Large headings, 17–20px main body text, and short action labels establish the hierarchy. Long headings checked at 320px and 390px; no hero-column overflow after the responsive repair.
- **Spacing and layout:** two-column desktop heroes, three-step walkthrough, stacked mobile sections, and shared margins verified. Tablet navigation changes to the menu at 900px. The documentation header matches its actual 56px container.
- **Colors:** existing dark/light tokens retained. Light-theme accent text uses the darker green token; measured foreground/background contrast is 6.43:1. Original dark illustration backgrounds remain visible as panels in light mode by design.
- **Image quality:** real approved WebP assets used with intrinsic dimensions and responsive sources. All homepage images loaded successfully. No generated illustration was replaced with a CSS or SVG imitation.
- **Copy:** the first visit explains sharing selected data or a useful task, reviewing an example, and sending a link. The organization page uses the life-sciences terminology example and distinguishes current access/execution controls from future agreement and SLA automation. Technical setup and integration maturity remain available on the developer page.

## Repair history

1. **P2 — Light-theme headline contrast.** The bright green display accent was weak against the light background. `src/styles/story-pages.css` now uses `--frog-700` in light mode. Post-repair evidence: `home-light-desktop.png`, inspected alongside the source and dark result; measured contrast 6.43:1.
2. **P2 — Narrow organization heading exceeded its grid column.** At 320px, the content column was 284px while the heading's minimum content width reached about 302px. Flexible zero-minimum grid tracks and a responsive 30–37px headline fix the overflow. Rechecked at 320px: heading, column, and column scroll width all 284px. Mobile layout evidence: `organizations-mobile.png` at 390px.
3. **P2 — Documentation menus overlapped and the sidebar control was behind the header.** `src/components/SiteHeader.astro` now reserves space for Starlight's actual `.sl-menu-button`, constrains the embedded header height, and raises the control above the header. Post-repair measured bounds: site menu x≈242–322; sidebar button x330–374; header height56. Both menus were opened independently. The sidebar reported `:popover-open=true`; `docs-mobile-sidebar.png` shows its visible contents.
4. **P2 — Catalog failure exposed a parser message as the main status.** The main status now uses plain language; diagnostics remain within the technical details disclosure and copied evidence. The malformed-response regression test passes. The initial local error came from using Astro without the Worker API, not from evidence of a live outage. Through the full local Worker runtime, the catalog returned 3 providers and 6 offers and displayed the successful status. Service execution availability remains separately labeled.
5. **P2 — Docs entry copy contradicted the current hosted-trial status.** `src/content/docs/docs.mdx` now leads with guided sharing and local setup, links to trial status, and explicitly labels staked identity as undeployed. The rendered docs page was inspected at desktop and mobile sizes.

## Runtime checks

- Production build: `npm run build` — passed, 37 pages. Log: `../_tmp/website-rework/build-final.log`.
- Website tests: `npm test -- --reporter=dot` — 172 passed across 22 files. Log: `../_tmp/website-rework/test-final.log`.
- `git diff --check` — passed.
- Live routes inspected: `/`, `/publish/`, `/managed/`, `/open-source/`, `/marketplace/`, `/docs/`.
- Viewports: 1440 × 1000 desktop, 820 × 1100 tablet, 390 × 844 mobile, and a focused 320 × 844 organization heading check.
- Interactions: homepage walkthrough anchor; sharing-page setup anchor and copy confirmation; organization capability anchor; FAQ disclosure; mobile route navigation; Escape dismissal; documentation sidebar; theme changes; developer installation disclosure and Docker/Codex selection.
- Generated setup output included both `FROGLET_AGENT_TARGET=codex` and `FROGLET_BOOTSTRAP_MODE=docker` after selection. No installation command was executed.
- Browser error/warning log checks returned no entries in the tested final page states.

## Changed files

- Story pages: `src/pages/index.astro`, `publish.astro`, `managed.astro`, `open-source.astro`.
- Discovery explanation/status: `src/pages/marketplace.astro`, `src/scripts/marketplace-live.ts`.
- Shared presentation: `src/layouts/StoryLayout.astro`, `src/components/StoryArt.astro`, `OperatorSetup.astro`, `SiteHeader.astro`, `SiteFooter.astro`, `src/styles/story-pages.css`, `src/data/nav-links.ts`.
- Supporting content: `src/content/docs/docs.mdx`, `src/data/agent-metadata.ts`, and approved WebP assets in `public/illustrations/`.
- Existing test updates: `src/scripts/__tests__/hosted-trial-copy.test.ts`, `positioning-truth.test.ts`, `marketplace-live.test.ts`.

## Residual limits

This verifies the website and the inspected UI states, not a new backend capability. No live service was published, bought, or executed. The user journey has not been tested with nontechnical participants, and no broad cross-browser or assistive-technology audit was performed. Kernel behavior, deployment-specific access controls, billing, and arbitrary SLA enforcement were not changed or certified. The website is local and has not been deployed.

Implementation checklist: page changes applied; original assets integrated; interaction and responsive checks completed; tests/build passed; local preview available.


## Follow-up verification

The shared review illustration was repaired in both website sizes and the original asset pack. The homepage and publishing walkthrough now render the corrected artwork. Onboarding and concept documentation were corrected; mobile identity controls and inline math rendering were repaired. The follow-up build passed with 37 pages, and 173 tests passed across 23 files. Exact files, findings, evidence, and limits are recorded in [documentation-review.md](documentation-review.md).


## Deployment follow-up — 27 September 2026

The reviewed website was subsequently deployed to [froglet.dev](https://froglet.dev/), version `5e066d2e-ef42-433f-98f2-06186618f42a`. Seventeen production pages/assets matched the built files byte for byte; the marketplace API passed and the live homepage, repaired illustration, introduction, and identity interaction were inspected. The detailed deployment record is local ignored evidence at `../_tmp/website-deploy-2026-09-27/deployment.md`, not included in a source checkout. See [the maintained deployment guide](README.md#production-deploy) for current publication and rollback state. The earlier “not deployed” statements describe the review-time state.
