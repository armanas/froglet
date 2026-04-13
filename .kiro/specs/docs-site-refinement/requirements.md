# Requirements Document

## Introduction

The Froglet documentation site (`docs-site/`) is an Astro 6 + Starlight 0.38.2 site that serves as the primary documentation and marketing surface for the Froglet protocol. The current implementation has accumulated technical debt: monolithic CSS, large inline scripts, duplicated logic, accessibility gaps, and a ThemeProvider that forces dark mode despite light mode tokens being defined. Beyond technical debt, the site needs a more polished visual design with conventional layout patterns, mathematical correctness verification for all interactive economic explanations, and additional interactive elements to make protocol concepts more engaging. This refinement effort aims to make the codebase isomorphic, elegant, accessible, mathematically sound, and maintainable.

## Glossary

- **Docs_Site**: The Astro 6 + Starlight 0.38.2 documentation website located in `docs-site/`.
- **Landing_Page**: The home page at `docs-site/src/pages/index.astro`.
- **Demo_Page**: The interactive walkthrough page at `docs-site/src/pages/demo.astro`.
- **ThemeProvider**: The Astro component at `docs-site/src/components/ThemeProvider.astro` responsible for setting the active color scheme.
- **Trust_Graph_Canvas**: The `<canvas>` element on the Landing_Page that renders the stake-vs-fee cheat payoff chart.
- **Demo_Canvas**: The `<canvas>` element on the Demo_Page that renders the animated whiteboard scene.
- **Copy_Button**: Any button that copies text to the clipboard using the Clipboard API.
- **Custom_CSS**: The monolithic stylesheet at `docs-site/src/styles/custom.css`.
- **Index_Page_CSS**: The landing page stylesheet at `docs-site/src/styles/index-page.css`.
- **Design_Token**: A CSS custom property defined on `:root` or `[data-theme]` that encodes a visual design decision (color, spacing, radius, shadow, font).
- **Magic_Number**: A hard-coded numeric literal in drawing or layout code whose purpose is not self-documenting.
- **Profit_Chart_Canvas**: The `<canvas>` element on the Economics learn page that renders the provider profit vs quality chart.
- **Settlement_Viz_Canvas**: The `<canvas>` element on the Settlement learn page that renders the settlement outcome scenarios.
- **Chain_Canvas**: The `<canvas>` element on the Deal Flow learn page that renders the interactive artifact chain diagram.
- **Deal_Flow_Visualization**: An interactive diagram showing the offer → quote → deal → receipt artifact chain with animated transitions.
- **Settlement_Calculator**: An interactive widget that computes settlement outcomes (requester outflow, provider inflow) from user-supplied base fee, success fee, and cost inputs.
- **Kernel_Spec**: The authoritative protocol specification at `docs/KERNEL.md` defining artifact types, hashing, signing, and settlement methods.

## Requirements

### Requirement 1: Extract Inline Scripts to External Modules

**User Story:** As a developer, I want inline JavaScript extracted into separate module files, so that the codebase is easier to navigate, test, and cache.

#### Acceptance Criteria

1. WHEN the Landing_Page is loaded, THE Docs_Site SHALL execute the Trust_Graph_Canvas rendering logic from an external TypeScript or JavaScript module rather than an inline `<script>` block.
2. WHEN the Landing_Page is loaded, THE Docs_Site SHALL execute the Copy_Button initialization logic from an external module rather than an inline `<script>` block.
3. WHEN the Demo_Page is loaded, THE Docs_Site SHALL execute the whiteboard canvas, terminal animation, and step navigation logic from external modules rather than inline `<script>` blocks.
4. THE Docs_Site SHALL produce identical visual and interactive behavior after script extraction as before extraction.

### Requirement 2: Unify Copy-to-Clipboard Logic

**User Story:** As a developer, I want a single reusable copy-to-clipboard implementation, so that clipboard behavior is consistent and maintained in one place.

#### Acceptance Criteria

1. THE Docs_Site SHALL provide a single shared module that implements copy-to-clipboard functionality.
2. WHEN a Copy_Button is clicked, THE shared clipboard module SHALL copy the text specified by the button's `data-copy` attribute to the system clipboard.
3. WHEN the clipboard write succeeds, THE shared clipboard module SHALL update the Copy_Button text to "Copied" for 1400 milliseconds, then restore the original text.
4. IF the clipboard write fails, THEN THE shared clipboard module SHALL update the Copy_Button text to "Failed" for 1400 milliseconds, then restore the original text.
5. WHEN the Landing_Page hero prompt copy button is clicked, THE shared clipboard module SHALL copy the prompt text to the system clipboard using the same shared implementation.

