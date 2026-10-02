import { afterAll, describe, expect, it, vi } from 'vitest';
import { COMPILER_ARGS, COMPILER_WORKER_SOURCE, CONTRACT, MAX_SOURCE_CHARS, createCompiler, describeImports, lineAndColumn, mapDiagnostics, type CompileResult } from '../playground/compiler';
import { FUNCTIONS } from '../playground/functions';
import { PLAYGROUND_LIMITS } from '../playground/protocol';
import type { WorkerLike } from '../playground/runner';
import { admitModule } from '../playground/wasm-module';
import { callModule, compilerUrl, kernel, newCompiler, nodeRunner, samples } from './playground-helpers';

// The real compiler, in a real Worker, over the real bundle the site build makes: what these tests compile is what the page
// compiles. Loading the compiler takes a second or so, so one is made for the file and used throughout.
vi.setConfig({ testTimeout: 60_000 });
const compiler = newCompiler();
afterAll(() => compiler.dispose());

const fn = (id: string) => FUNCTIONS.find((candidate) => candidate.id === id)!;
const compile = (source: string) => compiler.compile(source);
async function built(source: string) {
  const result = await compile(source);
  if (!result.ok) throw new Error(`did not compile: ${JSON.stringify(result)}`);
  return result;
}
const exportsOf = async (bytes: Uint8Array) => WebAssembly.Module.exports(await WebAssembly.compile(bytes as BufferSource)).map((entry) => `${entry.name}:${entry.kind}`).sort();

describe('compiling the functions the page starts from', () => {
  it.each(FUNCTIONS.map((info) => info.id))('%s compiles, with no warning, to a module a node would accept', async (id) => {
    const result = await built(fn(id).code);
    expect(result.warnings).toEqual([]);
    const compiled = await WebAssembly.compile(result.module as BufferSource);
    expect(WebAssembly.Module.imports(compiled)).toEqual([]);
    expect(await exportsOf(result.module)).toEqual(['alloc:function', 'memory:memory', 'run:function']);
    expect(result.module.length).toBeLessThan(8_192);
    const admitted = await admitModule(result.module, PLAYGROUND_LIMITS);
    // A function with no static data starts with no memory at all, and its allocator grows it.
    expect(admitted.memory.min).toBeLessThanOrEqual(1);
    expect(admitted.memory).toMatchObject({ declaredMax: null, cappedTo: 128 });
  });

  it('gives the same module for the same source, so the same source has the same hash', async () => {
    const first = await built(fn('adder').code);
    const again = await built(fn('adder').code);
    expect(kernel.hashBytes(again.module)).toBe(kernel.hashBytes(first.module));
    expect(Array.from(again.module)).toEqual(Array.from(first.module));
  });

  it('puts the contract after the code, so a place in the code is a place in the editor', () => {
    expect(CONTRACT).toContain('export function alloc(len: i32): usize');
    expect(CONTRACT).toContain('export function run(pointer: usize, len: i32): i64');
    expect(CONTRACT.endsWith('\n')).toBe(false);
    for (const info of FUNCTIONS) {
      expect(info.code, info.id).not.toMatch(/export function (alloc|run)\b/);
      expect(info.code, info.id).toMatch(/^function respond\(request: string\): string/m);
    }
  });

  it('compiles with the options it documents', () => {
    expect(COMPILER_ARGS).toEqual(['--outFile', 'binary', '--optimizeLevel', '3', '--shrinkLevel', '2', '--runtime', 'stub', '--use', 'abort=', '--noColors']);
  });
});

