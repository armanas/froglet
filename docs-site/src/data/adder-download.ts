import { createHash } from 'node:crypto';
import * as asc from 'assemblyscript/asc';
import adderSource from '../scripts/playground/functions/adder.as?raw';
import { COMPILER_ARGS, CONTRACT } from '../scripts/playground/compiler';

export const ADDER_BYTES = 3484;
export const ADDER_SHA256 = '31bc64a4c91dcb6e5258578eab5a2efe6a06c0ddd611458581bb83bb86f8bb28';

/** Bind the public source compilation to its checked bytes and run_json ABI. */
export function checkedAdder(bytes: Uint8Array): Uint8Array {
  const actualSha256 = createHash('sha256').update(bytes).digest('hex');
  if (bytes.byteLength !== ADDER_BYTES || actualSha256 !== ADDER_SHA256) {
    throw new Error(`The compiled AssemblyScript adder differs from the checked download fixture: got ${bytes.byteLength} bytes, SHA-256 ${actualSha256}; expected ${ADDER_BYTES} bytes, SHA-256 ${ADDER_SHA256}`);
  }
  const module = new WebAssembly.Module(bytes as BufferSource);
  const exports = new Map(WebAssembly.Module.exports(module).map(({ name, kind }) => [name, kind]));
  if (WebAssembly.Module.imports(module).length || exports.get('memory') !== 'memory' ||
      exports.get('alloc') !== 'function' || exports.get('run') !== 'function') {
    throw new Error('The checked AssemblyScript adder must export the run_json ABI and import nothing');
  }
  return bytes;
}

/** Prerender from bundled public sources, independently of host-specific Rust builds. */
export async function compileAdder(): Promise<Uint8Array> {
  if (asc.version !== '0.28.20') throw new Error(`The checked adder requires AssemblyScript 0.28.20; got ${asc.version}`);
  const program = `${adderSource}\n${CONTRACT}`;
  const stderr = asc.createMemoryStream();
  let binary: Uint8Array | undefined;
  const result = await asc.main([...COMPILER_ARGS, 'input.ts'], {
    stderr,
    readFile: name => name === 'input.ts' ? program : null,
    writeFile: (name, contents) => { if (name === 'binary' && typeof contents !== 'string') binary = contents; },
    listFiles: () => [],
  });
  if (result.error || !binary) {
    throw new Error(`Could not compile the public adder: ${result.error?.message ?? 'no binary output'}\n${stderr.toString()}`);
  }
  return checkedAdder(binary);
}
