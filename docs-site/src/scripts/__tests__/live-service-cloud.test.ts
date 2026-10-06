// @vitest-environment node
// Explicit operator opt-in. CI does not consume the public beta's allowances.
import { writeFileSync } from 'node:fs';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { prepareLiveRun, resumeLiveRun, type LiveDeps, type LiveResult } from '../live-service-client';
import { kernel, verifier, newCompiler } from './playground-helpers';
import { FUNCTIONS } from '../playground/functions';

const enabled = process.env.FROGLET_PUBLIC_DEMO_REMOTE_NODE === '1';
const origin = 'https://froglet-public-beta-20261006.fly.dev';
const providerId = 'c7a15140cc28833978197bdef7daf30e419502f186f59ff88584f453531ea894';

describe.skipIf(!enabled)('actual released Linux provider reached by a separate public HTTPS requester', () => {
  const compiler = newCompiler();
  const evidence: Record<string, unknown> = { origin, providerId, scope: 'Operator-run requester code on Mac to one released Linux node on Fly; no fresh LLM or human setup claim', cases: [] };
  const cases = evidence.cases as unknown[];
  const deps: LiveDeps = { kernel, verifier, providerId, transport: async ({ method, path, body }) => {
    const response = await fetch(origin + path, { method, headers: body === undefined ? {} : { 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(15000) });
    return { status: response.status, body: await response.json() };
  } };
  const status = async () => await (await fetch(origin + '/demo/status', { signal: AbortSignal.timeout(10000) })).json();

  beforeAll(async () => { evidence.before = await status(); }, 15000);
  afterAll(async () => {
    compiler.dispose();
    evidence.after = await status();
    const path = process.env.FROGLET_PUBLIC_DEMO_EVIDENCE_FILE;
    if (path) writeFileSync(path, JSON.stringify(evidence, null, 2) + '\n', { mode: 0o600 });
  }, 15000);

  async function compiled(id: string, edited = false) {
    let source = FUNCTIONS.find(f => f.id === id)!.code;
    if (edited) source = source.replace('const sum = a + b;', 'const sum = a * a + b;');
    const result = await compiler.compile(source);
    if (!result.ok) throw new Error('The actual browser compiler failed.');
    return result.module;
  }

  function done(result: LiveResult) {
    expect(result.terminal, JSON.stringify(result)).toBe(true);
    if (!result.terminal) throw new Error(result.reason);
    expect(result.chain.valid).toBe(true);
    return result;
  }

  it('runs the browser-compiled calculator and verifies its actual result commitment', async () => {
    const module = await compiled('adder');
    const run = await prepareLiveRun(deps, '{"a":6,"b":7}', module);
    const outcome = done(await resumeLiveRun(deps, run));
    expect(outcome.result).toEqual({ sum: 13, product: 42 });
    expect(outcome.artifacts.receipt.payload.executor.module_hash).toBe(kernel.hashBytes(module));
    cases.push({ name: 'adder13/42', outcome });
    const before = await status();
    const recovered = done(await resumeLiveRun(deps, JSON.parse(JSON.stringify(run))));
    expect(recovered.result).toEqual(outcome.result);
    const after = await status();
    expect(after.usage).toEqual(before.usage);
    cases.push({ name: 'exactacceptedrecovery', before, after, outcome: recovered });
  }, 60000);

  it('runs changed code, not a fixed replacement endpoint', async () => {
    const module = await compiled('adder', true);
    const run = await prepareLiveRun(deps, '{"a":6,"b":7}', module);
    const outcome = done(await resumeLiveRun(deps, run));
    expect(outcome.result).toEqual({ sum: 43, product: 42 });
    expect(outcome.artifacts.receipt.payload.executor.module_hash).toBe(kernel.hashBytes(module));
    cases.push({ name: 'edited43/42', outcome });
  }, 60000);

  it('returns only the selected synthetic fields for the ambiguous blood label', async () => {
    const run = await prepareLiveRun(deps, JSON.stringify({ op: 'select', collection: 'terminology', columns: ['source', 'target'], equals: { source: 'DEMO:sample.blood' }, limit: 100 }));
    const outcome = done(await resumeLiveRun(deps, run));
    expect(outcome.status).toBe('succeeded');
    const result = outcome.result as any;
    expect(result.rows).toHaveLength(2);
    expect(result.rows.every((row: any) => Object.keys(row).sort().join(',') === 'source,target')).toBe(true);
    expect(result.rows.map((row: any) => row.target).sort()).toEqual(['DEMO:specimen.plasma', 'DEMO:specimen.whole_blood']);
    cases.push({ name: 'selectedbloodrows', outcome });
  }, 60000);

  it('stops the browser-compiled loop with a signed execution-budget failure', async () => {
    const run = await prepareLiveRun(deps, '{}', await compiled('never-ends'));
    const outcome = done(await resumeLiveRun(deps, run));
    expect(outcome.status).toBe('failed');
    expect(outcome.failure).toBe('execution_limit_exceeded');
    cases.push({ name: 'signedbudgetfailure', outcome });
  }, 60000);

  it('does not expose operator, runtime or general execution endpoints', async () => {
    for (const path of ['/v1/provider/usage', '/v1/runtime/deals', '/v1/provider/artifacts/publish', '/v1/node/execute/wasm']) {
      const response = await fetch(origin + path, { signal: AbortSignal.timeout(10000) });
      expect(response.status).toBe(404);
    }
    const refused = await fetch(origin + '/v1/provider/quotes', { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{"kind":"execution","execution":{"runtime":"python"}}', signal: AbortSignal.timeout(10000) });
    expect(refused.status).toBe(400);
    cases.push({ name: 'operatorandotherruntimesrefused', passed: true });
  }, 60000);
});