describe('the ports answer what the Rust services answer', () => {
  // A small seeded generator, so a failure can be reproduced.
  let seed = 20260929;
  const rand = () => (seed = (seed * 1664525 + 1013904223) >>> 0) / 2 ** 32;
  const pick = <T,>(items: T[]) => items[Math.floor(rand() * items.length)];
  const MAX = 9007199254740991;
  const spaces = () => pick(['', ' ', '  ', '\n', '\t ']);

  async function sameAnswers(id: 'adder' | 'fibonacci', inputs: string[]) {
    const port = (await built(fn(id).code)).module;
    const rust = await samples[id].bytes();
    const different: string[] = [];
    for (const text of inputs) {
      const [mine, theirs] = [await callModule(port, text), await callModule(rust, text)];
      if (mine !== theirs) different.push(`${text}\n  port: ${mine}\n  rust: ${theirs}`);
    }
    expect(different).toEqual([]);
  }

  it('gives the adder\'s answers on hand-picked inputs, malformed and boundary ones included', async () => {
    await sameAnswers('adder', [
      '{"a":6,"b":7}', '{"a": 6, "b": 7}', '{ "a" : 6 , "b" : 7 }', '{"b":7,"a":6}', '{"a":-4,"b":9}', '{"a":0,"b":0}', '{"a":-0,"b":5}',
      '{"a":1}', '{"b":1}', '{}', '', 'not json', '[]', 'null', '{"a":"6","b":"7"}', '{"a":1.5,"b":2}', '{"a":1e3,"b":2}', '{"a":+5,"b":2}',
      '{"a":--5,"b":2}', '{"a":5-3,"b":2}', '{"a":-,"b":2}', '{"a":007,"b":2}', '{"a":,"b":2}', '{"a" 6,"b":2}', '{"a":6 "b":2}',
      '{"x":"\\"a\\": 5","a":1,"b":2}', '{"a":1,"a":2,"b":3}', '{"a":1,"b":2,"c":3}', '{"c":{"a":9},"a":1,"b":2}',
      `{"a":${MAX},"b":0}`, `{"a":-${MAX},"b":0}`, `{"a":${MAX},"b":1}`, `{"a":${MAX},"b":-1}`, `{"a":${MAX + 2},"b":1}`,
      '{"a":94906265,"b":94906265}', '{"a":94906266,"b":94906266}', '{"a":94906267,"b":94906267}',
      `{"a":1,"b":${MAX}}`, `{"a":2,"b":${MAX}}`, `{"a":-1,"b":${MAX}}`, `{"a":${MAX},"b":${MAX}}`, '{"a":123456789,"b":73000000}',
      '{"a":é,"b":1}', '{"a":1,"b":2}\n', '﻿{"a":1,"b":2}', '{"a":\n1,"b":\t2}',
    ]);
  });

  it('gives the adder\'s answers on 300 random inputs, in either key order and with any spacing', async () => {
    const inputs: string[] = [];
    for (let count = 0; count < 300; count++) {
      const magnitude = pick([10, 1000, 1e6, 1e9, 1e12, 9e15]);
      const number = () => Math.trunc((rand() * 2 - 1) * magnitude);
      const [a, b] = [number(), number()];
      const keys: Array<[string, number]> = rand() < 0.5 ? [['a', a], ['b', b]] : [['b', b], ['a', a]];
      inputs.push(`{${keys.map(([key, value]) => `${spaces()}"${key}"${spaces()}:${spaces()}${value}`).join(',')}${spaces()}}`);
    }
    await sameAnswers('adder', inputs);
  });

  it('gives the Fibonacci function\'s answers on hand-picked and on random inputs', async () => {
    const random: string[] = [];
    for (let count = 0; count < 150; count++) random.push(`{${spaces()}"n"${spaces()}:${spaces()}${Math.trunc((rand() * 2 - 0.3) * 100)}${spaces()}}`);
    await sameAnswers('fibonacci', [
      '{"n":0}', '{"n":1}', '{"n":2}', '{"n":10}', '{"n":50}', '{"n":77}', '{"n":78}', '{"n":79}', '{"n":80}', '{"n":100}', '{"n":-1}', '{"n":-78}',
      '{"n": 10 }', '{ "n" : 10 }', '{"n":"10"}', '{"n":1.5}', '{"n":1e2}', '{"n":}', '{}', '', 'not json', '{"m":5}', '{"n":10,"n":20}',
      '{"x":"\\"n\\": 5","n":3}', '{"n":007}', '{"n":+5}', '{"n":5-3}', `{"n":${MAX}}`, `{"n":-${MAX}}`,
      ...random,
    ]);
  });

  it('differs in one way only: a number beyond what a browser holds exactly counts as missing', async () => {
    const port = (await built(fn('fibonacci').code)).module;
    const rust = await samples.fibonacci.bytes();
    for (const text of [`{"n":${MAX + 2}}`, '{"n":99999999999999999}']) {
      expect(await callModule(port, text)).toBe('{"error":"send an object like {\\"n\\": 10}"}');
      expect(await callModule(rust, text)).toBe('{"error":"n must be between 0 and 78"}');
    }
  });
});