### Requirement 3: Add Canvas Error Handling

**User Story:** As a developer, I want canvas rendering to handle errors gracefully, so that a drawing failure does not break the rest of the page.

#### Acceptance Criteria

1. IF the Trust_Graph_Canvas 2D context cannot be obtained, THEN THE Landing_Page SHALL skip canvas rendering without throwing an uncaught exception.
2. IF the Demo_Canvas 2D context cannot be obtained, THEN THE Demo_Page SHALL skip canvas rendering and display the lesson content without throwing an uncaught exception.
3. IF an error occurs during a canvas `requestAnimationFrame` draw cycle, THEN THE Docs_Site SHALL catch the error, log it to the console, and continue operating without crashing the animation loop.

### Requirement 4: Fix ThemeProvider to Respect System Preference

**User Story:** As a user, I want the site to respect my operating system color scheme preference, so that I see the theme I expect rather than being forced into dark mode.

#### Acceptance Criteria

1. WHEN no explicit theme override is stored, THE ThemeProvider SHALL read the user's system color scheme preference via `prefers-color-scheme` and apply the matching theme (`dark` or `light`).
2. WHEN the system color scheme preference changes, THE ThemeProvider SHALL update the active theme to match the new preference.
3. THE ThemeProvider SHALL set `document.documentElement.dataset.theme` and `document.documentElement.style.colorScheme` to the resolved theme value.
4. WHILE the `[data-theme='dark']` attribute is set, THE Docs_Site SHALL apply the dark mode Design_Token values defined in Custom_CSS.
5. WHILE no `[data-theme]` attribute is set or `[data-theme='light']` is set, THE Docs_Site SHALL apply the light mode (`:root`) Design_Token values defined in Custom_CSS.

### Requirement 5: Improve Canvas Accessibility

**User Story:** As a user who relies on assistive technology, I want canvas elements to have appropriate ARIA attributes, so that screen readers convey the purpose of each canvas.

#### Acceptance Criteria

1. THE Trust_Graph_Canvas element SHALL have `role="img"` and an `aria-label` attribute that describes the chart (e.g., "Cheat payoff chart: stake versus fee ratio").
2. THE Demo_Canvas element SHALL have `role="img"` and an `aria-label` attribute that describes the scene (e.g., "Animated whiteboard illustrating the current protocol step").
3. WHEN the Demo_Page step changes, THE Demo_Page SHALL update the Demo_Canvas `aria-label` to reflect the current step title.

### Requirement 6: Add Keyboard Navigation to Demo Page

**User Story:** As a user who navigates with a keyboard, I want to control the demo walkthrough using keyboard shortcuts, so that I can progress through steps without a mouse.

#### Acceptance Criteria

1. WHEN the user presses the Right Arrow key on the Demo_Page, THE Demo_Page SHALL advance to the next step.
2. WHEN the user presses the Left Arrow key on the Demo_Page, THE Demo_Page SHALL return to the previous step.
3. WHEN the user presses the Escape key during a terminal typing animation, THE Demo_Page SHALL skip the animation and display the final terminal output immediately.
4. THE Demo_Page navigation buttons SHALL be focusable and operable via Enter and Space keys.

### Requirement 7: Eliminate Magic Numbers in Canvas Drawing Code

**User Story:** As a developer, I want named constants instead of magic numbers in canvas code, so that the drawing logic is self-documenting and tunable.

#### Acceptance Criteria

1. THE Trust_Graph_Canvas module SHALL define named constants for all layout values including padding, axis ranges, grid step thresholds, font sizes, and color values.
2. THE Demo_Canvas module SHALL define named constants for node radii, arrow sizes, animation durations, grid spacing, and chalk-style stroke parameters.
3. THE Docs_Site SHALL produce identical visual output after replacing magic numbers with named constants.

### Requirement 8: Split Monolithic CSS into Modular Files

**User Story:** As a developer, I want CSS organized by concern, so that styles are easier to find, modify, and reason about.

#### Acceptance Criteria

1. THE Docs_Site SHALL organize Design_Token definitions (`:root` and `[data-theme='dark']` custom properties) in a dedicated tokens file.
2. THE Docs_Site SHALL organize Starlight theme overrides (sidebar, header, content panel, pagination, right sidebar, markdown content styles) in a dedicated Starlight overrides file.
3. THE Docs_Site SHALL organize reusable component styles (`.learn-card`, `.learn-button`, `.learn-grid`, `.learn-hero`, `.learn-code-box`, `.plot-shell`, `.protocol-chain`, etc.) in a dedicated components file.
4. THE Docs_Site SHALL maintain a root CSS entry point that imports all partials in the correct cascade order.
5. THE Docs_Site SHALL produce identical visual rendering after the CSS split as before the split.

