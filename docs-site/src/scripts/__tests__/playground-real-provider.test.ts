import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { runExchange, type ExchangeOutcome } from '../playground/consumer';
import { FUNCTIONS } from '../playground/functions';
import { PLAYGROUND_LIMITS } from '../playground/protocol';
import { createProvider } from '../playground/provider';
import type { Transport } from '../playground/types';
import { createWire } from '../playground/wire';
import { kernel, newCompiler, nodeRunner, verifier } from './playground-helpers';
import { freePort, startNode, type RunningNode } from './real-node';

// Opt-in: a real froglet-node runs what the editor compiles.
//
// The editor turns the source in the page into a WebAssembly module, and the page says a real node accepts those same bytes,
// runs them, and reports the same module hash. This is what that rests on. A real froglet-node, run as provider and runtime
// together, is given the modules the compiler makes: the starting functions, an edited one, and some that fail in the ways a
// function can. Each is published through the node's provider-control API, and called by the consumer that runs in the tab, over
// local HTTP, with an identity of its own. What the node answers is compared with what the tab's runner answers for the same
// bytes, and the node's receipt with the hash of the bytes the compiler produced.
//
// Each function is run twice, with the same consumer: on the real node, and on the provider that runs in the tab. Where the two
// should say the same thing, the test holds them to it. Where they cannot, it says what differs and why.
//
// Run it after building the node (see real-node.ts):
//   FROGLET_PLAYGROUND_REAL_PROVIDER=1 npx vitest --run src/scripts/__tests__/playground-real-provider.test.ts
// Without the variable the tests are skipped, as they need the binary.

const enabled = process.env.FROGLET_PLAYGROUND_REAL_PROVIDER === '1';
const fn = (id: string) => FUNCTIONS.find((candidate) => candidate.id === id)!;
/** The tab's time limit for a function, and the node's (FROGLET_EXECUTION_TIMEOUT_SECS below): a runaway is stopped in about a second. */
const TAB_LIMITS = { ...PLAYGROUND_LIMITS, max_runtime_ms: 1000 };