describe('editing a function', () => {
  it('changes what it answers, and its hash', async () => {
    const original = await built(fn('adder').code);
    const edited = await built(fn('adder').code.replace('`{"sum":${sum},"product":${a * b}}`', '`{"sum":${a * a + b},"product":${a * b},"note":"edited"}`'));
    expect(edited.module.length).toBeGreaterThan(0);
    expect(await callModule(original.module, '{"a":6,"b":7}')).toBe('{"sum":13,"product":42}');
    expect(await callModule(edited.module, '{"a":6,"b":7}')).toBe('{"sum":43,"product":42,"note":"edited"}');
    expect(kernel.hashBytes(edited.module)).not.toBe(kernel.hashBytes(original.module));
  });

  it('can be a function of its own, in the language\'s ordinary parts', async () => {
    const source = `export function respond(request: string): string {
  const words = request.split(",");
  const counts = new Map<string, i32>();
  for (let i = 0; i < words.length; i++) counts.set(words[i], counts.has(words[i]) ? counts.get(words[i]) + 1 : 1);
  return '{"distinct":' + counts.size.toString() + ',"total":' + words.length.toString() + '}';
}`;
    const { module } = await built(source);
    expect(await callModule(module, 'a,b,a,c,b,a')).toBe('{"distinct":3,"total":6}');
  });

  it('answers with the text it was given when it is the echo function', async () => {
    const { module } = await built(fn('echo').code);
    expect(await callModule(module, '{"hello":"world"}')).toBe('{"hello":"world"}');
    expect(await callModule(module, '{"é":"ü\\n"}')).toBe('{"é":"ü\\n"}');
  });

  it('sees exactly the text it is given: the canonical form, which is shorter than what a person types', async () => {
    const { module } = await built(`export function respond(request: string): string {
  return '{"length":' + request.length.toString() + '}';
}`);
    const typed = '{ "b": 7,\n  "a": [3, {"y": 1, "x": 2}] }';
    const canonical = kernel.canonicalize(JSON.parse(typed));
    expect(canonical).toBe('{"a":[3,{"x":2,"y":1}],"b":7}');
    expect(canonical.length).toBeLessThan(typed.length);
    expect(await callModule(module, canonical)).toBe(`{"length":${canonical.length}}`);
  });

  it('traps on a failed assert, which is a failure the receipt records, not a hang', async () => {
    const { module } = await built('export function respond(request: string): string { assert(request.length > 100); return request; }');
    const outcome = await nodeRunner()((await admitModule(module, PLAYGROUND_LIMITS)).bytes, '{}', PLAYGROUND_LIMITS);
    expect(outcome.ok).toBe(false);
    if (!outcome.ok) expect(outcome.error).toMatch(/unreachable/i);
  });

  it('is stopped at the time limit when it never returns', async () => {
    const { module } = await built(fn('never-ends').code);
    const limits = { ...PLAYGROUND_LIMITS, max_runtime_ms: 300 };
    const outcome = await nodeRunner()((await admitModule(module, limits)).bytes, '{}', limits);
    expect(outcome).toMatchObject({ ok: false, timedOut: true });
  });

  it('answers once the loop is taken out', async () => {
    const { module } = await built(fn('never-ends').code.replace('while (true) {} // remove this line and the function answers\n', ''));
    expect(await callModule(module, '{}')).toBe('{"finished":true}');
  });
});

