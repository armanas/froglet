// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { createHash } from 'node:crypto';
import { ADDER_BYTES, ADDER_SHA256, checkedAdder, readAdder } from '../../data/adder-download';
import { GET, prerender } from '../../pages/services/adder.wasm';
import { callModule } from './playground-helpers';
import { docsSite } from './route-helpers';

describe('checked Rust adder download', () => {
  it('serves the exact generated module through the static endpoint', async () => {
    expect(prerender).toBe(true);
    const response = await GET({} as Parameters<typeof GET>[0]);
    expect(response.headers.get('content-type')).toBe('application/wasm');
    const bytes = new Uint8Array(await response.arrayBuffer());
    expect(bytes.byteLength).toBe(ADDER_BYTES);
    expect(createHash('sha256').update(bytes).digest('hex')).toBe(ADDER_SHA256);
    expect(bytes).toEqual(readAdder(docsSite));
    const module = await WebAssembly.compile(bytes as BufferSource);
    expect(WebAssembly.Module.imports(module)).toEqual([]);
    expect(WebAssembly.Module.exports(module)).toEqual(expect.arrayContaining([
      { name: 'memory', kind: 'memory' },
      { name: 'alloc', kind: 'function' },
      { name: 'run', kind: 'function' },
    ]));
    expect(JSON.parse(await callModule(bytes, '{"a":6,"b":7}'))).toEqual({ sum: 13, product: 42 });
  });

  it('rejects missing bytes rather than publishing a different module', () => {
    expect(() => checkedAdder(readAdder(docsSite).slice(0, -1))).toThrow('differs from the checked download fixture');
  });

  it('rejects changed bytes even when the generated file size matches', () => {
    const bytes = readAdder(docsSite).slice();
    bytes[bytes.length - 1] ^= 1;
    expect(() => checkedAdder(bytes)).toThrow('differs from the checked download fixture');
  });
});
