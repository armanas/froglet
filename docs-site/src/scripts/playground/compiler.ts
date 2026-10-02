import contractSource from './functions/contract.as?raw';
import { MAX_MODULE_BYTES } from './protocol';
import type { WorkerLike } from './runner';

// Compiles the function in the editor to a WebAssembly module, in this page, with the AssemblyScript compiler. The compiler
// is a large file (about 2 MB compressed), so it is loaded only when someone compiles or is about to, and it runs in a
// Worker: the page stays responsive, and a compile that never ends can be stopped.
//
// The source is the person's code followed by the contract (functions/contract.as), which supplies `alloc` and `run` around
// their `respond`. Their code comes first, so an offset the compiler reports inside it is an offset in the editor.

/** The contract that follows the person's code: the same for every function. */
export const CONTRACT = contractSource.trimEnd();

/** The most source the editor accepts. */
export const MAX_SOURCE_CHARS = 65_536;

/**
 * The compiler's options. `--runtime stub` is a bump allocator with no collector (each call runs in a fresh Worker, so
 * nothing needs freeing), and `--use abort=` drops the `env.abort` import, so a failed assert is a trap and the module
 * imports nothing, as the contract requires.
 */
export const COMPILER_ARGS: readonly string[] = ['--outFile', 'binary', '--optimizeLevel', '3', '--shrinkLevel', '2', '--runtime', 'stub', '--use', 'abort=', '--noColors'];

export type Severity = 'error' | 'warning' | 'info';

export interface Diagnostic {
  severity: Severity;
  code: number;
  message: string;
  /** Where it is in the editor's text, counted from 1, or null when it is not in the person's code. */
  line: number | null;
  column: number | null;
  endLine: number | null;
  endColumn: number | null;
  /** True when the compiler reported it in the contract after their code, which usually means something in their code broke it. */
  inContract: boolean;
}

export interface ImportedName {
  module: string;
  name: string;
}

export type CompileResult =
  | { ok: true; module: Uint8Array; warnings: Diagnostic[]; ms: number }
  | { ok: false; kind: 'errors'; diagnostics: Diagnostic[]; ms: number }
  | { ok: false; kind: 'imports'; imports: ImportedName[]; message: string; ms: number }
  | { ok: false; kind: 'too-large' | 'timeout' | 'crashed' | 'unavailable'; message: string; ms: number };

export type Phase = 'loading' | 'compiling';

export interface Compiler {
  /** Starts loading the compiler, so the first compile finds it ready. It never fails: a compile reports what went wrong. */
  preload(): void;
  compile(source: string, onPhase?: (phase: Phase) => void): Promise<CompileResult>;
  dispose(): void;
}

/**
 * The Worker's whole program. It loads the compiler from the URL it is given (the site's own copy), compiles one source
 * held in memory, and reports the module and the compiler's diagnostics as data. It has no network or storage code of
 * its own, and it is not given a way to read files.
 */
export const COMPILER_WORKER_SOURCE = `let asc;
let loading;
const load = (url) => (loading ??= import(url).then((module) => { asc = module; }, (error) => { loading = undefined; throw error; }));
const place = (range) => (range ? { start: range.start, end: range.end } : null);
self.onmessage = async ({ data }) => {
  if (data.type === 'load') {
    try { await load(data.url); postMessage({ type: 'loaded' }); }
    catch (error) { postMessage({ type: 'unavailable', error: String(error && error.message || error) }); }
    return;
  }
  const started = performance.now();
  try {
    const files = { 'input.ts': data.program };
    const out = {};
    const diagnostics = [];
    await asc.main([...data.args, 'input.ts'], {
      stderr: asc.createMemoryStream(),
      readFile: (name) => (Object.prototype.hasOwnProperty.call(files, name) ? files[name] : null),
      writeFile: (name, contents) => { out[name] = contents; },
      listFiles: () => [],
      reportDiagnostic: (d) => diagnostics.push({ category: d.category, code: d.code, message: d.message, range: place(d.range), related: place(d.relatedRange) }),
    });
    postMessage({ type: 'result', id: data.id, binary: out.binary || null, diagnostics, ms: Math.round(performance.now() - started) });
  } catch (error) {
    postMessage({ type: 'result', id: data.id, binary: null, diagnostics: [], crashed: String(error && error.message || error), ms: Math.round(performance.now() - started) });
  }
};`;