describe('a mistake gets a place and a reason', () => {
  const at = (source: string, needle: string) => lineAndColumn(source, source.indexOf(needle));
  const errors = (result: CompileResult) => {
    expect(result.ok).toBe(false);
    if (result.ok || result.kind !== 'errors') throw new Error(`expected compile errors, got ${JSON.stringify(result).slice(0, 200)}`);
    return result.diagnostics;
  };

  it('points at a name that is not there', async () => {
    const source = fn('adder').code.replace('${a * b}', '${a * bee}');
    const [first, ...rest] = errors(await compile(source));
    expect(rest).toEqual([]);
    expect(first).toMatchObject({ severity: 'error', code: 2304, message: "Cannot find name 'bee'.", inContract: false });
    expect({ line: first.line, column: first.column }).toEqual(at(source, 'bee'));
    expect(source.split('\n')[first.line! - 1]).toContain('bee');
  });

  it('points at a wrong type', async () => {
    const source = 'export function respond(request: string): string {\n  return 42;\n}';
    const [first] = errors(await compile(source));
    expect(first).toMatchObject({ code: 2322, line: 2, column: 10, endLine: 2, endColumn: 12 });
    expect(first.message).toContain("Type 'i32' is not assignable");
  });

  it('counts a place correctly after characters outside the basic plane, and with Windows line endings', async () => {
    const source = '// a function \u{1f438} with a tricky comment: é\r\nexport function respond(request: string): string {\r\n  return request.lenght;\r\n}';
    const [first] = errors(await compile(source));
    expect({ line: first.line, column: first.column }).toEqual(at(source, 'lenght'));
    expect(first.line).toBe(3);
  });

  it('says when the trouble is after the code, as it is for an open bracket', async () => {
    const diagnostics = errors(await compile('export function respond(request: string): string {\n  return request;'));
    expect(diagnostics.length).toBeGreaterThan(0);
    expect(diagnostics.every((diagnostic) => diagnostic.inContract && diagnostic.line === null)).toBe(true);
  });

  it('shows the place in the code when the code redefines a name the contract uses', async () => {
    const source = 'export function respond(request: string): string { return request; }\nexport function run(a: i32): i32 { return a; }';
    const [first] = errors(await compile(source));
    expect(first).toMatchObject({ code: 2300, inContract: true, line: 2 });
    expect(first.message).toContain("'run'");
  });

  it('does not invent problems for a function that is fine', async () => {
    expect((await built(fn('echo').code)).warnings).toEqual([]);
  });
});

describe('a function that needs something from the host', () => {
  const needs = async (call: string) => {
    const result = await compile(`export function respond(request: string): string { ${call} return request; }`);
    expect(result.ok).toBe(false);
    if (result.ok || result.kind !== 'imports') throw new Error(`expected imports, got ${JSON.stringify(result).slice(0, 200)}`);
    return result;
  };

  it.each([
    ['Math.random().toString();', 'env', 'seed', 'Math.random()'],
    ['Date.now().toString();', 'env', 'Date.now', 'Date.now()'],
    ['console.log("hi");', 'env', 'console.log', 'console.log()'],
    ['trace("hi");', 'env', 'trace', 'trace()'],
  ])('is refused for %s, and says which call to remove', async (call, module, name, shown) => {
    const result = await needs(call);
    expect(result.imports).toEqual([{ module, name }]);
    expect(result.message).toContain(shown);
    expect(result.message).toContain('may not import anything');
  });

  it('names an import it does not know by its own name', async () => {
    const result = await compile('@external("host", "clock") declare function clock(): i32;\nexport function respond(request: string): string { return clock().toString(); }');
    expect(result).toMatchObject({ ok: false, kind: 'imports', imports: [{ module: 'host', name: 'clock' }] });
    expect(describeImports([{ module: 'host', name: 'clock' }])).toContain('host.clock');
  });
});

describe('the limits on what is compiled', () => {
  it('refuses source that is too long, without waking the compiler', async () => {
    let spawned = 0;
    const untouched = createCompiler({ spawn: () => (spawned++, nodeCompilerSpawnNever()), compilerUrl });
    const result = await untouched.compile('x'.repeat(MAX_SOURCE_CHARS + 1));
    expect(result).toMatchObject({ ok: false, kind: 'too-large' });
    expect(spawned).toBe(0);
    untouched.dispose();
  });
});

/** A Worker that starts and never answers. */
function nodeCompilerSpawnNever(): WorkerLike {
  return { onmessage: null, onerror: null, postMessage: () => {}, terminate: () => {} };
}