describe.skipIf(!enabled)('a real froglet-node running what the editor compiles', () => {
  const compiler = newCompiler();
  const runInTab = nodeRunner();
  let node: RunningNode | undefined;
  let providerUrl: string;
  let transport: Transport;

  beforeAll(async () => {
    const [providerPort, runtimePort] = [await freePort(), await freePort()];
    providerUrl = `http://127.0.0.1:${providerPort}`;
    // A short time limit, so the function that never ends is stopped quickly.
    node = await startNode(
      'provider',
      { FROGLET_NODE_ROLE: 'dual', FROGLET_LISTEN_ADDR: `127.0.0.1:${providerPort}`, FROGLET_RUNTIME_LISTEN_ADDR: `127.0.0.1:${runtimePort}`, FROGLET_EXECUTION_TIMEOUT_SECS: '1' },
      providerUrl,
    );
    transport = async ({ method, path, body }) => {
      const response = await fetch(`${providerUrl}${path}`, { method, headers: body === undefined ? {} : { 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) });
      return { status: response.status, body: await response.json().catch(() => null) };
    };
  }, 120_000);

  afterAll(async () => {
    compiler.dispose();
    await node?.stop();
  });

  /** Compiles `source` in the page's own compiler. */
  async function compile(source: string) {
    const result = await compiler.compile(source);
    if (!result.ok) throw new Error(`did not compile: ${JSON.stringify(result).slice(0, 300)}`);
    return result.module;
  }

  /** Publishes `module` on the real node under `serviceId`, through its provider-control API. */
  async function publishOnNode(serviceId: string, module: Uint8Array) {
    const response = await fetch(`${providerUrl}/v1/provider/artifacts/publish`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: `Bearer ${node!.token('froglet-control')}` },
      body: JSON.stringify({
        service_id: serviceId,
        runtime: 'wasm',
        package_kind: 'inline_module',
        entrypoint_kind: 'handler',
        entrypoint: 'run',
        contract_version: 'froglet.wasm.run_json.v1',
        wasm_module_hex: Array.from(module, (byte) => byte.toString(16).padStart(2, '0')).join(''),
        summary: `built in the page: ${serviceId}`,
        mode: 'sync',
        price_sats: 0,
        publication_state: 'active',
      }),
    });
    const body = await response.text();
    expect(response.status, `the node refused the module: ${body.slice(0, 400)}`).toBe(201);
  }

  /** The provider that runs in the tab, over the same module, reached through the page's own wire. */
  async function tabTransport(serviceId: string, module: Uint8Array): Promise<Transport> {
    const provider = createProvider({ kernel, verifier, runModule: runInTab, limits: TAB_LIMITS });
    await provider.publish({ serviceId, summary: `built in the page: ${serviceId}`, module });
    return createWire((request) => provider.handle(request)).transport;
  }

  const exchange = (transport: Transport, serviceId: string, inputText: string): Promise<ExchangeOutcome> =>
    runExchange({ kernel, verifier, transport, requester: kernel.newIdentity(), serviceId, inputText, pollIntervalMs: 50 });

  /** Compiles `source`, publishes it on the node and in the tab, and calls both from the same consumer with the same input. */
  async function bothRun(serviceId: string, source: string, inputText: string) {
    const module = await compile(source);
    await publishOnNode(serviceId, module);
    const [real, tab] = [await exchange(transport, serviceId, inputText), await exchange(await tabTransport(serviceId, module), serviceId, inputText)];
    return { module, moduleHash: kernel.hashBytes(module), real, tab };
  }

  const finished = (outcome: ExchangeOutcome, expected: 'succeeded' | 'failed') => {
    if (!outcome.ok) throw new Error(`the exchange was refused at ${outcome.stage} by ${outcome.refusedBy}: ${outcome.reason}`);
    expect(outcome.status, JSON.stringify(outcome.failure)).toBe(expected);
    expect(outcome.chain.valid).toBe(true);
    return outcome;
  };

  it.each([
    ['adder', '{"a": 6, "b": 7}'],
    ['adder', '{"b": -5, "a": 2}'],
    ['fibonacci', '{"n": 50}'],
    ['echo', '{"hello": "world", "list": [1, 2, 3]}'],
  ])('runs the %s function on %s: the node and the tab give the same answer, and the node names the compiled bytes', async (id, inputText) => {
    const serviceId = `${fn(id).serviceId}.${Math.random().toString(36).slice(2, 8)}`;
    const { moduleHash, real, tab } = await bothRun(serviceId, fn(id).code, inputText);
    const [onNode, inTab] = [finished(real, 'succeeded'), finished(tab, 'succeeded')];
    expect(onNode.result).toEqual(inTab.result);
    // Each one says it ran the bytes the compiler made, and both say so with the same hash.
    expect(onNode.artifacts.receipt.payload.executor.module_hash).toBe(moduleHash);
    expect(inTab.artifacts.receipt.payload.executor.module_hash).toBe(moduleHash);
  }, 60_000);

  it('runs an edited function, and reports a module hash that is not the original\'s', async () => {
    const original = await bothRun('demo.adder.original', fn('adder').code, '{"a": 6, "b": 7}');
    const edited = await bothRun('demo.adder.edited', fn('adder').code.replace('const sum = a + b;', 'const sum = a * a + b;'), '{"a": 6, "b": 7}');
    expect(edited.moduleHash).not.toBe(original.moduleHash);
    expect(finished(original.real, 'succeeded').result).toEqual({ sum: 13, product: 42 });
    expect(finished(edited.real, 'succeeded').result).toEqual({ sum: 43, product: 42 });
    expect(finished(edited.tab, 'succeeded').result).toEqual({ sum: 43, product: 42 });
    expect(finished(original.real, 'succeeded').artifacts.receipt.payload.executor.module_hash).toBe(original.moduleHash);
    expect(finished(edited.real, 'succeeded').artifacts.receipt.payload.executor.module_hash).toBe(edited.moduleHash);
  }, 60_000);

  it('gives the function the same input bytes as the tab does: the canonical text, not the text as typed', async () => {
    const source = "function respond(request: string): string {\n  return '{\"length\":' + request.length.toString() + '}';\n}";
    const { real, tab } = await bothRun('demo.length', source, '{ "b": 7,   "a": 6 }');
    expect(finished(real, 'succeeded').result).toEqual({ length: '{"a":6,"b":7}'.length });
    expect(finished(tab, 'succeeded').result).toEqual({ length: '{"a":6,"b":7}'.length });
  }, 60_000);

  describe('a function that fails', () => {
    it('answers with something that is not JSON: the node and the tab fail with the same code and the same words', async () => {
      for (const [index, answer] of ['hello', '{"unfinished": ', ''].entries()) {
        const { real, tab } = await bothRun(`demo.not-json.${index}`, fn('echo').code.replace('return request;', `return '${answer}';`), '{}');
        const [onNode, inTab] = [finished(real, 'failed'), finished(tab, 'failed')];
        expect(onNode.failure!.code, JSON.stringify(answer)).toBe('execution_failed');
        expect(inTab.failure, JSON.stringify(answer)).toEqual(onNode.failure);
      }
    }, 60_000);

    it('traps: both fail with the same code, and the reason is each engine\'s own words', async () => {
      const { real, tab } = await bothRun('demo.traps', fn('echo').code.replace('return request;', 'assert(request.length > 1000);\n  return request;'), '{}');
      const [onNode, inTab] = [finished(real, 'failed'), finished(tab, 'failed')];
      expect(onNode.failure!.code).toBe('execution_failed');
      expect(inTab.failure!.code).toBe('execution_failed');
      // A node reports where in the module the trap was, and a browser says what kind of trap it was.
      expect(onNode.failure!.message).toMatch(/^error while executing at wasm backtrace/);
      expect(inTab.failure!.message).toMatch(/unreachable/i);
    }, 60_000);

    it('never ends: both stop it and sign the failure, the node by counting fuel and the tab by its time limit', async () => {
      const { real, tab } = await bothRun('demo.never-ends.real', fn('never-ends').code, '{}');
      const [onNode, inTab] = [finished(real, 'failed'), finished(tab, 'failed')];
      // A node counts fuel as well as time, and a loop that does nothing else runs out of fuel long before the time is up. A
      // browser cannot count fuel, so the tab stops the same loop when its time limit passes.
      expect(onNode.failure).toEqual({ code: 'execution_limit_exceeded', message: 'Wasm module execution limit exceeded' });
      expect(inTab.failure!.code).toBe('execution_timed_out');
      expect(inTab.failure!.message).toBe('Wasm module wall-clock timeout exceeded after 1s');
    }, 60_000);
  });
});