### Requirement 9: Extract Inline Styles from Demo Page

**User Story:** As a developer, I want demo page styles in an external stylesheet, so that they follow the same pattern as the rest of the site.

#### Acceptance Criteria

1. THE Demo_Page SHALL load its styles from an external CSS file rather than an inline `<style is:inline>` block.
2. THE Demo_Page SHALL produce identical visual rendering after style extraction as before extraction.
3. THE Demo_Page external stylesheet SHALL reference Design_Tokens from the shared token definitions rather than re-declaring shorthand aliases.

### Requirement 10: Extract Inline Styles from 404 Page

**User Story:** As a developer, I want 404 page styles in an external stylesheet, so that all pages follow a consistent style organization pattern.

#### Acceptance Criteria

1. THE 404 page SHALL load its styles from an external CSS file rather than an inline `<style is:inline>` block.
2. THE 404 page SHALL produce identical visual rendering after style extraction as before extraction.
3. THE 404 page external stylesheet SHALL reference Design_Tokens from the shared token definitions rather than re-declaring shorthand aliases.

### Requirement 11: Add TypeScript Interfaces for Component Props

**User Story:** As a developer, I want all Astro component props typed with TypeScript interfaces, so that the component API is self-documenting and type-checked.

#### Acceptance Criteria

1. THE SiteHeader component SHALL export a TypeScript `Props` interface that types `currentPath` as `string` and `embedded` as `boolean`.
2. THE SiteFooter component SHALL export a TypeScript `Props` interface that types `compact` as `boolean`.
3. WHEN a component is used with incorrect prop types, THE Astro build SHALL report a type error.

### Requirement 12: Remove Duplicate Font Loading

**User Story:** As a developer, I want fonts loaded once through the Starlight head configuration, so that pages do not issue redundant font requests.

#### Acceptance Criteria

1. THE Demo_Page SHALL load the Inter and JetBrains Mono fonts via the shared Starlight head configuration rather than a separate inline `@import` statement.
2. THE 404 page SHALL load fonts via the shared Starlight head configuration rather than a separate inline `@import` statement.
3. THE Docs_Site SHALL issue at most one set of Google Fonts requests per page load for the Inter and JetBrains Mono font families.

### Requirement 13: Ensure Color Contrast Beyond Color Alone

**User Story:** As a user with color vision deficiency, I want interactive canvas elements to use shape or text labels in addition to color, so that information is not conveyed by color alone.

#### Acceptance Criteria

1. WHEN the Trust_Graph_Canvas renders the "cheat profitable" and "cheating costs money" zones, THE Trust_Graph_Canvas SHALL differentiate the zones using text labels in addition to color fill.
2. WHEN the Trust_Graph_Canvas renders the current position dot, THE Trust_Graph_Canvas SHALL display the numeric payoff value as a text label adjacent to the dot.
3. THE Demo_Canvas node circles SHALL display their text label inside or adjacent to the circle, ensuring the node identity is conveyed by text and not solely by color.

### Requirement 14: Refine Landing Page Layout and Visual Hierarchy

**User Story:** As a visitor, I want the landing page to follow conventional, polished web design patterns, so that the site feels professional and easy to scan.

#### Acceptance Criteria

1. THE Landing_Page hero section SHALL use a typographic scale with a minimum of three distinct heading levels (h1, section title, card heading) that decrease in size by a consistent ratio defined via Design_Tokens.
2. THE Landing_Page sections SHALL maintain consistent vertical rhythm using the `--froglet-section-space` token, with each section separated by equal spacing.
3. THE Landing_Page hero h1 SHALL have a maximum width constraint that prevents lines from exceeding 18 words, ensuring readable line lengths.
4. THE Landing_Page compare cards (Hosted node, Local install, Docker Compose) SHALL render at equal height within their row, with footer content aligned to the bottom of each card.
5. THE Landing_Page service grid cards SHALL use consistent internal padding and a uniform minimum height so that cards in the same row align visually.
6. WHEN the viewport width is below 900px, THE Landing_Page SHALL collapse multi-column grids to a single column while preserving the vertical rhythm and spacing tokens.

### Requirement 15: Standardize Component Styling Patterns

**User Story:** As a developer, I want card, button, and grid components to follow a single consistent design pattern, so that the visual language is cohesive across all pages.

#### Acceptance Criteria

