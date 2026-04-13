# Implementation Plan: Docs Site Refinement

## Overview

This plan implements 22 requirements across five layers: CSS modularization, script extraction, theme/accessibility fixes, mathematical correctness verification, and new interactive elements. Foundational work (CSS split, shared modules) comes first so that dependent work (new interactive elements, page refactors) builds on stable infrastructure. All code targets TypeScript for scripts and CSS for styles within the `docs-site/` directory.

## Tasks

- [x] 1. Split monolithic CSS into modular partials
  - [x] 1.1 Create `docs-site/src/styles/tokens.css` with all `:root` and `[data-theme='dark']` custom property declarations extracted from `custom.css`, including new standardized component tokens (`--froglet-card-hover-shift`, `--froglet-grid-gap`, `--froglet-transition-fast`, tag color tokens)
    - _Requirements: 8.1, 15.5_
  - [x] 1.2 Create `docs-site/src/styles/starlight-overrides.css` with all Starlight theme override rules (sidebar, header, content panel, pagination, right sidebar, markdown content styles) extracted from `custom.css`
    - _Requirements: 8.2_
  - [x] 1.3 Create `docs-site/src/styles/components.css` with all reusable component styles (`.learn-*`, `.plot-*`, `.protocol-*`, `.chapter-*`, `.compare-*`, `.trust-*`) extracted from `custom.css`, updating hard-coded values to use Design Tokens for card, button, and grid patterns
    - _Requirements: 8.3, 15.1, 15.2, 15.3, 15.4_
  - [x] 1.4 Rewrite `docs-site/src/styles/custom.css` as a root entry point that imports `tokens.css`, `starlight-overrides.css`, and `components.css` in correct cascade order
    - _Requirements: 8.4, 8.5_

- [x] 2. Extract demo and 404 page inline styles to external CSS files
  - [x] 2.1 Create `docs-site/src/styles/demo-page.css` from the inline `<style is:inline>` block in `demo.astro`, referencing Design Tokens from the shared token definitions instead of re-declaring shorthand aliases; update `demo.astro` to import the external stylesheet
    - _Requirements: 9.1, 9.2, 9.3_
  - [x] 2.2 Create `docs-site/src/styles/404-page.css` from the inline `<style is:inline>` block in `404.astro`, referencing Design Tokens from the shared token definitions; update `404.astro` to import the external stylesheet
    - _Requirements: 10.1, 10.2, 10.3_
  - [x] 2.3 Remove duplicate Google Fonts `@import` statements from `demo.astro` and `404.astro`, relying on the shared Starlight head configuration in `astro.config.mjs` for Inter and JetBrains Mono loading
    - _Requirements: 12.1, 12.2, 12.3_

- [x] 3. Checkpoint — Verify CSS split produces identical rendering
  - Ensure the site builds without errors (`npm run build` in `docs-site/`). Ensure all tests pass, ask the user if questions arise.

- [x] 4. Create shared clipboard module and extract landing page scripts
  - [x] 4.1 Create `docs-site/src/scripts/clipboard.ts` implementing the shared copy-to-clipboard module with `initCopyButtons()` and `copyToClipboard()` functions, handling success ("Copied" for 1400ms) and failure ("Failed" for 1400ms) states
    - _Requirements: 2.1, 2.2, 2.3, 2.4_
  - [x] 4.2 Create `docs-site/src/scripts/trust-graph.ts` extracting the trust graph canvas renderer from `index.astro` inline script, defining named constants for all layout values (padding, axis ranges, grid steps, font sizes, colors), computing cheat payoff as `fee - stake` and ratio as `stake / fee`, adding text labels for "cheat profitable" and "cheating costs money" zones, and displaying numeric payoff value adjacent to the current position dot
    - _Requirements: 1.1, 7.1, 17.1, 17.2, 17.3, 17.4, 17.5, 17.6, 13.1, 13.2_
  - [x] 4.3 Update `docs-site/src/pages/index.astro` to import and call `initCopyButtons()` from `clipboard.ts` and `initTrustGraph()` from `trust-graph.ts`, removing all inline `<script>` blocks; update the hero prompt copy button to use `data-copy` attribute with the shared clipboard module; add `role="img"` and `aria-label` to the trust graph canvas
    - _Requirements: 1.2, 2.5, 5.1, 1.4_
  - [x] 4.4 Write unit tests for `clipboard.ts` verifying success/failure text states and reset timing
    - _Requirements: 2.3, 2.4_
  - [x] 4.5 Write unit tests for `trust-graph.ts` verifying `computeCheatPayoff` and `computeRatio` math functions
    - _Requirements: 17.1, 17.2, 17.3, 17.4, 17.5, 17.6_

