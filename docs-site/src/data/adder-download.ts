import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

export const ADDER_BYTES = 29_036;
export const ADDER_SHA256 = 'c54d02ca8c6f9db3a301ef1eebd7ed87c03d89bfa69c382d1f8cac22fc968553';

/** The exact Rust example produced by the existing locked browser build. */
export function checkedAdder(bytes: Uint8Array): Uint8Array {
  if (bytes.byteLength !== ADDER_BYTES || createHash('sha256').update(bytes).digest('hex') !== ADDER_SHA256) {
    throw new Error('The generated Rust adder differs from the checked download fixture');
  }
  const module = new WebAssembly.Module(bytes as BufferSource);
  const exports = new Map(WebAssembly.Module.exports(module).map(({ name, kind }) => [name, kind]));
  if (WebAssembly.Module.imports(module).length || exports.get('memory') !== 'memory' ||
      exports.get('alloc') !== 'function' || exports.get('run') !== 'function') {
    throw new Error('The checked Rust adder must export the run_json ABI and import nothing');
  }
  return bytes;
}

export function readAdder(siteRoot = process.cwd()): Uint8Array {
  // npm runs the Astro build from docs-site. A source-relative import.meta URL
  // would instead point into the generated server bundle while prerendering.
  return checkedAdder(new Uint8Array(readFileSync(resolve(siteRoot, 'src/generated/playground/adder.wasm'))));
}
