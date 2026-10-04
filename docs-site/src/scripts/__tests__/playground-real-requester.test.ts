import { createHash } from 'node:crypto';
import http from 'node:http';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createProvider, type Provider } from '../playground/provider';
import { PLAYGROUND_LIMITS } from '../playground/protocol';
import { kernel, nodeRunner, samples, verifier } from './playground-helpers';
import { freePort, startNode, wait, type RunningNode } from './real-node';

// Opt-in: the unmodified Rust requester against the shipped provider.
//
// A real froglet-node, run as a runtime, is the requester: its runtime API is asked to call a published function, and it
// then does what any node's requester does (reads the descriptor and the service, asks for a quote, verifies it, signs a
// deal, polls, and checks the receipt against the result). The provider it calls is `createProvider`, the code that runs
// in the tab, served here over local HTTP. A node's requester refuses loopback providers except its own, so the provider
// takes the node's own identity (a self-deal); a distinct-identity requester is covered by playground-exchange.test.ts.
//
// Run it after building the node (see real-node.ts):
//   FROGLET_PLAYGROUND_REAL_REQUESTER=1 npx vitest --run src/scripts/__tests__/playground-real-requester.test.ts
// Without the variable the tests are skipped, as they need the binary.

const enabled = process.env.FROGLET_PLAYGROUND_REAL_REQUESTER === '1';

// RFC 8785 for the JSON this test sends (integers, strings, arrays, objects), written apart from the code under test.
const jcs = (value: unknown): string =>
  value === null || typeof value !== 'object'
    ? JSON.stringify(value)
    : Array.isArray(value)
      ? `[${value.map(jcs).join(',')}]`
      : `{${Object.keys(value as object)
          .sort()
          .map((key) => `${JSON.stringify(key)}:${jcs((value as Record<string, unknown>)[key])}`)
          .join(',')}}`;
const sha256 = (text: string) => createHash('sha256').update(text).digest('hex');

