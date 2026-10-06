// @vitest-environment node
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { prepareLiveRun, resumeLiveRun, exportLiveEvidence, type LiveDeps } from '../live-service-client';
import * as verifierBindings from '../../generated/verifier/froglet_verify.js';
import { kernel, samples, verifier, newCompiler } from './playground-helpers';
import { FUNCTIONS } from '../playground/functions';
import { freePort, startNode, type RunningNode } from './real-node';

const enabled = process.env.FROGLET_PUBLIC_DEMO_REAL_NODE === '1';

describe.skipIf(!enabled)('self-service browser requester against a real finite-trial Froglet node', () => {
  let node: RunningNode | undefined;
  let origin: string;
  let deps: LiveDeps;
  const seen: string[] = [];

  beforeAll(async () => {
    const [providerPort, runtimePort] = [await freePort(), await freePort()];
    origin = `http://127.0.0.1:${providerPort}`;
    node = await startNode('public-trial', {
      FROGLET_NODE_ROLE: 'dual', FROGLET_NETWORK_MODE: 'clearnet',
      FROGLET_LISTEN_ADDR: `127.0.0.1:${providerPort}`, FROGLET_RUNTIME_LISTEN_ADDR: `127.0.0.1:${runtimePort}`,
      FROGLET_PROVIDER_ACCESS_MODE: 'trial', FROGLET_PROVIDER_REQUIRE_PAYMENT: 'false',
      FROGLET_PROVIDER_MAX_TOTAL_DEALS: '12', FROGLET_PROVIDER_MAX_TOTAL_QUOTES: '30', FROGLET_PROVIDER_MAX_TOTAL_RUNTIME_MS: '24000',
      FROGLET_EXECUTION_TIMEOUT_SECS: '2',
    }, origin);
    const descriptor = await (await fetch(origin + '/v1/provider/descriptor')).json() as any;
    deps = { kernel, verifier, providerId: descriptor.payload.provider_id, transport: async ({ method, path, body }) => {
      seen.push(method + ' ' + path);
      const response = await fetch(origin + path, { method, headers: body === undefined ? {} : { 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) });
      return { status: response.status, body: await response.json() };
    } };
  }, 60000);

  afterAll(async () => { await node?.stop(); });

  it('runs caller-supplied bytes without any invitation and verifies the actual signed result', async () => {
    const run = await prepareLiveRun(deps, '{"a":8,"b":9}', await samples.adder.bytes());
    const result = await resumeLiveRun(deps, run);
    expect(result.terminal, JSON.stringify(result)).toBe(true);
    if (!result.terminal) return;
    expect(result.status).toBe('succeeded');
    expect(result.result).toEqual({ sum: 17, product: 72 });
    expect(result.chain.valid).toBe(true);
    expect(result.artifacts.receipt.payload.executor.module_hash).toBe(kernel.hashBytes(await samples.adder.bytes()));
    const exported = exportLiveEvidence(result);
    const standalone = JSON.parse(verifierBindings.validate_chain_json(JSON.stringify(exported)));
    expect(standalone.valid).toBe(true);
    expect(standalone.chain_evaluated).toBe(true);
  }, 30000);

  it('recovery reads the same accepted job and never posts another deal', async () => {
    const run = await prepareLiveRun(deps, '{"a":3,"b":4}', await samples.adder.bytes());
    const result = await resumeLiveRun(deps, run);
    expect(result.terminal).toBe(true);
    seen.length = 0;
    const recovered = await resumeLiveRun(deps, JSON.parse(JSON.stringify(run)));
    expect(recovered.terminal).toBe(true);
    if (recovered.terminal) expect(recovered.result).toEqual({ sum: 7, product: 12 });
    expect(seen.every(line => line.startsWith('GET '))).toBe(true);
  }, 30000);

  it('the real provider stops an infinite loop and signs the bounded failure', async () => {
    const run = await prepareLiveRun(deps, '{}', await samples['never-ends'].bytes());
    const result = await resumeLiveRun(deps, run);
    expect(result.terminal, JSON.stringify(result)).toBe(true);
    if (!result.terminal) return;
    expect(result.status).toBe('failed');
    expect(result.failure).toBe('execution_limit_exceeded');
    expect(result.chain.valid).toBe(true);
  }, 30000);

  it('a locally changed saved input is rejected before any network request', async () => {
    const run = await prepareLiveRun(deps, '{"a":2,"b":5}', await samples.adder.bytes());
    run.input = { a: 999, b: 5 };
    seen.length = 0;
    await expect(resumeLiveRun(deps, run)).rejects.toThrow('saved input');
    expect(seen).toEqual([]);
  }, 30000);

  it('runs edited browser-compiled code rather than substituting the original calculator', async () => {
    const compiler = newCompiler();
    try {
      const source = FUNCTIONS.find(f => f.id === 'adder')!.code.replace('const sum = a + b;', 'const sum = a * a + b;');
      const compiled = await compiler.compile(source);
      expect(compiled.ok).toBe(true);
      if (!compiled.ok) return;
      const run = await prepareLiveRun(deps, '{"a":6,"b":7}', compiled.module);
      const result = await resumeLiveRun(deps, run);
      expect(result.terminal).toBe(true);
      if (result.terminal) {
        expect(result.result).toEqual({ sum: 43, product: 42 });
        expect(result.artifacts.receipt.payload.executor.module_hash).toBe(kernel.hashBytes(compiled.module));
      }
    } finally { compiler.dispose(); }
  }, 30000);

  it('a lost acceptance response is reconciled with the exact saved body without extra admissions', async () => {
    const usage = async () => {
      const response = await fetch(origin + '/v1/provider/usage', { headers: { authorization: 'Bearer ' + node!.token('froglet-control') } });
      return (await response.json() as any).usage;
    };
    const run = await prepareLiveRun(deps, '{"a":11,"b":12}', await samples.adder.bytes());
    const before = await usage();
    let lost = false;
    const fault: LiveDeps = { ...deps, transport: async request => {
      const response = await deps.transport(request);
      if (!lost && request.method === 'POST' && request.path === '/v1/provider/deals') { lost = true; throw new Error('simulated lost response after real admission'); }
      return response;
    } };
    expect((await resumeLiveRun(fault, run)).terminal).toBe(false);
    expect(run.dealId).toBeUndefined();
    const admitted = await usage();
    expect(admitted.reserved_deals).toBe(before.reserved_deals + 1);
    const recovered = await resumeLiveRun(deps, JSON.parse(JSON.stringify(run)));
    expect(recovered.terminal).toBe(true);
    if (recovered.terminal) expect(recovered.result).toEqual({ sum: 23, product: 132 });
    expect(await usage()).toEqual(admitted);
  }, 30000);

  it('oversized programs, oversized input and unsafe integers are rejected before discovery', async () => {
    seen.length = 0;
    await expect(prepareLiveRun(deps, '{}', new Uint8Array(262145))).rejects.toThrow('bounded Wasm');
    await expect(prepareLiveRun(deps, JSON.stringify('x'.repeat(131072)), await samples.adder.bytes())).rejects.toThrow('input is too large');
    await expect(prepareLiveRun(deps, '{"a":9007199254740992,"b":1}', await samples.adder.bytes())).rejects.toThrow('represent exactly');
    expect(seen).toEqual([]);
  }, 30000);

  it('the actual node refuses a forged signed deal without consuming a deal allowance', async () => {
    const run = await prepareLiveRun(deps, '{"a":1,"b":2}', await samples.adder.bytes());
    const usage = async () => (await (await fetch(origin + '/v1/provider/usage', {
      headers: { authorization: 'Bearer ' + node!.token('froglet-control') },
    })).json() as any).usage;
    const before = await usage();
    const body = structuredClone(run.request);
    body.deal.signature = '0'.repeat(128);
    const refused = await deps.transport({ method: 'POST', path: '/v1/provider/deals', body });
    expect(refused.status).toBe(400);
    expect(await usage()).toEqual(before);
  }, 30000);

  it('changed results and changed receipts are refused on recovery of a real accepted job', async () => {
    const run = await prepareLiveRun(deps, '{"a":4,"b":5}', await samples.adder.bytes());
    expect((await resumeLiveRun(deps, run)).terminal).toBe(true);
    for (const change of ['result', 'receipt']) {
      const altered: LiveDeps = { ...deps, transport: async request => {
        const response = await deps.transport(request);
        const body = structuredClone(response.body) as any;
        if (change === 'result') body.result.sum = 999;
        else body.receipt.signature = '0'.repeat(128);
        return { ...response, body };
      } };
      const recovered = await resumeLiveRun(altered, structuredClone(run));
      expect(recovered.terminal).toBe(false);
      if (!recovered.terminal) expect(recovered.reason).toContain(change === 'result' ? 'does not match the signed receipt' : 'receipt does not verify');
    }
    const real = await resumeLiveRun(deps, run);
    expect(real.terminal).toBe(true);
    if (real.terminal) expect(real.result).toEqual({ sum: 9, product: 20 });
  }, 30000);

  it('real memory growth is denied and excessive output stops within the signed execution limits', async () => {
    const compiler = newCompiler();
    try {
      for (const [source, expectedStatus, expectedFailure] of [
        ['function respond(request: string): string { const before = memory.size(); const grown = memory.grow(1); return `{"before":${before},"grown":${grown},"after":${memory.size()}}`; }', 'succeeded', undefined],
        // The native StoreLimits explicitly traps on a denied memory.grow.
        ['function respond(request: string): string { memory.grow(129); return "{}"; }', 'failed', 'execution_failed'],
        // AssemblyScript traps after the denied allocation. The receipt records
        // that trap as execution_failed, not a specifically classified fuel failure.
        ['function respond(request: string): string { const data = new Uint8Array(9 * 1024 * 1024); data[0] = 1; return `${data[0]}`; }', 'failed', 'execution_failed'],
        ['function respond(request: string): string { return "\\\"" + "x".repeat(131073) + "\\\""; }', 'failed', 'execution_limit_exceeded'],
      ]) {
        const compiled = await compiler.compile(source!);
        expect(compiled.ok).toBe(true);
        if (!compiled.ok) throw new Error(JSON.stringify(compiled));
        const run = await prepareLiveRun(deps, '{}', compiled.module);
        const outcome = await resumeLiveRun(deps, run);
        expect(outcome.terminal, JSON.stringify(outcome)).toBe(true);
        if (!outcome.terminal) return;
        expect(outcome.status).toBe(expectedStatus);
        expect(outcome.failure).toBe(expectedFailure);
        if (expectedStatus === 'succeeded') {
          const result = outcome.result as { before: number; after: number; grown: number };
          expect(result.grown).toBe(result.before);
          expect(result.after).toBe(result.before + 1);
          expect(result.after * 65536).toBeLessThanOrEqual(run.request.quote.payload.execution_limits.max_memory_bytes);
        }
        expect(outcome.chain.valid).toBe(true);
      }
    } finally { compiler.dispose(); }
  }, 30000);
});
