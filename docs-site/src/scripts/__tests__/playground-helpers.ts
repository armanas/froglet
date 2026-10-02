// Shared by the playground tests. Nothing here is mocked that matters: the kernel and verifier are the real WebAssembly
// builds, the reference services are the real compiled Rust, the editor's compiler is the real AssemblyScript, and the
// runner starts real Workers (Node's worker_threads, running the same source a browser Worker runs).
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { Worker as NodeWorker } from 'node:worker_threads';
import * as kernelBindings from '../../generated/kernel/froglet_wasm.js';
import * as verifierBindings from '../../generated/verifier/froglet_verify.js';
import { COMPILER_WORKER_SOURCE, createCompiler } from '../playground/compiler';
import { createWorkerRunner, RUNNER_SOURCE, type WorkerLike } from '../playground/runner';
import { kernelFrom, verifierFrom } from '../playground/kernel';
import type { Kernel, Verifier } from '../playground/types';
import { docsSite, repoRoot } from './route-helpers';

kernelBindings.initSync({ module: readFileSync(resolve(docsSite, 'src/generated/kernel/froglet_wasm_bg.wasm')) });
verifierBindings.initSync({ module: readFileSync(resolve(docsSite, 'src/generated/verifier/froglet_verify_bg.wasm')) });

export const kernel: Kernel = kernelFrom(kernelBindings);
export const verifier: Verifier = verifierFrom(verifierBindings);

export const vectors = JSON.parse(readFileSync(resolve(repoRoot, 'conformance/kernel_v1.json'), 'utf8'));
export const nodeExchange = JSON.parse(readFileSync(resolve(repoRoot, 'froglet-wasm/tests/fixtures/node_service_exchange.json'), 'utf8'));

const PRELUDE = `const { parentPort } = require('node:worker_threads');
globalThis.self = globalThis;
globalThis.postMessage = (message) => parentPort.postMessage(message);
parentPort.on('message', (data) => globalThis.onmessage({ data }));
`;

/** A Worker with the browser Worker's surface, running one of the page's own Worker programs in a Node thread. */
function nodeWorkerFor(source: string): WorkerLike {
  const worker = new NodeWorker(PRELUDE + source, { eval: true });
  let stopped = false;
  const like: WorkerLike = {
    onmessage: null,
    onerror: null,
    postMessage: (message) => worker.postMessage(message),
    terminate: () => {
      stopped = true;
      void worker.terminate();
    },
  };
  worker.on('message', (data) => like.onmessage?.({ data }));
  worker.on('error', (error) => like.onerror?.({ message: error.message }));
  // A Worker that ends by itself (the compiler exits the process when it crashes) is reported, as a browser reports one.
  worker.on('exit', (code) => {
    if (!stopped) like.onerror?.({ message: `the worker exited with code ${code}` });
  });
  return like;
}

export const nodeWorker = () => nodeWorkerFor(RUNNER_SOURCE);
export const nodeCompilerWorker = () => nodeWorkerFor(COMPILER_WORKER_SOURCE);

/** The compiler bundle the site build made, as the URL a Worker imports it from. */
export const compilerUrl = pathToFileURL(resolve(docsSite, 'src/generated/assemblyscript/asc.js')).href;

/** A compiler over the real bundle. Loading it takes about a second, so a test file makes one and disposes of it at the end. */
export const newCompiler = (overrides: Partial<Parameters<typeof createCompiler>[0]> = {}) => createCompiler({ spawn: nodeCompilerWorker, compilerUrl, ...overrides });

/** Runs a module on some input text in this thread, as the runner's Worker does: alloc, write, run, read. For comparing many answers quickly. */
export async function callModule(bytes: Uint8Array, text: string): Promise<string> {
  const { instance } = await WebAssembly.instantiate(bytes as BufferSource, {});
  const ex = instance.exports as { alloc(len: number): number; run(pointer: number, len: number): bigint; memory: WebAssembly.Memory };
  const input = new TextEncoder().encode(text);
  const pointer = ex.alloc(input.length);
  new Uint8Array(ex.memory.buffer, pointer, input.length).set(input);
  const packed = BigInt.asUintN(64, ex.run(pointer, input.length));
  return new TextDecoder().decode(new Uint8Array(ex.memory.buffer, Number(packed >> 32n), Number(packed & 0xffffffffn)));
}

export const nodeRunner = () => createWorkerRunner(nodeWorker);

const generated = (name: string) => new Uint8Array(readFileSync(resolve(docsSite, 'src/generated/playground', name)));

