import { WASM_PAGE_BYTES } from './protocol';
import type { Limits, ModuleRunner, RunOutcome } from './types';

// The module runs in a Web Worker so the provider can stop it. The Worker gets the module bytes and the input and nothing
// else: the module is instantiated with an empty import object, so it cannot reach the page, the network, or storage.

/**
 * The Worker's whole program. It follows what a node does with a froglet.wasm.run_json.v1 module, checks and words
 * included (src/sandbox.rs): the input is written where `alloc` says, `run` returns the result's pointer and length
 * packed in an i64, and the result must be UTF-8 JSON within the output limit.
 */
export const RUNNER_SOURCE = `self.onmessage = async ({ data }) => {
  try {
    const { instance } = await WebAssembly.instantiate(data.bytes, {});
    const ex = instance.exports;
    const input = new TextEncoder().encode(data.input);
    const size = () => ex.memory.buffer.byteLength;
    const pointer = ex.alloc(input.length);
    if (pointer < 0) throw new Error('Wasm alloc returned a negative pointer');
    if (pointer + input.length > size()) throw new Error('Wasm alloc returned out-of-bounds pointer');
    new Uint8Array(ex.memory.buffer, pointer, input.length).set(input);
    const packed = BigInt.asUintN(64, ex.run(pointer, input.length));
    const at = Number(packed >> 32n), length = Number(packed & 0xffffffffn);
    if (length > data.maxOutput) throw new Error('Wasm module output size limit exceeded');
    if (at + length > size()) throw new Error('Wasm result pointer is out of bounds');
    let output;
    try { output = new TextDecoder('utf-8', { fatal: true }).decode(new Uint8Array(ex.memory.buffer, at, length).slice()); }
    catch { throw new Error('Wasm result is not valid UTF-8 JSON'); }
    let capEnforced = false;
    try { ex.memory.grow(data.maxPages + 1); } catch { capEnforced = true; }
    postMessage({ ok: true, output, memoryBytes: ex.memory.buffer.byteLength, capEnforced });
  } catch (error) {
    postMessage({ ok: false, error: String(error && error.message || error) });
  }
};`;

/** The part of a Worker the runner uses. A browser's Worker fits it, and so does an adapter over Node's worker_threads. */
export interface WorkerLike {
  postMessage(message: unknown): void;
  terminate(): void;
  onmessage: ((event: { data: any }) => void) | null;
  onerror: ((event: { message?: string }) => void) | null;
}

/** A runner that starts a fresh Worker for each run and stops it when the run ends or `max_runtime_ms` passes. */
export function createWorkerRunner(spawn: () => WorkerLike): ModuleRunner {
  return (module: Uint8Array, input: string, limits: Limits) =>
    new Promise<RunOutcome>((resolve) => {
      const worker = spawn();
      let finished = false;
      const finish = (outcome: RunOutcome) => {
        if (finished) return;
        finished = true;
        clearTimeout(timer);
        worker.terminate();
        resolve(outcome);
      };
      const timer = setTimeout(() => finish({ ok: false, error: `Wasm module wall-clock timeout exceeded after ${limits.max_runtime_ms / 1000}s`, timedOut: true }), limits.max_runtime_ms);
      worker.onmessage = ({ data }) => finish(data as RunOutcome);
      worker.onerror = (event) => finish({ ok: false, error: event.message || 'the function failed' });
      worker.postMessage({ bytes: module, input, maxOutput: limits.max_output_bytes, maxPages: limits.max_memory_bytes / WASM_PAGE_BYTES });
    });
}

/** A Worker in the browser, started from the program above, seen through the same small surface the runner uses. */
export function browserWorker(): WorkerLike {
  const url = URL.createObjectURL(new Blob([RUNNER_SOURCE], { type: 'text/javascript' }));
  const worker = new Worker(url);
  const like: WorkerLike = {
    onmessage: null,
    onerror: null,
    postMessage: (message) => worker.postMessage(message),
    terminate: () => {
      worker.terminate();
      URL.revokeObjectURL(url);
    },
  };
  worker.onmessage = (event) => like.onmessage?.({ data: event.data });
  worker.onerror = (event) => like.onerror?.({ message: event.message });
  return like;
}