1. THE Docs_Site SHALL define a single card pattern (border, radius, padding, shadow, background) using Design_Tokens, applied consistently to `.learn-card`, `.gc`, `.compare-card`, `.plot-shell`, and `.chapter-card` elements.
2. THE Docs_Site SHALL define a single button pattern (border-radius, padding, font-weight, transition) using Design_Tokens, applied consistently to `.learn-button`, `.btn`, `.btn-ghost`, `.plot-button`, and Copy_Button elements.
3. THE Docs_Site SHALL define grid gap values using Design_Tokens rather than hard-coded pixel values, applied consistently to `.learn-grid`, `.grid`, `.learn-sequence`, `.protocol-chain`, and `.chapter-grid` elements.
4. WHEN a card is hovered, THE Docs_Site SHALL apply a consistent hover treatment (border-color change and subtle translateY shift) across all card variants.
5. THE Landing_Page `.tag` badge colors (purple, blue, cyan, green, yellow) SHALL be defined as Design_Tokens rather than inline utility classes with hard-coded HSL values.

### Requirement 16: Improve Landing Page Section Design

**User Story:** As a visitor, I want each landing page section to have clear visual boundaries and a scannable layout, so that I can quickly understand what Froglet offers.

#### Acceptance Criteria

1. THE Landing_Page section labels (Install, Services, Integrations, Game Theory) SHALL use the same font size, weight, letter-spacing, and color defined by the `.section-label` pattern.
2. THE Landing_Page section descriptions SHALL have a maximum width of 690px to maintain readable line lengths.
3. THE Landing_Page integration badges row SHALL wrap gracefully on narrow viewports, maintaining consistent gap spacing between badges.
4. THE Landing_Page trust graph section SHALL visually contain the slider controls, canvas, and explanatory note within a single card boundary with consistent internal padding.
5. THE Landing_Page divider elements SHALL use the `--froglet-border` token color and span the full shell width consistently.

### Requirement 17: Verify Trust Graph Mathematical Correctness

**User Story:** As a visitor reading the economics explanation, I want the trust graph to display mathematically correct values, so that I can trust the protocol claims.

#### Acceptance Criteria

1. WHEN the Trust_Graph_Canvas renders with stake S and fee F, THE Trust_Graph_Canvas SHALL compute cheat payoff as exactly `F - S` (the provider keeps the fee but loses the stake).
2. WHEN the Trust_Graph_Canvas renders with stake S and fee F, THE Trust_Graph_Canvas SHALL compute the stake-to-fee ratio as exactly `S / F`.
3. WHEN the stake-to-fee ratio equals 1.0, THE Trust_Graph_Canvas SHALL plot the cheat payoff at exactly zero on the Y axis.
4. WHEN the stake-to-fee ratio is greater than 1.0, THE Trust_Graph_Canvas SHALL plot the cheat payoff below zero (negative), indicating cheating is irrational.
5. WHEN the stake-to-fee ratio is less than 1.0, THE Trust_Graph_Canvas SHALL plot the cheat payoff above zero (positive), indicating cheating is profitable.
6. THE Trust_Graph_Canvas cheat payoff line SHALL follow the linear function `payoff(r) = F × (1 - r)` where `r = S / F`, matching the formula displayed in the chart label.

### Requirement 18: Verify Settlement Fee Mathematical Consistency

**User Story:** As a visitor reading the settlement explanation, I want the fee examples and interactive charts to be mathematically consistent with the formal model, so that the numbers add up correctly.

#### Acceptance Criteria

1. WHEN the Settlement_Viz_Canvas renders the "Success" scenario with base fee B and success fee F, THE Settlement_Viz_Canvas SHALL display requester outflow as exactly `B + F` and provider inflow as exactly `B + F`.
2. WHEN the Settlement_Viz_Canvas renders the "Provider fails" scenario, THE Settlement_Viz_Canvas SHALL display requester outflow as exactly B (base fee only) and provider inflow as exactly B, with the success fee shown as canceled.
3. WHEN the Settlement_Viz_Canvas renders the "Free service" scenario, THE Settlement_Viz_Canvas SHALL display both requester outflow and provider inflow as zero.
4. WHEN the Profit_Chart_Canvas renders with base fee B, success fee F, and cost C, THE Profit_Chart_Canvas SHALL compute provider payoff at quality q as exactly `B + q × F - C`, matching the formal model `E[π_P] = b_s + q·f_s - c`.
5. WHEN the Profit_Chart_Canvas renders with base fee B, success fee F, and cost C, THE Profit_Chart_Canvas SHALL compute requester payoff at quality q as exactly `q × V - B - q × F` where V is the fixed requester value, matching the formal model `E[π_R] = q·v - b_s - q·f_s`.
6. WHEN the Profit_Chart_Canvas renders, THE Profit_Chart_Canvas SHALL compute the break-even quality threshold as `max(0, (C - B) / F)` when F is greater than zero, and display "never" when F equals zero and B is less than C.

