# Wasm services

Three sample `froglet.wasm.run_json.v1` programs, written in Rust. The adder and Fibonacci examples have no dependencies;
the terminology checker uses Serde for strict JSON parsing. All modules import nothing.

| Service | Reads | Returns |
|---|---|---|
| `adder` | `{"a": 6, "b": 7}` | `{"sum": 13, "product": 42}` |
| `fibonacci` | `{"n": 10}` | `{"n": 10, "fibonacci": 55}` |
| `ontology-check` | `{"mappings":[{"source":"DEMO:a","target":"DEMO:b"}],"observed_terms":["DEMO:a","DEMO:missing"]}` | `{"mappings_consistent":true,"conflicts":[],"unmapped_terms":["DEMO:missing"],"mapped_terms":1}` |

`ontology-check` audits exact, case-sensitive strings under a one-target-per-source policy; it does not evaluate scientific
truth or semantic equivalence. Conflicts include all sources having multiple distinct targets, preserving sorted candidate
targets even for unobserved sources. Identical duplicate rows are allowed. Unmapped observed terms are sorted and deduplicated;
`mapped_terms` counts unique observed terms having exactly one target. An unmapped term does not make the mapping table inconsistent.
The two input arrays are required, row fields must be exactly `source` and `target`, and strings must be nonempty. Unknown,
duplicate or missing fields, positional-array rows, malformed JSON/UTF-8 and invalid value types return a deterministic JSON error.

## The contract

A module exports `memory`, `alloc(len: i32) -> i32`, and `run(ptr: i32, len: i32) -> i64`, and imports nothing. The host
writes the request JSON into the memory `alloc` returns, calls `run`, and reads the response JSON from the pointer in the
high 32 bits of the result, with its length in the low 32. The full contract, including the limits a node applies, is in
[`docs/SERVICE_BINDING.md`](../../docs/SERVICE_BINDING.md).

Keep numbers within ±(2^53 − 1), as [RFC 7493](https://www.rfc-editor.org/rfc/rfc7493) asks of interoperable JSON. A
JavaScript host, such as the playground on froglet.dev, rounds larger integers, so the same request would hash and answer
differently from a Rust node. Both numeric samples refuse inputs and results outside that range.

## Build

```bash
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown --manifest-path examples/wasm-services/Cargo.toml
```

The modules are `adder.wasm`, `fibonacci.wasm` and `ontology_check.wasm` under `examples/wasm-services/target/wasm32-unknown-unknown/release/`.
This directory is its own Cargo workspace, with a release profile tuned for small modules and its own `Cargo.lock`.

The docs-site build compiles it too (`npm run build:playground` in `docs-site/`). The playground on the Developers page
does not serve these modules. Its editor starts from AssemblyScript ports of the adder and the Fibonacci function
(`docs-site/src/scripts/playground/functions/`), which the page compiles in the tab, and the site's tests run these Rust
builds as the answer that each port must give, input for input. You can change a port and call it without installing
anything.

To publish a module from a node, follow the [provider setup guide](https://froglet.dev/learn/provider-onboarding/).
For a real selected-data retrieval followed by requester-supplied audit code, run
`python3 examples/a2a_compute_demo.py --scenario ontology` from the repository root; see [the demo guide](../README.md).