/**
 * A module with the run_json.v1 exports whose `run` is `loop { br 0 }`, built by hand rather than compiled from anything.
 * It is the conformance-style module (alloc returns 64; run returns a pointer and length for "42") with the nine bytes of
 * run's body replaced: `00 42 82 80 80 80 80 04 0b` became `00 03 40 0c 00 0b 42 00 0b`, so run spins forever.
 */
export const NEVER_ENDS_MODULE_HEX =
  '0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a1102050041c0000b090003400c000b42000b0b08010041200b023432';

export interface Sample {
  id: string;
  serviceId: string;
  summary: string;
  input: string;
  bytes(): Promise<Uint8Array>;
}

/**
 * Modules to publish in the tests that are not about the editor: the two Rust services in examples/wasm-services, built
 * with the site, and the hand-built module that never returns. The Rust ones are also what the editor's functions are
 * compared with.
 */
export const samples: Record<string, Sample> = {
  adder: { id: 'adder', serviceId: 'demo.adder', summary: 'Adds and multiplies two integers', input: '{"a": 6, "b": 7}', bytes: async () => generated('adder.wasm') },
  fibonacci: { id: 'fibonacci', serviceId: 'demo.fibonacci', summary: 'The nth Fibonacci number', input: '{"n": 10}', bytes: async () => generated('fibonacci.wasm') },
  'never-ends': { id: 'never-ends', serviceId: 'demo.never-ends', summary: 'A function that never finishes', input: '{}', bytes: async () => hexBytes(NEVER_ENDS_MODULE_HEX) },
};

/** A clock the test moves by hand. */
export function fakeClock(start = 1_700_000_000) {
  let seconds = start;
  return { now: () => seconds, advance: (by: number) => void (seconds += by) };
}

/** Deterministic ids for the provider. */
export function counterHex() {
  let count = 0;
  return (bytes: number) => (count += 1).toString(16).padStart(bytes * 2, '0');
}

// ── A small WebAssembly section editor, written independently of the code under test ──
const leb = (value: number): number[] => {
  const out: number[] = [];
  do {
    let byte = value & 0x7f;
    value >>>= 7;
    if (value) byte |= 0x80;
    out.push(byte);
  } while (value);
  return out;
};

const readLeb = (bytes: Uint8Array, at: number): [number, number] => {
  let value = 0;
  let shift = 0;
  for (;;) {
    const byte = bytes[at++];
    value |= (byte & 0x7f) << shift;
    if (!(byte & 0x80)) return [value >>> 0, at];
    shift += 7;
  }
};

export interface Section {
  id: number;
  body: Uint8Array;
}

export function sectionsOf(bytes: Uint8Array): Section[] {
  const sections: Section[] = [];
  let at = 8;
  while (at < bytes.length) {
    const id = bytes[at++];
    let size: number;
    [size, at] = readLeb(bytes, at);
    sections.push({ id, body: bytes.slice(at, at + size) });
    at += size;
  }
  return sections;
}

export function assemble(sections: Section[]): Uint8Array {
  const out = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
  for (const { id, body } of sections) out.push(id, ...leb(body.length), ...body);
  return Uint8Array.from(out);
}

/** The module with one section swapped for another body, added when the module had none. */
export function withSection(bytes: Uint8Array, id: number, body: number[] | null): Uint8Array {
  const sections = sectionsOf(bytes).filter((section) => section.id !== id);
  if (body) {
    const at = sections.findIndex((section) => section.id > id);
    sections.splice(at === -1 ? sections.length : at, 0, { id, body: Uint8Array.from(body) });
  }
  return assemble(sections);
}

export const memorySection = (min: number, max?: number, flags = max === undefined ? 0 : 1) => [1, flags, ...leb(min), ...(max === undefined ? [] : leb(max))];

export const hexBytes = (hex: string) => Uint8Array.from(hex.match(/../g)!.map((pair) => parseInt(pair, 16)));

/** JSON with its object keys sorted at every depth, so two values compare by content and not by key order. */
export const sorted = (value: unknown): unknown =>
  Array.isArray(value)
    ? value.map(sorted)
    : value && typeof value === 'object'
      ? Object.fromEntries(Object.entries(value).sort(([a], [b]) => (a < b ? -1 : 1)).map(([key, inner]) => [key, sorted(inner)]))
      : value;

/** The set of key paths a JSON value has, with the type of each leaf: what "the same shape" means here. */
export function shapeOf(value: unknown, path = '$'): string[] {
  if (Array.isArray(value)) return value.length ? shapeOf(value[0], `${path}[]`) : [`${path}[]`];
  if (value && typeof value === 'object') return Object.entries(value).flatMap(([key, inner]) => shapeOf(inner, `${path}.${key}`)).sort();
  return [`${path}:${typeof value}`];
}
