import { describe, expect, it } from 'vitest';
import { MAX_MODULE_BYTES, PLAYGROUND_LIMITS, Refusal, WASM_PAGE_BYTES } from '../playground/protocol';
import { admitModule, capMemory } from '../playground/wasm-module';
import { hexBytes, memorySection, NEVER_ENDS_MODULE_HEX, samples, sectionsOf, withSection } from './playground-helpers';

const CAP_PAGES = PLAYGROUND_LIMITS.max_memory_bytes / WASM_PAGE_BYTES;
const loop = hexBytes(NEVER_ENDS_MODULE_HEX);

const refusal = async (attempt: () => unknown) => {
  try {
    await attempt();
  } catch (error) {
    expect(error).toBeInstanceOf(Refusal);
    return error as Refusal;
  }
  throw new Error('expected a refusal');
};

describe('the memory cap', () => {
  it('is 128 pages for the playground limit', () => {
    expect(CAP_PAGES).toBe(128);
  });

  it('adds a maximum to a module that declared none, and changes nothing else in the module', async () => {
    const adder = await samples.adder.bytes();
    const { bytes, memory } = capMemory(adder, CAP_PAGES);
    expect(memory.declaredMax).toBeNull();
    expect(memory.cappedTo).toBe(CAP_PAGES);
    const before = sectionsOf(adder);
    const after = sectionsOf(bytes);
    expect(after.map((section) => section.id)).toEqual(before.map((section) => section.id));
    for (const [index, section] of before.entries()) {
      if (section.id !== 5) expect(after[index].body, `section ${section.id}`).toEqual(section.body);
    }
    expect(bytes.length).toBeGreaterThan(adder.length - 4);
  });

  it('makes growth past the maximum fail, where the uncapped module could grow', async () => {
    const adder = await samples.adder.bytes();
    const grow = async (module: Uint8Array) => {
      const { instance } = await WebAssembly.instantiate(module as BufferSource, {});
      return () => (instance.exports.memory as WebAssembly.Memory).grow(CAP_PAGES + 1);
    };
    expect(await grow(adder)).not.toThrow();
    expect(await grow(capMemory(adder, CAP_PAGES).bytes)).toThrow(RangeError);
  });

  it('lets a module keep a smaller maximum than the cap, and lowers a larger one to it', () => {
    const smaller = capMemory(withSection(loop, 5, memorySection(1, 2)), CAP_PAGES);
    expect(smaller.memory).toEqual({ min: 1, declaredMax: 2, cappedTo: 2 });
    const larger = capMemory(withSection(loop, 5, memorySection(1, 60_000)), CAP_PAGES);
    expect(larger.memory).toEqual({ min: 1, declaredMax: 60_000, cappedTo: CAP_PAGES });
    expect(sectionsOf(larger.bytes).find((section) => section.id === 5)!.body).toEqual(Uint8Array.from(memorySection(1, CAP_PAGES)));
  });

  it('refuses a module that starts with more memory than the cap allows', async () => {
    const error = await refusal(() => capMemory(withSection(loop, 5, memorySection(CAP_PAGES + 1)), CAP_PAGES));
    expect(error.status).toBe(400);
    expect(error.message).toContain('more memory than the limit allows');
  });

  it('refuses modules it cannot read safely', async () => {
    expect((await refusal(() => capMemory(new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8]), CAP_PAGES))).message).toContain('not a WebAssembly module');
    expect((await refusal(() => capMemory(loop.slice(0, 40), CAP_PAGES))).message).toContain('not a valid WebAssembly module');
    expect((await refusal(() => capMemory(withSection(loop, 5, null), CAP_PAGES))).message).toContain('must declare a memory');
    expect((await refusal(() => capMemory(withSection(loop, 5, [2, 0, 1, 0, 1]), CAP_PAGES))).message).toContain('exactly one memory');
    expect((await refusal(() => capMemory(withSection(loop, 5, memorySection(1, 2, 0x03)), CAP_PAGES))).message).toContain('shared and 64-bit');
  });
});

describe('admitting a module', () => {
  it('admits the samples and reports their memory', async () => {
    for (const id of ['adder', 'fibonacci', 'never-ends']) {
      const admitted = await admitModule(await samples[id].bytes(), PLAYGROUND_LIMITS);
      expect(admitted.memory.cappedTo, id).toBeLessThanOrEqual(CAP_PAGES);
      expect(admitted.memory.min, id).toBeGreaterThan(0);
    }
  });

  it('refuses a module over the size limit before reading it', async () => {
    const error = await refusal(() => admitModule(new Uint8Array(MAX_MODULE_BYTES + 1), PLAYGROUND_LIMITS));
    expect(error.status).toBe(413);
  });

  it('refuses bytes that are not a module', async () => {
    const error = await refusal(() => admitModule(Uint8Array.from([0, 0x61, 0x73, 0x6d, 1, 0, 0, 0, 0x99, 0x01, 0x00]), PLAYGROUND_LIMITS));
    expect(error.status).toBe(400);
    expect(error.message).toContain('not a valid WebAssembly module');
  });

  it('refuses a module that imports anything, since a function may not reach outside itself', async () => {
    // (module (type () -> ()) (import "a" "b" (func (type 0))) (memory 1) (export "memory" (memory 0)))
    const importing = withSection(withSection(withSection(withSection(new Uint8Array([0, 0x61, 0x73, 0x6d, 1, 0, 0, 0]), 1, [1, 0x60, 0, 0]), 2, [1, 1, 0x61, 1, 0x62, 0, 0]), 5, memorySection(1)), 7, [1, 6, ...Array.from('memory', (c) => c.charCodeAt(0)), 2, 0]);
    const error = await refusal(() => admitModule(importing, PLAYGROUND_LIMITS));
    expect(error.message).toContain('may not import anything');
  });

  it('refuses a module without the run_json exports', async () => {
    const bare = withSection(withSection(new Uint8Array([0, 0x61, 0x73, 0x6d, 1, 0, 0, 0]), 5, memorySection(1)), 7, [1, 6, ...Array.from('memory', (c) => c.charCodeAt(0)), 2, 0]);
    const error = await refusal(() => admitModule(bare, PLAYGROUND_LIMITS));
    expect(error.message).toContain('must export memory, alloc, and run');
  });
});
