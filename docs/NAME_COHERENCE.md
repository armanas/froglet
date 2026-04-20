# Name and Registry Coherence Check

Status: **completed** as of 2026-04-17.

This is the lightweight name-coherence check for Froglet. Froglet is an
open-source protocol name, so the goal is limited to: (a) avoid stepping on a
clearly-conflicting existing software name, and (b) make sure the protocol
does not collide with itself across package registries.

---

## Registry status

### ✅ Free to take

| Registry | Name | Status | Action |
|---|---|---|---|
| **crates.io** | `froglet` | Not published (404 at `crates.io/api/v1/crates/froglet`). | **Register to lock.** Requires your crates.io API token. `cargo publish` from the workspace root once the package metadata is clean. |
| **npm** | `froglet` (unscoped) | Not published (404 at `registry.npmjs.org/froglet`). | **Register to lock.** Requires your npm account. A placeholder package that redirects users to the real repo is enough; `npm publish` with minimal metadata. |
| **RubyGems** | `froglet` | Not published (404 at `rubygems.org/api/v1/gems/froglet.json`). | **Optional lock.** Low risk of conflict — we have no Ruby surface planned. |
| **Packagist** | `froglet/froglet` | Not published (404). | **Optional lock.** No PHP surface planned. |
| **Snapcraft** | `froglet` | Not published (404 at `snapcraft.io/froglet`). | **Optional lock.** Only if snaps become a distribution target. |
| **Homebrew** | `froglet` | No formula. Homebrew doesn't pre-reserve names; formulas get reviewed and merged via PR to `homebrew-core` when a real release ships. | No action required until release. |

### ⚠️ Conflict — name taken by an unrelated project

| Registry | Name | Conflict | Our recommendation |
|---|---|---|---|
| **PyPI** | `froglet` | Taken since 2016 by `hslatman` — a Python client for the Frog NLP server (a Dutch-language NLP tool from Radboud University). Last release: `v0.3`, January 2016. Effectively abandoned but held. | Use `froglet-protocol` on PyPI if we ever need a Python package. Document the aliasing in the Python client docs so users don't land on the wrong package. |
| **npm** | `@froglet` scope | Taken. `jryanconklin` published `@froglet/ui` (an accessible React component library) on 2026-02-24. The scope is effectively owned; fresh activity in 2026 means this is an active, unrelated project. | **Do not publish under `@froglet/*`.** Use unscoped `froglet-*` names (`froglet-mcp`, `froglet-shared-lib`, etc.) or register a different scope like `@frogletdev` or `@froglet-protocol`. Current repo state is fine — all `@froglet/*` package names in `integrations/` are marked `"private": true` and never publish. |

### ⚠️ Squatter-held — no active content

| Registry | Name | Status | Our recommendation |
|---|---|---|---|
| **GitHub user** `github.com/froglet` | Exists, 0 public repos. Old squat or passive hold. | We operate as `armanas/froglet`. Cannot claim `github.com/froglet` without contacting the account holder. **Acceptable**: the README, docs-site, and docs all point at `armanas/froglet`; no confusion. Could optionally register `froglet-dev`, `froglet-protocol`, or `frogletdev` as a GitHub organization if we want to move the repo later. |
| **Docker Hub user** `hub.docker.com/u/froglet` | Exists, 0 repositories. Squatter. | We already publish to `ghcr.io/armanas/froglet-*`. Docker Hub's `froglet` namespace is not needed for the MVP. If we ever want mirrors on Docker Hub, use `armanas/froglet-*` there too, matching GHCR. |

---

## Software-space conflict review

Short web search for "froglet" in programming / protocol / framework context.
Listed so operators + users can recognize these are distinct from this project.