- [x] 5. Extract demo page scripts to external modules
  - [x] 5.1 Create `docs-site/src/scripts/demo/steps.ts` with the `Step`, `BoardNode`, `BoardArrow`, `BoardNote`, `TerminalLine` TypeScript interfaces and the `STEPS` data array extracted from `demo.astro`
    - _Requirements: 1.3, 11.1_
  - [x] 5.2 Create `docs-site/src/scripts/demo/whiteboard.ts` extracting the whiteboard canvas renderer, defining named constants for node radii, arrow sizes, animation durations, grid spacing, chalk-style stroke parameters, and colors; adding text labels inside/adjacent to node circles for accessibility
    - _Requirements: 1.3, 7.2, 13.3_
  - [x] 5.3 Create `docs-site/src/scripts/demo/terminal.ts` extracting the terminal typing animation logic with `createTerminalAnimator()` returning `animate()`, `skip()`, `isTyping()`, and `destroy()` methods
    - _Requirements: 1.3_
  - [x] 5.4 Create `docs-site/src/scripts/demo/index.ts` as the demo page entry point that initializes whiteboard, terminal, step navigation, keyboard shortcuts (Right Arrow → next, Left Arrow → prev, Escape → skip animation), and pip indicators
    - _Requirements: 1.3, 6.1, 6.2, 6.3, 6.4_
  - [x] 5.5 Update `docs-site/src/pages/demo.astro` to import `demo/index.ts` module, remove all inline `<script>` blocks, add `role="img"` and `aria-label` to the demo canvas, ensure `aria-label` updates on step change, and make nav buttons focusable via Enter/Space
    - _Requirements: 1.3, 1.4, 5.2, 5.3, 6.4_
  - [x] 5.6 Write unit tests for `demo/terminal.ts` verifying skip behavior and typing state management
    - _Requirements: 6.3_

- [x] 6. Fix ThemeProvider and add TypeScript component props
  - [x] 6.1 Update `docs-site/src/components/ThemeProvider.astro` to read system preference via `window.matchMedia('(prefers-color-scheme: dark)')` when no stored override exists, set `document.documentElement.dataset.theme` and `style.colorScheme` to the resolved value, and listen for `matchMedia` changes to update the theme dynamically
    - _Requirements: 4.1, 4.2, 4.3, 4.4, 4.5_
  - [x] 6.2 Verify `SiteHeader.astro` Props interface types `currentPath` as `string` and `embedded` as `boolean`; verify `SiteFooter.astro` Props interface types `compact` as `boolean`; add or correct any missing type annotations
    - _Requirements: 11.1, 11.2, 11.3_

- [x] 7. Add canvas error handling across all canvas modules
  - [x] 7.1 Add null-check guards for 2D context in `trust-graph.ts` (skip rendering if context is null), `demo/whiteboard.ts` (skip rendering, display lesson content), and all other canvas modules; wrap `requestAnimationFrame` draw cycles in try/catch that logs errors and continues the animation loop
    - _Requirements: 3.1, 3.2, 3.3_

- [x] 8. Checkpoint — Verify script extraction and theme fix
  - Ensure the site builds without errors. Verify landing page, demo page, and 404 page render correctly. Ensure all tests pass, ask the user if questions arise.

- [x] 9. Refine landing page layout and standardize component styling
  - [x] 9.1 Update `docs-site/src/styles/index-page.css` to enforce typographic scale with three distinct heading levels (h1, section title, card heading) using Design Tokens, add `max-width` constraint on hero h1 to prevent lines exceeding ~18 words, ensure consistent vertical rhythm via `--froglet-section-space`, and collapse multi-column grids to single column below 900px
    - _Requirements: 14.1, 14.2, 14.3, 14.6_
  - [x] 9.2 Update compare cards in `index.astro` to render at equal height with footer content aligned to bottom; update service grid cards to use consistent padding and uniform minimum height
    - _Requirements: 14.4, 14.5_
  - [x] 9.3 Update section labels, descriptions, integration badges, trust graph wrap, and divider elements in `index-page.css` to use consistent Design Token values for font size, weight, letter-spacing, max-width (690px for descriptions), gap spacing, and border colors
    - _Requirements: 16.1, 16.2, 16.3, 16.4, 16.5_
  - [x] 9.4 Replace hard-coded HSL values in `.tag` badge colors (purple, blue, cyan, green, yellow) with Design Token custom properties defined in `tokens.css`
    - _Requirements: 15.5_

- [x] 10. Extract and verify economics page profit chart
  - [x] 10.1 Create `docs-site/src/scripts/profit-chart.ts` extracting the profit chart canvas renderer from `economics.mdx`, computing provider payoff as `baseFee + q * successFee - cost` and requester payoff as `q * value - baseFee - q * successFee`, computing break-even threshold as `max(0, (cost - baseFee) / successFee)` when successFee > 0 or "never" when successFee = 0 and baseFee < cost
    - _Requirements: 18.4, 18.5, 18.6_
  - [x] 10.2 Update `docs-site/src/content/docs/learn/economics.mdx` to import `profit-chart.ts` module, remove the inline `<script>` block, and add canvas error handling with fallback
    - _Requirements: 1.1, 3.3_
  - [x] 10.3 Write unit tests for `profit-chart.ts` verifying `computeProviderPayoff` and `computeRequesterPayoff` math functions against the formal model
    - _Requirements: 18.4, 18.5, 18.6_