interface RawDiagnostic {
  category: number;
  code: number;
  message: string;
  range: { start: number; end: number } | null;
  related: { start: number; end: number } | null;
}

/** 1-based line and column of an offset in `text`. */
export function lineAndColumn(text: string, offset: number): { line: number; column: number } {
  const before = text.slice(0, Math.max(0, Math.min(offset, text.length)));
  const line = before.split('\n').length;
  return { line, column: before.length - (before.lastIndexOf('\n') + 1) + 1 };
}

/**
 * The compiler's diagnostics as the editor shows them. An offset up to the end of the person's code is a place in it. One
 * after that is in the contract; if the compiler also points at a place in their code (a name defined twice, say), that place
 * is the one to show.
 */
export function mapDiagnostics(source: string, raw: RawDiagnostic[]): Diagnostic[] {
  const inCode = (range: { start: number } | null): range is { start: number; end: number } => range !== null && range.start <= source.length;
  const severities: Record<number, Severity | undefined> = { 1: 'info', 2: 'warning', 3: 'error' };
  return raw.flatMap((entry): Diagnostic[] => {
    const severity = severities[entry.category];
    if (!severity) return [];
    const shown = inCode(entry.range) ? entry.range : inCode(entry.related) ? entry.related : null;
    const start = shown && lineAndColumn(source, shown.start);
    const end = shown && lineAndColumn(source, Math.min(shown.end, source.length));
    return [
      {
        severity,
        code: entry.code,
        message: entry.message,
        line: start?.line ?? null,
        column: start?.column ?? null,
        endLine: end?.line ?? null,
        endColumn: end?.column ?? null,
        inContract: entry.range !== null && !inCode(entry.range),
      },
    ];
  });
}

/** What a function is doing when it needs the host for something, for the names the compiler imports. */
const IMPORT_CALLS: Record<string, string> = { seed: 'Math.random()', 'Date.now': 'Date.now()', 'console.log': 'console.log()', trace: 'trace()' };

/** The message for a function that would need something from the host. A Froglet function may import nothing. */
export function describeImports(imports: ImportedName[]): string {
  const needs = imports.map(({ module, name }) => {
    const call = IMPORT_CALLS[name];
    return call ? `${call} (${module}.${name})` : `${module}.${name}`;
  });
  return `This function needs ${needs.join(' and ')} from the host. A Froglet function may not import anything, so it has no clock, randomness, logging, or network. Remove those calls.`;
}

export interface CompilerDeps {
  /** Starts a Worker running COMPILER_WORKER_SOURCE. */
  spawn: () => WorkerLike;
  /** Where the compiler bundle is, as an absolute URL. */
  compilerUrl: string;
  /** How long to wait for the compiler to load, in ms. */
  loadTimeoutMs?: number;
  /** How long a compile may take before it is stopped, in ms. */
  compileTimeoutMs?: number;
}

