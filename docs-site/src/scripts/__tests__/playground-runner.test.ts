import { describe, expect, it } from 'vitest';
import { createWorkerRunner, RUNNER_SOURCE, type WorkerLike } from '../playground/runner';
import { PLAYGROUND_LIMITS } from '../playground/protocol';

import { admitModule } from '../playground/wasm-module';
import { hexBytes, NEVER_ENDS_MODULE_HEX, nodeRunner, samples, withSection } from './playground-helpers';

const limits = { ...PLAYGROUND_LIMITS, max_runtime_ms: 300 };
const runner = nodeRunner();
const capped = async (id: string) => (await admitModule(await samples[id].bytes(), limits)).bytes;

// Small modules assembled by hand, from the same base as the never-ending one: a page of memory, and the three exports of
// froglet.wasm.run_json.v1. Only the two function bodies differ.
const sleb = (value: bigint): number[] => {
  const out: number[] = [];
  for (;;) {
    const byte = Number(value & 0x7fn);
    value >>= 7n;
    if ((value === 0n && !(byte & 0x40)) || (value === -1n && byte & 0x40)) return [...out, byte];
    out.push(byte | 0x80);
  }
};
const packed = (pointer: number, length: number) => (BigInt(pointer) << 32n) | BigInt(length);
/** A module whose `alloc` and `run` have these bodies (each starts with its local count and ends with `end`). */
const module = (allocBody: number[], runBody: number[], data: number[] | null = [1, 0, 0x41, 32, 0x0b, 2, 0x34, 0x32]) => {
  const base = hexBytes(NEVER_ENDS_MODULE_HEX);
  const withCode = withSection(base, 10, [2, allocBody.length, ...allocBody, runBody.length, ...runBody]);
  return data ? withSection(withCode, 11, data) : withCode;
};
const allocReturns = (pointer: number) => [0, 0x41, ...sleb(BigInt(pointer)), 0x0b];
const runReturns = (value: bigint) => [0, 0x42, ...sleb(value), 0x0b];
/** `run` returns (pointer << 32) | length of its own input: the input text comes back unchanged. */
const echo = () => module(allocReturns(64), [0, 0x20, 0, 0xad, 0x42, 32, 0x86, 0x20, 1, 0xad, 0x84, 0x0b]);
/** Answers "42", read from the data at offset 32. */
const answers42 = () => module(allocReturns(64), runReturns(packed(32, 2)));