describe('the compiler adapter, with Workers that misbehave', () => {
  /** A Worker that answers as the test says. */
  const scripted = (answer: (message: any, worker: WorkerLike) => void) => {
    const events: string[] = [];
    const spawn = (): WorkerLike => {
      events.push('spawn');
      const worker: WorkerLike = {
        onmessage: null,
        onerror: null,
        terminate: () => void events.push('terminate'),
        postMessage: (message: any) => {
          events.push(message.type);
          queueMicrotask(() => answer(message, worker));
        },
      };
      return worker;
    };
    return { events, spawn };
  };
  const send = (worker: WorkerLike, data: unknown) => worker.onmessage?.({ data });
  const okReply = (message: any, worker: WorkerLike) => {
    if (message.type === 'load') send(worker, { type: 'loaded' });
    if (message.type === 'compile') send(worker, { type: 'result', id: message.id, binary: new Uint8Array([0, 0x61, 0x73, 0x6d, 1, 0, 0, 0]), diagnostics: [], ms: 5 });
  };

  it('loads once, then compiles as often as it is asked', async () => {
    const { events, spawn } = scripted(okReply);
    const scriptedCompiler = createCompiler({ spawn, compilerUrl: 'https://example.invalid/asc.js' });
    const phases: string[] = [];
    expect((await scriptedCompiler.compile('a', (phase) => phases.push(phase))).ok).toBe(true); // the empty module is valid, and imports nothing
    expect((await scriptedCompiler.compile('b')).ok).toBe(true);
    expect(events.filter((event) => event === 'load')).toHaveLength(1);
    expect(events.filter((event) => event === 'compile')).toHaveLength(2);
    expect(phases).toEqual(['loading', 'compiling']);
  });

  it('starts loading when asked to, and a compile then finds it ready', async () => {
    const { events, spawn } = scripted(okReply);
    const scriptedCompiler = createCompiler({ spawn, compilerUrl: 'https://example.invalid/asc.js' });
    scriptedCompiler.preload();
    scriptedCompiler.preload();
    await scriptedCompiler.compile('a');
    expect(events.filter((event) => event === 'load')).toHaveLength(1);
    expect(events.filter((event) => event === 'spawn')).toHaveLength(1);
  });

  it('passes the compiler\'s address on, and only that', async () => {
    const seen: any[] = [];
    const { spawn } = scripted((message, worker) => {
      seen.push(message);
      okReply(message, worker);
    });
    await createCompiler({ spawn, compilerUrl: 'https://example.invalid/asc.js' }).compile('source');
    expect(seen[0]).toEqual({ type: 'load', url: 'https://example.invalid/asc.js' });
    expect(seen[1]).toMatchObject({ type: 'compile', program: `source\n${CONTRACT}`, args: COMPILER_ARGS });
  });

  it('reports a compiler that cannot load, and tries again the next time', async () => {
    let failing = true;
    const { events, spawn } = scripted((message, worker) => {
      if (message.type === 'load') send(worker, failing ? { type: 'unavailable', error: 'Failed to fetch' } : { type: 'loaded' });
      else okReply(message, worker);
    });
    const scriptedCompiler = createCompiler({ spawn, compilerUrl: 'https://example.invalid/asc.js' });
    expect(await scriptedCompiler.compile('a')).toMatchObject({ ok: false, kind: 'unavailable', message: 'The compiler could not load: Failed to fetch' });
    failing = false;
    expect((await scriptedCompiler.compile('a')).ok).toBe(true);
    expect(events.filter((event) => event === 'spawn')).toHaveLength(2);
    expect(events).toContain('terminate');
  });

  it('gives up on a compiler that never loads', async () => {
    const { events, spawn } = scripted(() => {});
    const scriptedCompiler = createCompiler({ spawn, compilerUrl: 'x', loadTimeoutMs: 30 });
    expect(await scriptedCompiler.compile('a')).toMatchObject({ ok: false, kind: 'unavailable', message: 'The compiler could not load: the compiler did not load in time' });
    expect(events).toContain('terminate');
  });

  it('stops a compile that takes too long, and starts a new compiler for the next one', async () => {
    let hang = true;
    const { events, spawn } = scripted((message, worker) => {
      if (message.type === 'compile' && hang) return;
      okReply(message, worker);
    });
    const scriptedCompiler = createCompiler({ spawn, compilerUrl: 'x', compileTimeoutMs: 30 });
    expect(await scriptedCompiler.compile('a')).toMatchObject({ ok: false, kind: 'timeout', message: 'Compiling took longer than 0.03 seconds, so it was stopped.' });
    expect(events).toContain('terminate');
    hang = false;
    expect((await scriptedCompiler.compile('a')).ok).toBe(true);
    expect(events.filter((event) => event === 'spawn')).toHaveLength(2);
  });

  it('reports a compiler that crashes', async () => {
    const { spawn } = scripted((message, worker) => {
      if (message.type === 'load') send(worker, { type: 'loaded' });
      else queueMicrotask(() => worker.onerror?.({ message: 'the worker exited with code 1' }));
    });
    expect(await createCompiler({ spawn, compilerUrl: 'x' }).compile('a')).toMatchObject({ ok: false, kind: 'crashed', message: 'The compiler stopped: the worker exited with code 1' });
  });

  it('reports a compile that fails inside the compiler', async () => {
    const { spawn } = scripted((message, worker) => {
      if (message.type === 'load') send(worker, { type: 'loaded' });
      else send(worker, { type: 'result', id: message.id, binary: null, diagnostics: [], crashed: 'Maximum call stack size exceeded', ms: 3 });
    });
    expect(await createCompiler({ spawn, compilerUrl: 'x' }).compile('a')).toMatchObject({ ok: false, kind: 'crashed', message: 'The compiler stopped: Maximum call stack size exceeded' });
  });

  it('compiles one source at a time, in the order they were asked for', async () => {
    const order: string[] = [];
    const { spawn } = scripted((message, worker) => {
      if (message.type === 'load') send(worker, { type: 'loaded' });
      else {
        order.push(message.program.split('\n')[0]);
        setTimeout(() => send(worker, { type: 'result', id: message.id, binary: new Uint8Array([0, 0x61, 0x73, 0x6d, 1, 0, 0, 0]), diagnostics: [], ms: 1 }), message.program.startsWith('slow') ? 40 : 1);
      }
    });
    const scriptedCompiler = createCompiler({ spawn, compilerUrl: 'x' });
    const done: string[] = [];
    await Promise.all([scriptedCompiler.compile('slow').then(() => done.push('slow')), scriptedCompiler.compile('quick').then(() => done.push('quick'))]);
    expect(order).toEqual(['slow', 'quick']);
    expect(done).toEqual(['slow', 'quick']);
  });
});