| Project | What it is | Conflict level |
|---|---|---|
| **Frog / Froglet (hslatman)** | A Python client for the Dutch NLP server **Frog** (Language Machines, Radboud). Abandoned since 2016. Holds the PyPI name. | Thematic overlap minimal — different domain (NLP tooling), dormant, not likely to be confused in search. |
| **Forge Froglet** | `#lang forge/froglet` — a beginner language-level in Brown University's **Forge** formal-methods teaching tool. Academic, not distributed standalone. | Thematic overlap very low — formal methods teaching, niche academic. Different surface (Racket `#lang`). |
| **Frog Protocols** | A Wayland display-server protocol project by `misyltoad` for faster-iteration Wayland extensions (`frog-fifo-v1` etc.). | Different name (`frog-protocols`, not `froglet`), different domain (Wayland / Linux graphics). No conflict. |
| **froglet-studio** (GitHub org) | Unclear — appears to be a game-related organization. | Different domain (games). No conflict. |
| **ToxicVillage/Froglet** | A cryptocurrency meme coin repo. | Not a software project per se; low overlap. |
| **Joshua-Ashton/d9vk release "Froglet"** | A one-off release codename for the D9VK (D3D9 → Vulkan) translation layer. | Not a project name, just a release codename. No conflict. |
| **munin/froglet** | Old PHP project, minimal activity. | No conflict. |

None of these are in the agent commerce / protocol / signed-economy space.
The name "Froglet" in that domain is effectively unused.

---

## Trademark (USPTO) — requires manual check

The USPTO TESS search interface is not machine-queryable (it is
session-based and requires a live browser). Run this manually to complete
the basic software-class trademark check:

1. Open [https://tmsearch.uspto.gov/search/search-information](https://tmsearch.uspto.gov/search/search-information).
2. Enter `froglet` as the search term, keyword search.
3. Check for **active** (not dead / cancelled) registrations in:
   - Class 9 (software, downloadable software, SaaS).
   - Class 42 (software design, SaaS, cloud computing).
4. If any active registration in those classes covers something close to
   "protocol / agent commerce / execution / settlement", flag it here.

Expected outcome based on general search: no active software-class
registration for "Froglet" — the word is more commonly associated with the
juvenile-frog biological meaning and the Pokémon character (game
merchandise class 28, not software), neither of which conflict with our
software use.

If the manual TESS search turns up a genuine software-class conflict,
re-evaluate the name before launch. Otherwise proceed.

---

## Recommended next actions

### Register now (lock the open names)

These all require your credentials and should be done before any public
launch. Cost: zero for all of them.

1. **crates.io**: `cargo publish -p froglet-protocol` (or a stub crate if
   the protocol crate isn't ready). Your crates.io API token goes in
   `~/.cargo/credentials.toml`.
2. **npm unscoped `froglet`**: `npm publish` on a minimal stub package
   that points readers at `github.com/armanas/froglet`. Minutes of work.
3. **PyPI `froglet-protocol`** (note: NOT `froglet` — that's taken):
   register the name with a stub package for future Python SDK work.
   `twine upload` with your PyPI account.

### Optional locks

- RubyGems `froglet` and Packagist `froglet/froglet` — only if you plan to
  publish Ruby or PHP clients. Otherwise leave unregistered.
- Snap `froglet` — only if snap becomes a distribution target.
- GitHub organization `frogletdev` — only if you want to move the repo out
  of your personal namespace.

### Accept as-is

- GitHub user `froglet` (squatter, 0 repos). We operate as
  `armanas/froglet`.
- Docker Hub user `froglet` (squatter, 0 repos). We publish to
  `ghcr.io/armanas/*`.
- npm `@froglet/*` scope (taken by unrelated project). We use unscoped
  names or a different scope.
- PyPI `froglet` exact name (abandoned but held). We use `froglet-protocol`
  if/when we publish Python.

---

## Verdict

**Proceed with `Froglet` as the protocol name.**

No active software-class trademark collision surfaced in the basic web
review. No active protocol-space project uses the name. The package-
registry situation is livable: three major registries (crates.io, npm
unscoped, RubyGems) are free to lock; two (PyPI exact, npm `@froglet`
scope) require an alternate name; two squatter-held namespaces (GitHub
user, Docker Hub user) don't actually affect how we ship (we operate under
`armanas/*` and `ghcr.io/armanas/*`).

Human decision to finalize: run the USPTO TESS manual step above. If that
comes back clean, the name decision is locked.

## Revision history

- 2026-04-17: First pass of the name-coherence check. npm + PyPI +
  crates.io + Docker Hub + GitHub + RubyGems + Packagist + Snap +
  Homebrew all checked via registry APIs. Software-space web search
  documented. USPTO step flagged for manual follow-up.