### Requirement 19: Verify Demo Page Economic Model Consistency

**User Story:** As a visitor watching the demo walkthrough, I want the economic examples shown in the terminal and lesson cards to match the formal protocol specification, so that the demo teaches correct concepts.

#### Acceptance Criteria

1. WHEN the Demo_Page displays the settlement step, THE Demo_Page terminal output SHALL show base fee and success fee values that sum to the total deal price shown in the lesson card.
2. WHEN the Demo_Page displays the staked reputation step, THE Demo_Page SHALL compute the safety ratio as `total_staked_msat / deal_value_msat`, matching the formal trust model definition.
3. WHEN the Demo_Page displays the equilibrium step, THE Demo_Page terminal output SHALL show that the honest strategy preserves the stake while the cheat strategy loses the stake, consistent with the game-theoretic payoff table.
4. THE Demo_Page artifact chain step SHALL present exactly six artifact types (descriptor, offer, quote, deal, invoice_bundle, receipt) matching the Kernel_Spec, with each artifact referencing the previous by SHA-256 hash.
5. THE Demo_Page settlement step SHALL state that the base fee locks on deal acceptance and the success fee settles on execution success, matching the two-leg settlement model defined in the Kernel_Spec.

### Requirement 20: Add Interactive Deal Flow Visualization

**User Story:** As a visitor, I want an interactive visualization of the deal flow lifecycle, so that I can see how artifacts chain together step by step.

#### Acceptance Criteria

1. THE Deal Flow learn page SHALL render an interactive Deal_Flow_Visualization that displays the six artifact types (descriptor, offer, quote, deal, invoice_bundle, receipt) as a connected chain.
2. WHEN the user clicks an artifact node in the Deal_Flow_Visualization, THE Deal_Flow_Visualization SHALL highlight the selected artifact and display its signer, purpose, and hash-link relationship to the previous artifact.
3. WHEN the user navigates between artifacts, THE Deal_Flow_Visualization SHALL animate the transition to show the directional flow from one artifact to the next.
4. THE Deal_Flow_Visualization SHALL indicate which artifacts are signed by the provider and which by the requester, using both text labels and distinct visual styling.
5. IF the Deal_Flow_Visualization canvas context cannot be obtained, THEN THE Deal Flow learn page SHALL fall back to a static HTML representation of the chain.

### Requirement 21: Add Interactive Settlement Calculator

**User Story:** As a visitor, I want an interactive calculator where I can input fee values and see settlement outcomes, so that I can understand how the two-leg model works with my own numbers.

#### Acceptance Criteria

1. THE Settlement learn page SHALL provide a Settlement_Calculator with input controls for base fee (in msat), success fee (in msat), and provider cost (in msat).
2. WHEN the user adjusts Settlement_Calculator inputs, THE Settlement_Calculator SHALL compute and display the three settlement outcomes (success, failure, free service) with requester outflow and provider inflow for each.
3. THE Settlement_Calculator SHALL compute provider profit at success as `base_fee + success_fee - cost` and provider profit at failure as `base_fee - cost`.
4. THE Settlement_Calculator SHALL compute the break-even quality threshold as `max(0, (cost - base_fee) / success_fee)` when success fee is greater than zero.
5. IF the user enters a base fee plus success fee that is less than the provider cost, THEN THE Settlement_Calculator SHALL display a warning indicating the provider would operate at a loss at full quality.

### Requirement 22: Add Animated Diagrams to Learn Pages

**User Story:** As a visitor reading the learn pages, I want animated and interactive diagrams that illustrate protocol concepts, so that complex ideas are easier to understand visually.

#### Acceptance Criteria

1. THE Identity learn page SHALL include an interactive diagram that shows keypair generation and the relationship between private key, public key, and node identity.
2. THE Deal Flow learn page SHALL include an animated sequence diagram showing the message exchange between requester and provider (request quote → signed quote → signed deal → execute + receipt).
3. THE Economics learn page SHALL include an interactive trust threshold diagram where the user can set a risk threshold k and see which stake-to-deal-value ratios pass the safety check `stake / deal_value > k`.
4. WHEN an animated diagram is playing, THE learn page SHALL provide a pause/play control so the user can stop and inspect the current state.
5. WHEN an interactive diagram includes slider or input controls, THE learn page SHALL display the current computed values in real time as the user adjusts inputs.
6. IF a diagram canvas context cannot be obtained, THEN THE learn page SHALL display a static fallback image or HTML representation of the concept.