describe('the compiler\'s diagnostics, as data', () => {
  it('counts lines and columns from one, by characters, whatever the line ending', () => {
    expect(lineAndColumn('ab\ncd', 0)).toEqual({ line: 1, column: 1 });
    expect(lineAndColumn('ab\ncd', 4)).toEqual({ line: 2, column: 2 });
    expect(lineAndColumn('ab\r\ncd', 4)).toEqual({ line: 2, column: 1 });
    expect(lineAndColumn('ab', 99)).toEqual({ line: 1, column: 3 });
  });

  it('keeps errors, warnings and information, and drops the rest', () => {
    const raw = [3, 2, 1, 0].map((category) => ({ category, code: 1, message: `m${category}`, range: { start: 0, end: 1 }, related: null }));
    expect(mapDiagnostics('code', raw).map((diagnostic) => diagnostic.severity)).toEqual(['error', 'warning', 'info']);
  });

  it('puts what is reported at the end of the code in the code, and what is reported after it in the contract', () => {
    const source = 'abc';
    const [end, after, related] = mapDiagnostics(source, [
      { category: 3, code: 1, message: 'at the end', range: { start: 3, end: 3 }, related: null },
      { category: 3, code: 2, message: 'after', range: { start: 4, end: 8 }, related: null },
      { category: 3, code: 3, message: 'related', range: { start: 10, end: 12 }, related: { start: 1, end: 2 } },
    ]);
    expect(end).toMatchObject({ line: 1, column: 4, inContract: false });
    expect(after).toMatchObject({ line: null, column: null, inContract: true });
    expect(related).toMatchObject({ line: 1, column: 2, inContract: true });
  });

  it('gives a diagnostic with no place no place', () => {
    expect(mapDiagnostics('abc', [{ category: 3, code: 9, message: 'somewhere', range: null, related: null }])).toEqual([
      { severity: 'error', code: 9, message: 'somewhere', line: null, column: null, endLine: null, endColumn: null, inContract: false },
    ]);
  });
});

describe('the Worker program', () => {
  it('reaches for nothing but the compiler it is told to load', () => {
    expect(COMPILER_WORKER_SOURCE).not.toMatch(/fetch|XMLHttpRequest|WebSocket|importScripts|indexedDB|localStorage|sendBeacon/);
    expect(COMPILER_WORKER_SOURCE.match(/\bimport\(/g)).toHaveLength(1);
    expect(COMPILER_WORKER_SOURCE).toContain('import(url)');
    expect(COMPILER_WORKER_SOURCE).not.toMatch(/https?:\/\//);
    expect(COMPILER_WORKER_SOURCE).toContain("readFile: (name) => (Object.prototype.hasOwnProperty.call(files, name) ? files[name] : null)");
    expect(COMPILER_WORKER_SOURCE).toContain('listFiles: () => []');
  });
});
