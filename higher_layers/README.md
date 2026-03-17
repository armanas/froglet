# Higher-Layer Planning

This directory is a staging area for product-layer work that is intentionally
outside the frozen Froglet kernel and bot/runtime core.

It exists inside this repo for now so marketplace and addon ideas can be
aligned before they are moved into separate repositories.

Boundary rules:

- Do not widen `SPEC.md` for work tracked here unless there is a clear kernel
  interoperability requirement.
- Treat Froglet as the source of signed artifacts, not as the home for
  marketplace policy, ranking, ownership profiles, or exchange logic.
- Prefer consuming public Froglet APIs and signed artifacts over coupling to
  private runtime internals or direct SQLite reads.
- Assume everything here is portable and should be easy to extract later.

Current topics:

- [MARKETPLACE.md](MARKETPLACE.md): service split and scope for discovery,
  indexing, catalog, broker, and reputation layers
- [EXECUTION_PLAN.md](EXECUTION_PLAN.md): phased implementation order for the
  open core, open integration tooling, and closed higher-layer services
- [REPO_STRATEGY.md](REPO_STRATEGY.md): public/private repo split, license
  position, and temporary ignored `private/` incubation rules
- [OWNERSHIP.md](OWNERSHIP.md): ownership and issuer-model notes that do not
  require kernel changes today
- [CHECKLIST.md](CHECKLIST.md): separate execution checklist for marketplace and
  addon work
- [DECISIONS.md](DECISIONS.md): decisions already made about repo boundaries
  and higher-layer services

Related core docs:

- [../docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md)
- [../docs/IMPLEMENTATION_CHECKLIST.md](../docs/IMPLEMENTATION_CHECKLIST.md)
- [../docs/REMOTE_AGENT_LAYER.md](../docs/REMOTE_AGENT_LAYER.md)