- [x] 11. Extract and verify settlement visualization
  - [x] 11.1 Create `docs-site/src/scripts/settlement-viz.ts` extracting the settlement outcome canvas from `settlement.mdx`, verifying Success scenario displays requester outflow = `B + F` and provider inflow = `B + F`, Failure scenario displays requester outflow = `B` and provider inflow = `B` with success fee canceled, and Free scenario displays both as zero
    - _Requirements: 18.1, 18.2, 18.3_
  - [x] 11.2 Update `docs-site/src/content/docs/learn/settlement.mdx` to import `settlement-viz.ts` module, remove the inline `<script>` block, and add canvas error handling
    - _Requirements: 1.1, 3.3_

- [x] 12. Verify demo page economic model consistency
  - [x] 12.1 Audit `docs-site/src/scripts/demo/steps.ts` settlement step to ensure base fee + success fee = total deal price in lesson card, safety ratio computed as `total_staked_msat / deal_value_msat`, equilibrium step shows honest strategy preserves stake while cheat loses it, artifact chain step lists exactly six types (descriptor, offer, quote, deal, invoice_bundle, receipt) with SHA-256 hash references, and settlement step states base fee locks on acceptance and success fee settles on success
    - _Requirements: 19.1, 19.2, 19.3, 19.4, 19.5_

- [x] 13. Checkpoint — Verify all existing pages render correctly with math verified
  - Ensure the site builds without errors. Ensure all tests pass, ask the user if questions arise.

- [x] 14. Add interactive settlement calculator
  - [x] 14.1 Create `docs-site/src/scripts/settlement-calculator.ts` implementing `computeSettlementOutcomes()` for three scenarios (success, failure, free), `computeBreakEvenThreshold()` as `max(0, (cost - baseFee) / successFee)` clamped to [0,1], `isProviderAtLoss()` check, and `initSettlementCalculator()` with input controls for base fee, success fee, and cost in msat
    - _Requirements: 21.1, 21.2, 21.3, 21.4, 21.5_
  - [x] 14.2 Add the settlement calculator widget to `docs-site/src/content/docs/learn/settlement.mdx` with HTML container and script import, including canvas error handling fallback
    - _Requirements: 21.1, 20.5_
  - [x] 14.3 Write unit tests for `settlement-calculator.ts` verifying all three outcome computations, break-even threshold, and loss warning logic
    - _Requirements: 21.2, 21.3, 21.4, 21.5_

- [x] 15. Add interactive deal flow visualization
  - [x] 15.1 Create `docs-site/src/scripts/chain-canvas.ts` rendering the six artifact types (descriptor, offer, quote, deal, invoice_bundle, receipt) as a connected chain, highlighting selected artifact with signer/purpose/hash-link details, animating transitions between artifacts, and indicating provider vs requester signing with text labels and distinct visual styling
    - _Requirements: 20.1, 20.2, 20.3, 20.4_
  - [x] 15.2 Update `docs-site/src/content/docs/learn/deal-flow.mdx` to use the `chain-canvas.ts` module for the existing chain canvas, adding a static HTML fallback if canvas context cannot be obtained, and removing the inline `<script>` block
    - _Requirements: 20.5, 1.1_

- [x] 16. Add animated diagrams to learn pages
  - [x] 16.1 Create `docs-site/src/scripts/identity-diagram.ts` implementing an interactive diagram showing keypair generation and the relationship between private key, public key, and node identity; add to `identity.mdx` with canvas error handling and static fallback
    - _Requirements: 22.1, 22.6_
  - [x] 16.2 Create `docs-site/src/scripts/deal-flow-sequence.ts` implementing an animated sequence diagram showing the message exchange between requester and provider (request quote → signed quote → signed deal → execute + receipt) with pause/play control; add to `deal-flow.mdx`
    - _Requirements: 22.2, 22.4_
  - [x] 16.3 Create `docs-site/src/scripts/trust-threshold.ts` implementing an interactive trust threshold diagram where the user sets risk threshold k and sees which stake-to-deal-value ratios pass `stake / deal_value > k`, with real-time value display as inputs change; add to `economics.mdx`
    - _Requirements: 22.3, 22.5_
  - [x] 16.4 Ensure all new diagram canvases have `role="img"` and descriptive `aria-label` attributes, and fall back to static HTML if canvas context is unavailable
    - _Requirements: 22.6, 5.1_

- [x] 17. Final checkpoint — Full build and verification
  - Ensure the site builds without errors. Verify all pages render correctly, all interactive elements function, and all canvas modules handle errors gracefully. Ensure all tests pass, ask the user if questions arise.

## Notes

- Tasks marked with `*` are optional and can be skipped for faster MVP
- Each task references specific requirements for traceability
- Checkpoints ensure incremental validation after each major phase
- All scripts use TypeScript; all styles use CSS with Design Tokens
- The `docs/KERNEL.md` specification is read-only reference material for mathematical verification
