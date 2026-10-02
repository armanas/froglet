import { MAX_MODULE_BYTES, Refusal, WASM_PAGE_BYTES } from './protocol';
import type { Limits } from './types';

// A browser cannot count fuel and cannot cap a module's memory from outside, so the provider does the two things it can:
// it rewrites the module's memory declaration to add a maximum (growth past it fails, as under a node's limiter), and it
// runs the module where it can be stopped (see runner.ts). Before either, it admits only modules that follow the
// froglet.wasm.run_json.v1 contract: no imports, and exports for memory, alloc, and run.

/** Unsigned LEB128 at `index`: the value, and the index after it. */
function readLeb(bytes: Uint8Array, index: number): [number, number] {
  let value = 0;
  let shift = 0;
  let byte: number;
  do {
    if (index >= bytes.length || shift > 28) throw new Refusal(400, 'not a valid WebAssembly module');
    byte = bytes[index++];
    value |= (byte & 0x7f) << shift;
    shift += 7;
  } while (byte & 0x80);
  return [value >>> 0, index];
}

function encodeLeb(value: number): number[] {
  const out: number[] = [];
  do {
    let byte = value & 0x7f;
    value >>>= 7;
    if (value) byte |= 0x80;
    out.push(byte);
  } while (value);
  return out;
}

export interface MemoryInfo {
  /** Pages the module starts with. */
  min: number;
  /** The maximum the module declared, or null if it declared none. */
  declaredMax: number | null;
  /** The maximum after the cap. */
  cappedTo: number;
}

/**
 * The module with its memory maximum set to at most `maxPages`. A module that starts larger than that is refused.
 * Only the memory section changes, so no offset in the module moves.
 */
export function capMemory(bytes: Uint8Array, maxPages: number): { bytes: Uint8Array; memory: MemoryInfo } {
  if (bytes.length < 8 || bytes[0] !== 0x00 || bytes[1] !== 0x61 || bytes[2] !== 0x73 || bytes[3] !== 0x6d) {
    throw new Refusal(400, 'not a WebAssembly module');
  }
  const out: number[] = Array.from(bytes.subarray(0, 8));
  let memory: MemoryInfo | null = null;
  let index = 8;
  while (index < bytes.length) {
    const id = bytes[index++];
    let size: number;
    [size, index] = readLeb(bytes, index);
    const end = index + size;
    if (end > bytes.length) throw new Refusal(400, 'not a valid WebAssembly module');
    if (id === 5) {
      let at = index;
      let count: number;
      [count, at] = readLeb(bytes, at);
      if (count !== 1) throw new Refusal(400, 'a module must declare exactly one memory');
      const flag = bytes[at++];
      if (flag & 0x06) throw new Refusal(400, 'shared and 64-bit memories are not allowed');
      let min: number;
      let max: number | null = null;
      [min, at] = readLeb(bytes, at);
      if (flag & 0x01) [max, at] = readLeb(bytes, at);
      if (min > maxPages) throw new Refusal(400, 'the module needs more memory than the limit allows');
      const cappedTo = Math.min(max ?? maxPages, maxPages);
      const body = [...encodeLeb(1), 0x01, ...encodeLeb(min), ...encodeLeb(cappedTo)];
      out.push(5, ...encodeLeb(body.length), ...body);
      memory = { min, declaredMax: max, cappedTo };
    } else {
      out.push(id, ...encodeLeb(size), ...bytes.subarray(index, end));
    }
    index = end;
  }
  if (!memory) throw new Refusal(400, 'a module must declare a memory');
  return { bytes: Uint8Array.from(out), memory };
}

/** Admit a module, or refuse it with the reason. Returns the bytes to run, with the memory cap applied. */
export async function admitModule(bytes: Uint8Array, limits: Limits): Promise<{ bytes: Uint8Array; memory: MemoryInfo }> {
  if (bytes.length > MAX_MODULE_BYTES) throw new Refusal(413, `the module is larger than ${MAX_MODULE_BYTES} bytes`);
  const compiled = await WebAssembly.compile(bytes as BufferSource).catch((error: Error) => {
    throw new Refusal(400, `not a valid WebAssembly module: ${error.message}`);
  });
  if (WebAssembly.Module.imports(compiled).length) throw new Refusal(400, 'a module may not import anything');
  const exports = new Map(WebAssembly.Module.exports(compiled).map((entry) => [entry.name, entry.kind]));
  if (exports.get('memory') !== 'memory' || exports.get('alloc') !== 'function' || exports.get('run') !== 'function') {
    throw new Refusal(400, 'a module must export memory, alloc, and run');
  }
  return capMemory(bytes, limits.max_memory_bytes / WASM_PAGE_BYTES);
}
