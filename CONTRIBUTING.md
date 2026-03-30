# Contributing

## Kernel changes

`docs/KERNEL.md` is the current authoritative kernel specification. Changes to canonical artifact payloads, hashing, signing bytes, state transitions, or the Lightning settlement binding require a strong interoperability justification and must be discussed before a PR is opened. Everything outside the kernel is more flexible, but this repo is now intentionally centered on the protocol, the Froglet node, and the OpenClaw/NemoClaw/MCP bot surfaces. Staged higher-layer or SDK material should live under ignored local incubation in `private_work/` rather than expanding the core surface.

## Before opening a PR

All of the following must pass:

```bash
# Format
cargo fmt --all --check

# Rust tests + warnings denied
RUSTFLAGS="-D warnings" cargo test --all-targets

# Clippy
cargo clippy --all-targets -- -D warnings

# Core Python-backed runtime tests
python3 -W error -m unittest \
  python.tests.test_protocol \
  python.tests.test_runtime \
  python.tests.test_discovery \
  python.tests.test_jobs \
  python.tests.test_payments \
  python.tests.test_sandbox \
  python.tests.test_security \
  python.tests.test_privacy \
  python.tests.test_hardening \
  python.tests.test_conformance_vectors -v
```

Or run the full matrix in one shot:

```bash
./scripts/strict_checks.sh
```

## Conformance vectors

`conformance/kernel_v1.json` contains fixed golden vectors for the v1 kernel. If you touch signing, hashing, or artifact structure, update the vectors and explain why in the PR description.

## Guidelines

- Keep the kernel small — new features belong in adapters or higher layers unless they require a signed artifact or a state transition.
- Public code in this repo must not depend on staged or unpublished higher-layer services; those services should consume only public APIs, signed artifacts, or documented external contracts.
- Prefer editing existing files over adding new ones.
- Match the existing code style; the formatter and linter enforce the rest.