describe('the module runner', () => {
  it('runs a real compiled Rust function on the input it is given', async () => {
    expect(await runner(await capped('adder'), '{"a":6,"b":7}', limits)).toMatchObject({ ok: true, output: '{"sum":13,"product":42}' });
    expect(await runner(await capped('fibonacci'), '{"n":10}', limits)).toMatchObject({ ok: true, output: '{"n":10,"fibonacci":55}' });
  });

  it('hands the function the text it was given, byte for byte, and does not write it again', async () => {
    const text = '{"b": 7,   "a":6, "é": "ü\\n"}';
    expect(await runner(echo(), text, limits)).toMatchObject({ ok: true, output: text });
    expect(await runner(await capped('adder'), '{"b":-5,"a":2}', limits)).toMatchObject({ ok: true, output: '{"sum":-3,"product":-10}' });
  });

  it('runs a module that answers from its own data', async () => {
    expect(await runner(answers42(), '{}', limits)).toMatchObject({ ok: true, output: '42' });
  });

  it('reports whether the memory cap held: it does for a capped module, and does not for an uncapped one', async () => {
    const capOn = await runner(await capped('adder'), '{"a":1,"b":1}', limits);
    expect(capOn).toMatchObject({ ok: true, capEnforced: true });
    const capOff = await runner(await samples.adder.bytes(), '{"a":1,"b":1}', limits);
    expect(capOff).toMatchObject({ ok: true, capEnforced: false });
  });

  it('stops a function that never returns at the time limit, and answers in about that time', async () => {
    const started = performance.now();
    const outcome = await runner(await capped('never-ends'), '{}', limits);
    const took = performance.now() - started;
    expect(outcome).toMatchObject({ ok: false, timedOut: true, error: 'Wasm module wall-clock timeout exceeded after 0.3s' });
    expect(took).toBeGreaterThanOrEqual(limits.max_runtime_ms - 20);
    expect(took).toBeLessThan(limits.max_runtime_ms + 1500);
  });

  it('keeps working after it has stopped a runaway function', async () => {
    await runner(await capped('never-ends'), '{}', limits);
    expect(await runner(await capped('adder'), '{"a":20,"b":22}', limits)).toMatchObject({ ok: true, output: '{"sum":42,"product":440}' });
  });

  it('reports a trap as a failure with its reason, not a timeout', async () => {
    const trap = module(allocReturns(64), [0, 0x00, 0x0b]);
    const outcome = await runner(trap, '{}', limits);
    expect(outcome.ok).toBe(false);
    if (!outcome.ok) {
      expect(outcome.timedOut).toBeUndefined();
      expect(outcome.error).toMatch(/unreachable/i);
    }
  });

  it('refuses an output larger than max_output_bytes, in the node\'s words', async () => {
    const outcome = await runner(await capped('adder'), '{"a":1,"b":1}', { ...limits, max_output_bytes: 5 });
    expect(outcome).toMatchObject({ ok: false, error: 'Wasm module output size limit exceeded' });
  });

  describe('the checks a node makes, in its words', () => {
    const fails = async (bytes: Uint8Array, input = '{}') => {
      const outcome = await runner(bytes, input, limits);
      expect(outcome.ok).toBe(false);
      return outcome.ok ? '' : outcome.error;
    };

    it('refuses a result that is not UTF-8', async () => {
      expect(await fails(module(allocReturns(64), runReturns(packed(32, 2)), [1, 0, 0x41, 32, 0x0b, 2, 0xff, 0xfe]))).toBe('Wasm result is not valid UTF-8 JSON');
    });

    it('refuses a result that points outside the module\'s memory', async () => {
      expect(await fails(module(allocReturns(64), runReturns(packed(32, 100_000))))).toBe('Wasm result pointer is out of bounds');
      expect(await fails(module(allocReturns(64), runReturns(packed(70_000, 2))))).toBe('Wasm result pointer is out of bounds');
    });

    it('refuses an alloc that returns a negative pointer or one where the input does not fit', async () => {
      expect(await fails(module(allocReturns(-1), runReturns(packed(32, 2))))).toBe('Wasm alloc returned a negative pointer');
      expect(await fails(module(allocReturns(65_535), runReturns(packed(32, 2))), '{"a":1}')).toBe('Wasm alloc returned out-of-bounds pointer');
    });
  });

  it('answers once, and stops the Worker, whichever of the result or the time limit comes first', async () => {
    let terminated = 0;
    let deliver: ((data: unknown) => void) | undefined;
    const spawn = (): WorkerLike => {
      const worker: WorkerLike = {
        onmessage: null,
        onerror: null,
        postMessage: () => {},
        terminate: () => void (terminated += 1),
      };
      deliver = (data) => worker.onmessage?.({ data });
      return worker;
    };
    const outcome = await createWorkerRunner(spawn)(new Uint8Array(), '{}', { ...limits, max_runtime_ms: 20 });
    expect(outcome).toMatchObject({ ok: false, timedOut: true });
    deliver!({ ok: true, output: 'late', memoryBytes: 0, capEnforced: true });
    expect(terminated).toBe(1);
  });

  it('reports a Worker that fails to start as a failed run', async () => {
    const spawn = (): WorkerLike => {
      const worker: WorkerLike = { onmessage: null, onerror: null, terminate: () => {}, postMessage: () => queueMicrotask(() => worker.onerror?.({ message: 'blocked' })) };
      return worker;
    };
    expect(await createWorkerRunner(spawn)(new Uint8Array(), '{}', limits)).toEqual({ ok: false, error: 'blocked' });
  });

  it('gives the module nothing to reach out with: its program instantiates it with no imports', () => {
    expect(RUNNER_SOURCE).toContain('WebAssembly.instantiate(data.bytes, {})');
    expect(RUNNER_SOURCE).not.toMatch(/fetch|XMLHttpRequest|importScripts|indexedDB|localStorage|WebSocket/);
  });
});