describe.skipIf(!enabled)('a real froglet-node requester calling the shipped provider', () => {
  let node: RunningNode | undefined;
  let server: http.Server | undefined;
  let provider: Provider;
  let runtimeUrl: string;
  let providerUrl: string;
  let token: string;
  let providerId: string;
  const seen: string[] = [];

  beforeAll(async () => {
    const [runtimePort, providerPort] = [await freePort(), await freePort()];
    runtimeUrl = `http://127.0.0.1:${runtimePort}`;
    providerUrl = `http://127.0.0.1:${providerPort}`;
    node = await startNode(
      'interop',
      { FROGLET_NODE_ROLE: 'runtime', FROGLET_RUNTIME_LISTEN_ADDR: `127.0.0.1:${runtimePort}`, FROGLET_RUNTIME_PROVIDER_BASE_URL: providerUrl },
      runtimeUrl,
    );
    token = node.token('auth');
    const seed = node.seed;
    providerId = kernel.publicKey(seed);
    // Runtime-only startup does not print the provider identity. Check the
    // running node's identity endpoint before using its seed for the self-deal.
    const identity = await fetch(`${runtimeUrl}/v1/node/identity`, {
      headers: { authorization: `Bearer ${token}` },
    });
    expect(identity.status).toBe(200);
    expect(await identity.json()).toMatchObject({ node_id: providerId });

    provider = createProvider({ kernel, verifier, runModule: nodeRunner(), identity: { seed_hex: seed, public_key: providerId }, publicUrl: providerUrl, limits: { ...PLAYGROUND_LIMITS, max_runtime_ms: 1500 } });
    for (const id of ['adder', 'fibonacci', 'never-ends']) await provider.publish({ serviceId: samples[id].serviceId, summary: samples[id].summary, module: await samples[id].bytes() });
    const listening = http.createServer(async (request, response) => {
      let body = '';
      for await (const chunk of request) body += chunk;
      const answered = await provider.handle({ method: request.method as 'GET' | 'POST', path: request.url!, ...(body ? { body: JSON.parse(body) } : {}) });
      seen.push(`${request.method} ${request.url!.replace(/[0-9a-f]{32,}/g, '<id>')} ${answered.status}`);
      response.writeHead(answered.status, { 'content-type': 'application/json' });
      response.end(JSON.stringify(answered.body));
    });
    server = listening;
    await new Promise<void>((done) => listening.listen(providerPort, '127.0.0.1', done));
  }, 60_000);

  afterAll(async () => {
    // The node goes first: it may hold connections to the provider open.
    await node?.stop();
    await new Promise<void>((done) => (server ? server.close(() => done()) : done()));
  });

  /** What the node's requester reports after calling `serviceId` with `input`. */
  async function call(serviceId: string, input: unknown) {
    const runtime = async (method: string, path: string, body?: unknown) => {
      const response = await fetch(`${runtimeUrl}${path}`, { method, headers: { authorization: `Bearer ${token}`, ...(body ? { 'content-type': 'application/json' } : {}) }, body: body ? JSON.stringify(body) : undefined });
      return { status: response.status, json: (await response.json().catch(() => null)) as any };
    };
    const { service } = (await (await fetch(`${providerUrl}/v1/provider/services/${serviceId}`)).json()) as any;
    const execution = {
      schema_version: 'froglet/v1',
      workload_kind: 'compute.execution.v1',
      runtime: service.runtime,
      package_kind: service.package_kind,
      entrypoint: { kind: service.entrypoint_kind, value: service.entrypoint },
      contract_version: service.contract_version,
      input_format: 'application/json+jcs',
      input_hash: sha256(jcs(input)),
      security: { mode: 'standard', service_id: serviceId },
      input,
      module_hash: service.binding_hash,
    };
    seen.length = 0;
    const created = await runtime('POST', '/v1/runtime/deals', { provider: { provider_id: providerId, provider_url: providerUrl }, offer_id: service.offer_id, kind: 'execution', execution, max_price_sats: 0 });
    if (created.status >= 300) return { created: created.status, error: created.json };
    let deal = created.json.deal;
    for (let attempt = 0; attempt < 200 && !['succeeded', 'failed', 'rejected'].includes(deal.status); attempt++) {
      await wait(100);
      const polled = await runtime('GET', `/v1/runtime/deals/${deal.deal_id}`);
      if (polled.status >= 300) return { created: created.status, error: polled.json };
      deal = polled.json.deal ?? polled.json;
    }
    return { created: created.status, deal };
  }

  it('completes a deal for the adder, and accepts the receipt', async () => {
    const outcome = await call('demo.adder', { a: 6, b: 7 });
    expect(outcome.error).toBeUndefined();
    expect(outcome.deal).toMatchObject({ status: 'succeeded', result: { sum: 13, product: 42 } });
    expect(outcome.deal.receipt.payload).toMatchObject({ deal_state: 'succeeded', provider_id: providerId });
    // The requests a node's requester makes, which are the ones the provider serves.
    expect(seen).toEqual(expect.arrayContaining(['GET /v1/provider/descriptor 200', 'POST /v1/provider/quotes 201', 'POST /v1/provider/deals 202']));
    expect(seen.some((line) => /^GET \/v1\/provider\/deals\/<id> 200$/.test(line))).toBe(true);
  }, 40_000);

  it('completes a deal for the other sample', async () => {
    const outcome = await call('demo.fibonacci', { n: 50 });
    expect(outcome.deal).toMatchObject({ status: 'succeeded', result: { n: 50, fibonacci: 12586269025 } });
  }, 40_000);

  it('records a signed failure receipt for a function that never ends, with the node\'s own failure code', async () => {
    const outcome = await call('demo.never-ends', {});
    expect(outcome.deal).toMatchObject({ status: 'failed' });
    expect(outcome.deal.receipt.payload).toMatchObject({ deal_state: 'failed', failure_code: 'execution_timed_out' });
  }, 40_000);

  it('rejects a receipt over a hash that is not the result it was handed', async () => {
    provider.setCheat('wrong-result');
    try {
      const outcome = await call('demo.adder', { a: 1, b: 2 });
      expect(outcome.deal?.status).not.toBe('succeeded');
      expect(JSON.stringify(outcome.error)).toContain('result_hash');
    } finally {
      provider.setCheat('none');
    }
  }, 40_000);

  it('refuses a quote request that the node\'s requester would never make, as it does for any requester', async () => {
    const response = await fetch(`${providerUrl}/v1/provider/quotes`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ offer_id: 'demo.adder', requester_id: 'nope', kind: 'execution' }) });
    expect(response.status).toBe(400);
  });
});
