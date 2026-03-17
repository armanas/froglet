# Contributing

## Kernel changes

`SPEC.md` is a frozen protocol document. Changes to canonical artifact payloads, hashing, signing bytes, state transitions, or the Lightning settlement binding require a strong interoperability justification and must be discussed before a PR is opened. Everything outside the kernel (adapters, runtime helpers, marketplace, Python SDK) is fair game.

## Before opening a PR

All of the following must pass:

```bash
# Format
cargo fmt --all --check

# Rust tests + warnings denied
RUSTFLAGS="-D warnings" cargo test --all-targets

# Clippy
cargo clippy --all-targets -- -D warnings

# Python tests
python3 -W error -m unittest discover -s python/tests -t . -v
```

Or run the full matrix in one shot:

```bash
./scripts/strict_checks.sh
```

## Conformance vectors

`conformance/kernel_v1.json` contains fixed golden vectors for the v1 kernel. If you touch signing, hashing, or artifact structure, update the vectors and explain why in the PR description.

## Guidelines

- Keep the kernel small — new features belong in adapters or higher layers unless they require a signed artifact or a state transition.
- Public code in this repo must not depend on ignored `private/` incubations; private higher-layer services should consume only public APIs, signed artifacts, or documented external contracts.
- Prefer editing existing files over adding new ones.
- Match the existing code style; the formatter and linter enforce the rest.
