This directory is for temporary local incubation of private higher-layer work.

Tracked files in this directory are limited to layout markers and this note.
Actual implementation files under `private/` are ignored by Git.

Use it only for work that is intentionally outside the public Froglet core,
such as:

- official marketplace product code
- indexers and catalog projections
- broker and routing logic
- reputation and ranking systems
- ownership / issuer services
- closed operational tooling around those services

Rules:

- do not make public code depend on anything in `private/`
- consume only public Froglet APIs, signed artifacts, or documented external
  contracts
- do not read Froglet SQLite state directly as a shortcut
- document any new public contract in `higher_layers/` before relying on it
- move each service into its own repo once the interface and ownership are
  stable
