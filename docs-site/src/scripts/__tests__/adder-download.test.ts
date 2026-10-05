// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { createHash } from 'node:crypto';
import { ADDER_BYTES, ADDER_SHA256, checkedAdder, compileAdder } from '../../data/adder-download';
import { GET, prerender } from '../../pages/services/adder.wasm';
import { callModule } from './playground-helpers';

describe('checked AssemblyScript adder download', () => {
  it('serves the exact public AssemblyScript module through the static endpoint', async () => {
    expect(prerender).toBe(true);
    const response = await GET({} as Parameters<typeof GET>[0]);
    expect(response.headers.get('content-type')).toBe('application/wasm');
    const bytes = new Uint8Array(await response.arrayBuffer());
    expect(bytes.byteLength).toBe(3484);
    expect(ADDER_BYTES).toBe(3484);
    expect(createHash('sha256').update(bytes).digest('hex')).toBe('31bc64a4c91dcb6e5258578eab5a2efe6a06c0ddd611458581bb83bb86f8bb28');
    expect(ADDER_SHA256).toBe('31bc64a4c91dcb6e5258578eab5a2efe6a06c0ddd611458581bb83bb86f8bb28');
    expect(bytes).toEqual(await compileAdder());
    const module = await WebAssembly.compile(bytes as BufferSource);
    expect(WebAssembly.Module.imports(module)).toEqual([]);
    expect(WebAssembly.Module.exports(module)).toEqual(expect.arrayContaining([
      { name: 'memory', kind: 'memory' },
      { name: 'alloc', kind: 'function' },
      { name: 'run', kind: 'function' },
    ]));
    expect(JSON.parse(await callModule(bytes, '{"a":6,"b":7}'))).toEqual({ sum: 13, product: 42 });
  });

  it('rejects missing bytes and reports the actual and expected identities', async () => {
    const bytes = (await compileAdder()).slice(0, -1);
    const actual = createHash('sha256').update(bytes).digest('hex');
    expect(() => checkedAdder(bytes)).toThrow(`got ${bytes.byteLength} bytes, SHA-256 ${actual}; expected ${ADDER_BYTES} bytes, SHA-256 ${ADDER_SHA256}`);
  });

  it('rejects changed bytes even when the compiled module size matches', async () => {
    const bytes = (await compileAdder()).slice();
    bytes[bytes.length - 1] ^= 1;
    const actual = createHash('sha256').update(bytes).digest('hex');
    expect(() => checkedAdder(bytes)).toThrow(`got ${ADDER_BYTES} bytes, SHA-256 ${actual}; expected ${ADDER_BYTES} bytes, SHA-256 ${ADDER_SHA256}`);
  });
});