export function createCompiler(deps: CompilerDeps): Compiler {
  let worker: WorkerLike | undefined;
  let ready: Promise<void> | undefined;
  let queue: Promise<unknown> = Promise.resolve();
  let nextId = 0;

  const stop = () => {
    worker?.terminate();
    worker = undefined;
    ready = undefined;
  };

  function start(): Promise<void> {
    if (ready) return ready;
    const current = (worker = deps.spawn());
    const loading = new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => {
        if (worker === current) stop();
        reject(new Error('the compiler did not load in time'));
      }, deps.loadTimeoutMs ?? 60_000);
      current.onmessage = ({ data }) => {
        if (data.type === 'loaded') {
          clearTimeout(timer);
          resolve();
        } else if (data.type === 'unavailable') {
          clearTimeout(timer);
          if (worker === current) stop();
          reject(new Error(data.error));
        }
      };
      current.onerror = (event) => {
        clearTimeout(timer);
        if (worker === current) stop();
        reject(new Error(event.message || 'the compiler could not start'));
      };
      current.postMessage({ type: 'load', url: deps.compilerUrl });
    });
    loading.catch(() => {});
    ready = loading;
    return loading;
  }

  async function compileNow(source: string, onPhase?: (phase: Phase) => void): Promise<CompileResult> {
    if (source.length > MAX_SOURCE_CHARS) return { ok: false, kind: 'too-large', message: `The source is ${source.length} characters. The most the editor takes is ${MAX_SOURCE_CHARS}.`, ms: 0 };
    onPhase?.('loading');
    try {
      await start();
    } catch (error) {
      return { ok: false, kind: 'unavailable', message: `The compiler could not load: ${error instanceof Error ? error.message : String(error)}`, ms: 0 };
    }
    onPhase?.('compiling');
    const current = worker!;
    const id = (nextId += 1);
    const reply = await new Promise<any>((resolve) => {
      const timer = setTimeout(() => {
        if (worker === current) stop();
        resolve({ timedOut: true });
      }, deps.compileTimeoutMs ?? 15_000);
      current.onmessage = ({ data }) => {
        if (data.type === 'result' && data.id === id) {
          clearTimeout(timer);
          resolve(data);
        }
      };
      current.onerror = (event) => {
        clearTimeout(timer);
        if (worker === current) stop();
        resolve({ crashed: event.message || 'the compiler stopped unexpectedly' });
      };
      current.postMessage({ type: 'compile', id, program: `${source}\n${CONTRACT}`, args: COMPILER_ARGS });
    });

    const ms = reply.ms ?? 0;
    if (reply.timedOut) return { ok: false, kind: 'timeout', message: `Compiling took longer than ${(deps.compileTimeoutMs ?? 15_000) / 1000} seconds, so it was stopped.`, ms };
    if (reply.crashed) return { ok: false, kind: 'crashed', message: `The compiler stopped: ${reply.crashed}`, ms };
    const diagnostics = mapDiagnostics(source, reply.diagnostics ?? []);
    if (diagnostics.some((diagnostic) => diagnostic.severity === 'error')) return { ok: false, kind: 'errors', diagnostics, ms };
    if (!reply.binary) return { ok: false, kind: 'crashed', message: 'The compiler finished without a module.', ms };

    const module: Uint8Array = reply.binary;
    if (module.length > MAX_MODULE_BYTES) return { ok: false, kind: 'too-large', message: `The module is ${module.length} bytes. The most a function may be is ${MAX_MODULE_BYTES}.`, ms };
    const imports = WebAssembly.Module.imports(await WebAssembly.compile(module as BufferSource));
    if (imports.length) return { ok: false, kind: 'imports', imports: imports.map(({ module: from, name }) => ({ module: from, name })), message: describeImports(imports), ms };
    return { ok: true, module, warnings: diagnostics, ms };
  }

  return {
    preload: () => void start().catch(() => {}),
    compile(source, onPhase) {
      const run = queue.then(() => compileNow(source, onPhase));
      queue = run.catch(() => undefined);
      return run;
    },
    dispose: stop,
  };
}

/** A module Worker in the browser, started from the program above, seen through the small surface the compiler uses. */
export function browserCompilerWorker(): WorkerLike {
  const url = URL.createObjectURL(new Blob([COMPILER_WORKER_SOURCE], { type: 'text/javascript' }));
  const worker = new Worker(url, { type: 'module' });
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
