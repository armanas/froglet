import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { runExchange, type ExchangeDeps, type ExchangeOutcome } from '../playground/consumer';
import { createProvider, failureCodeFor, type ProviderEvent } from '../playground/provider';
import { dealPayloadFor, executionFor, isFinished, PLAYGROUND_LIMITS, QUOTE_TTL_SECS, Refusal } from '../playground/protocol';
import type { HttpRequest, HttpResponse, ModuleRunner, SignedArtifact, Transport } from '../playground/types';
import { createWire } from '../playground/wire';
import { capturedNodeModule, counterHex, fakeClock, hexBytes, kernel, NEVER_ENDS_MODULE_HEX, nodeExchange, nodeRunner, samples, shapeOf, verifier, withSection } from './playground-helpers';
import { repoRoot } from './route-helpers';

// Two parties in one page, run for real: Bob is createProvider and Alice is runExchange, joined by the wire. The kernel and
// the verifier are the Rust code compiled to WebAssembly, the functions are compiled Rust, and each run happens in a
// Worker, so what these tests see is what the page does. Only the clock and the ids are fixed, so runs can be compared.

/** Long enough that a slow machine never times out an honest function; the runaway tests use a short one. */
const HONEST_MS = 10_000;
const RUNAWAY_MS = 300;

interface Options {
  publish?: string[];
  runtimeMs?: number;
  runModule?: ModuleRunner;
}

async function setup({ publish = ['adder'], runtimeMs = HONEST_MS, runModule = nodeRunner() }: Options = {}) {
  const clock = fakeClock();
  const events: ProviderEvent[] = [];
  const limits = { ...PLAYGROUND_LIMITS, max_runtime_ms: runtimeMs };
  const provider = createProvider({ kernel, verifier, runModule, now: clock.now, randomHex: counterHex(), limits, onEvent: (event) => events.push(event) });
  for (const id of publish) {
    const sample = samples[id];
    await provider.publish({ serviceId: sample.serviceId, summary: sample.summary, module: await sample.bytes() });
  }
  const wire = createWire((request) => provider.handle(request));
  const requester = kernel.newIdentity();
  const exchange = (overrides: Partial<ExchangeDeps> = {}) =>
    runExchange({ kernel, verifier, transport: wire.transport, requester, serviceId: 'demo.adder', inputText: '{"a": 6, "b": 7}', now: clock.now, pollIntervalMs: 2, waitForNextSecond: async () => clock.advance(1), ...overrides });
  return { clock, events, provider, wire, requester, exchange, limits };
}
type Context = Awaited<ReturnType<typeof setup>>;

const get = async (context: Context, path: string) => context.provider.handle({ method: 'GET', path });

/** Alice's first two steps, by hand, so a test can send Bob something she never would. */
async function quoted(context: Context, input: unknown = { a: 6, b: 7 }, serviceId = 'demo.adder') {
  const service = ((await get(context, `/v1/provider/services/${serviceId}`)).body as any).service;
  const execution = executionFor(service, input, kernel);
  const request = { offer_id: service.offer_id, requester_id: context.requester.public_key, kind: 'execution', execution, max_price_sats: 0 };
  const response = await context.provider.handle({ method: 'POST', path: '/v1/provider/quotes', body: request });
  return { service, execution, request, response, quote: response.body as SignedArtifact };
}

const signedDeal = (context: Context, quote: SignedArtifact, extra: Record<string, unknown> = {}, who = context.requester) =>
  kernel.sign(who.seed_hex, 'deal', context.clock.now(), { ...dealPayloadFor(quote, who.public_key, 'ab'.repeat(32)), ...extra });

const postDeal = (context: Context, body: unknown) => context.provider.handle({ method: 'POST', path: '/v1/provider/deals', body });

async function finished(context: Context, dealId: string) {
  for (let attempt = 0; attempt < 400; attempt++) {
    const record = (await get(context, `/v1/provider/deals/${dealId}`)).body as any;
    if (record.status !== 'accepted') return record;
    await new Promise((resolveWait) => setTimeout(resolveWait, 5));
  }
  throw new Error('the deal did not finish');
}

/** Wrap a transport so a test can change what one response says on its way back to Alice. */
const tampering = (transport: Transport, change: (request: HttpRequest, response: HttpResponse) => void): Transport => async (request) => {
  const response = await transport(request);
  change(request, response);
  return response;
};

function assertOk(outcome: ExchangeOutcome): asserts outcome is Extract<ExchangeOutcome, { ok: true }> {
  if (!outcome.ok) throw new Error(`the exchange was refused at ${outcome.stage} by ${outcome.refusedBy}: ${outcome.reason}`);
}

const otherIdentity = () => kernel.newIdentity();

describe('Bob publishes a function', () => {
  it('serves nothing until something is published', async () => {
    const context = await setup({ publish: [] });
    const response = await get(context, '/v1/provider/descriptor');
    expect(response.status).toBe(503);
    expect((await get(context, '/health')).status).toBe(200);
  });

  it('serves a signed catalog that verifies, and lists the function under its own name', async () => {
    const context = await setup();
    const descriptor = (await get(context, '/v1/provider/descriptor')).body as SignedArtifact;
    expect(verifier.verifyDocument(descriptor).status).toBe('verified');
    expect(descriptor.signer).toBe(context.provider.publicKey);
    expect(descriptor.payload.provider_id).toBe(context.provider.publicKey);

    const { offers } = (await get(context, '/v1/provider/offers')).body as { offers: SignedArtifact[] };
    expect(offers).toHaveLength(1);
    expect(verifier.verifyDocument(offers[0]).status).toBe('verified');
    expect(offers[0].payload.offer_id).toBe('demo.adder');
    expect(offers[0].payload.descriptor_hash).toBe(descriptor.hash);

    const { services } = (await get(context, '/v1/provider/services')).body as { services: any[] };
    expect(services.map((service) => service.service_id)).toEqual(['demo.adder']);
    const adder = await samples.adder.bytes();
    expect(services[0].module_hash).toBe(kernel.hashBytes(adder));
    expect(services[0].binding_hash).toBe(services[0].module_hash);

    const one = (await get(context, '/v1/provider/services/demo.adder')).body as any;
    expect(one).toEqual({ service: services[0], execution_access: 'open' });
    expect((await get(context, '/v1/provider/services/nope')).status).toBe(404);
  });

  it('hashes the module as it was published, not as it is run with its memory cap', async () => {
    const context = await setup();
    const adder = await samples.adder.bytes();
    const receiptHash = (await context.exchange()) as any;
    assertOk(receiptHash);
    expect(receiptHash.artifacts.receipt.payload.executor.module_hash).toBe(kernel.hashBytes(adder));
  });

  it('signs the catalog again when a second function is published, and every offer follows the new descriptor', async () => {
    const context = await setup();
    const first = (await get(context, '/v1/provider/descriptor')).body as SignedArtifact;
    const fibonacci = samples.fibonacci;
    await context.provider.publish({ serviceId: fibonacci.serviceId, summary: fibonacci.summary, module: await fibonacci.bytes() });
    const second = (await get(context, '/v1/provider/descriptor')).body as SignedArtifact;
    expect(second.hash).not.toBe(first.hash);
    expect(second.payload.descriptor_seq).toBe(first.payload.descriptor_seq + 1);
    const { offers } = (await get(context, '/v1/provider/offers')).body as { offers: SignedArtifact[] };
    expect(offers.map((offer) => offer.payload.offer_id).sort()).toEqual(['demo.adder', 'demo.fibonacci']);
    for (const offer of offers) {
      expect(offer.payload.descriptor_hash).toBe(second.hash);
      expect(verifier.verifyDocument(offer).status).toBe('verified');
    }
    // The earlier descriptor stays fetchable by hash, so evidence that points at it still resolves.
    expect(((await get(context, `/v1/artifacts/${first.hash}`)).body as SignedArtifact).hash).toBe(first.hash);
  });

  it('refuses what it cannot host, and says why', async () => {
    const context = await setup({ publish: [] });
    const good = await samples.adder.bytes();
    const attempt = async (input: { serviceId: string; module: Uint8Array }) => {
      try {
        await context.provider.publish({ summary: 'x', ...input });
      } catch (error) {
        expect(error).toBeInstanceOf(Refusal);
        return (error as Refusal).message;
      }
      throw new Error('expected a refusal');
    };
    expect(await attempt({ serviceId: 'Demo Adder!', module: good })).toContain('lowercase letters');
    expect(await attempt({ serviceId: '', module: good })).toContain('lowercase letters');
    expect(await attempt({ serviceId: 'ok', module: new TextEncoder().encode('not a module at all') })).toContain('not a valid WebAssembly module');
    expect(await attempt({ serviceId: 'ok', module: withSection(good, 7, [1, 3, 0x72, 0x75, 0x6e, 0, 0]) })).toBeTruthy();
    expect(await attempt({ serviceId: 'ok', module: new Uint8Array(300_000) })).toContain('larger than');
    expect((await get(context, '/v1/provider/descriptor')).status).toBe(503);
  });
});

describe('a whole exchange between Alice and Bob', () => {
  it('calls a compiled Rust function by name and gets its answer, with evidence that verifies', async () => {
    const context = await setup();
    const outcome = await context.exchange();
    assertOk(outcome);
    expect(outcome.status).toBe('succeeded');
    expect(outcome.result).toEqual({ sum: 13, product: 42 });
    expect(Object.keys(outcome.artifacts)).toEqual(['descriptor', 'offer', 'quote', 'deal', 'receipt']);
    expect(outcome.chain.valid).toBe(true);
    expect(outcome.chain.artifacts.map((artifact) => artifact.status)).toEqual(['verified', 'verified', 'verified', 'verified', 'verified']);
  });

  it('runs the other sample, and both can be published at once', async () => {
    const context = await setup({ publish: ['adder', 'fibonacci'] });
    const fib = await context.exchange({ serviceId: 'demo.fibonacci', inputText: '{"n": 78}' });
    assertOk(fib);
    expect(fib.result).toEqual({ n: 78, fibonacci: 8944394323791464 });
    const sum = await context.exchange({ inputText: '{"a": -4, "b": 9}' });
    assertOk(sum);
    expect(sum.result).toEqual({ sum: 5, product: -36 });
  });

  it('has the receipt commit to the result by the canonical hash, and to the module and the deal it answers', async () => {
    const context = await setup();
    const outcome = await context.exchange();
    assertOk(outcome);
    const { receipt, deal, quote, descriptor } = outcome.artifacts;
    expect(receipt.payload.result_hash).toBe(kernel.hashJson(outcome.result));
    expect(receipt.payload.deal_hash).toBe(deal.hash);
    expect(receipt.payload.quote_hash).toBe(quote.hash);
    expect(receipt.payload.provider_id).toBe(descriptor.payload.provider_id);
    expect(receipt.payload.requester_id).toBe(context.requester.public_key);
    expect(receipt.payload.deal_state).toBe('succeeded');
    expect(receipt.payload.limits_applied).toEqual(context.limits);
    expect(deal.signer).toBe(context.requester.public_key);
    expect(receipt.signer).toBe(context.provider.publicKey);
  });

  it('asks for the workload it built, so its hash is the one in the quote and the deal', async () => {
    const context = await setup();
    const outcome = await context.exchange();
    assertOk(outcome);
    const service = ((await get(context, '/v1/provider/services/demo.adder')).body as any).service;
    const expected = kernel.hashJson(executionFor(service, { a: 6, b: 7 }, kernel));
    expect(outcome.artifacts.quote.payload.workload_hash).toBe(expected);
    expect(outcome.artifacts.deal.payload.workload_hash).toBe(expected);
  });

  it('crosses the wire in the order a node\'s API is used, as JSON text', async () => {
    const context = await setup();
    assertOk(await context.exchange());
    const steps = context.wire.entries.map((entry) => `${entry.method} ${entry.path.replace(/[0-9a-f]{32}$/, '<deal>')} ${entry.status}`);
    expect(steps.slice(0, 5)).toEqual([
      'GET /v1/provider/descriptor 200',
      'GET /v1/provider/offers 200',
      'GET /v1/provider/services/demo.adder 200',
      'POST /v1/provider/quotes 201',
      'POST /v1/provider/deals 202',
    ]);
    const polls = steps.slice(5);
    expect(polls.length).toBeGreaterThan(0);
    expect(polls.every((step) => /^GET \/v1\/provider\/deals\/<deal> (200)$/.test(step))).toBe(true);
    expect(context.wire.entries.map((entry) => entry.n)).toEqual(context.wire.entries.map((_, index) => index + 1));
    for (const entry of context.wire.entries) {
      expect(JSON.parse(JSON.stringify(entry.response))).toEqual(entry.response);
      expect(entry.ms).toBeGreaterThanOrEqual(0);
    }
    context.wire.clear();
    expect(context.wire.entries).toHaveLength(0);
  });

  it('lets the evidence be fetched again by hash, and verifies it again from what was fetched', async () => {
    const context = await setup();
    const outcome = await context.exchange();
    assertOk(outcome);
    const fetched: SignedArtifact[] = [];
    for (const artifact of Object.values(outcome.artifacts)) {
      const response = await get(context, `/v1/artifacts/${artifact.hash}`);
      expect(response.status).toBe(200);
      expect(response.body).toEqual(artifact);
      fetched.push(response.body as SignedArtifact);
    }
    expect(verifier.validateChain(fetched).valid).toBe(true);
    expect((await get(context, `/v1/artifacts/${'0'.repeat(64)}`)).status).toBe(404);
  });

  it('is undone by a one-value change to the evidence, which the chain check then reports', async () => {
    const context = await setup();
    const outcome = await context.exchange();
    assertOk(outcome);
    const { descriptor, offer, quote, deal, receipt } = outcome.artifacts;
    const changed = structuredClone(receipt);
    changed.payload.result_hash = kernel.hashJson({ sum: 14, product: 42 });
    const report = verifier.validateChain([descriptor, offer, quote, deal, changed]);
    expect(report.valid).toBe(false);
    expect(report.artifacts.find((artifact) => artifact.artifact_type === 'receipt')!.status).toBe('invalid');
    expect(report.artifacts.filter((artifact) => artifact.status === 'verified')).toHaveLength(4);
  });

  it('hands over copies: changing what came back does not change what Bob holds', async () => {
    const context = await setup();
    const outcome = await context.exchange();
    assertOk(outcome);
    (outcome.artifacts.quote.payload as any).workload_hash = 'changed';
    const again = (await get(context, `/v1/artifacts/${outcome.artifacts.quote.hash}`)).body as SignedArtifact;
    expect(again.payload.workload_hash).not.toBe('changed');
    expect(verifier.verifyDocument(again).status).toBe('verified');
  });

  it('reports each step as it happens, in order', async () => {
    const context = await setup();
    const steps: string[] = [];
    assertOk(await context.exchange({ onStep: (step) => steps.push(step) }));
    expect(steps).toEqual(['discover', 'quote', 'deal', 'result', 'verify']);
  });
});

describe('a function that does not give an answer', () => {
  it('is stopped at the time limit, and Bob signs a receipt that says it failed', async () => {
    const context = await setup({ publish: ['adder', 'never-ends'], runtimeMs: RUNAWAY_MS });
    const started = performance.now();
    const outcome = await context.exchange({ serviceId: 'demo.never-ends', inputText: '{}' });
    const took = performance.now() - started;
    assertOk(outcome);
    expect(outcome.status).toBe('failed');
    expect(outcome.failure!.code).toBe('execution_timed_out');
    expect(outcome.failure!.message).toBe('Wasm module wall-clock timeout exceeded after 0.3s');
    expect(outcome.result).toBeUndefined();
    expect(took).toBeGreaterThanOrEqual(RUNAWAY_MS - 20);
    // Honest failure is still evidence: every artifact verifies and the chain is valid.
    expect(outcome.chain.valid).toBe(true);
    const receipt = outcome.artifacts.receipt;
    expect(receipt.payload.deal_state).toBe('failed');
    expect(receipt.payload.execution_state).toBe('failed');
    expect(receipt.payload.failure_code).toBe('execution_timed_out');
    expect(receipt.payload.result_hash).toBeUndefined();
    const record = (await get(context, `/v1/artifacts/${receipt.hash}`)).body as SignedArtifact;
    expect(record.hash).toBe(receipt.hash);
    expect(context.events.some((event) => event.type === 'executed' && !event.outcome.ok && event.outcome.timedOut)).toBe(true);
  });

  it('carries on serving after a runaway function', async () => {
    const context = await setup({ publish: ['adder', 'never-ends'], runtimeMs: RUNAWAY_MS });
    assertOk(await context.exchange({ serviceId: 'demo.never-ends', inputText: '{}' }));
    const outcome = await context.exchange({ inputText: '{"a": 1, "b": 2}', serviceId: 'demo.adder' });
    assertOk(outcome);
    expect(outcome.status).toBe('succeeded');
  });

  it('reports a function that traps as a failure with the reason, not as a timeout', async () => {
    const trap = withSection(hexBytes(NEVER_ENDS_MODULE_HEX), 10, [2, 5, 0, 0x41, 0xc0, 0x00, 0x0b, 3, 0, 0x00, 0x0b]);
    const context = await setup({ publish: [] });
    await context.provider.publish({ serviceId: 'demo.trap', summary: 'traps', module: trap });
    const outcome = await context.exchange({ serviceId: 'demo.trap', inputText: '{}' });
    assertOk(outcome);
    expect(outcome.status).toBe('failed');
    expect(outcome.failure!.code).toBe('execution_failed');
    expect(outcome.failure!.message).toMatch(/unreachable/i);
    expect(outcome.chain.valid).toBe(true);
  });

  it('classifies an output over the limit as the node does', async () => {
    const context = await setup({ publish: [] });
    const smallOutput = createProvider({ kernel, verifier, runModule: nodeRunner(), now: context.clock.now, limits: { ...PLAYGROUND_LIMITS, max_output_bytes: 8 } });
    await smallOutput.publish({ serviceId: 'demo.adder', summary: 'x', module: await samples.adder.bytes() });
    const wire = createWire((request) => smallOutput.handle(request));
    const outcome = await runExchange({ kernel, verifier, transport: wire.transport, requester: kernel.newIdentity(), serviceId: 'demo.adder', inputText: '{"a": 6, "b": 7}', now: context.clock.now, pollIntervalMs: 2 });
    assertOk(outcome);
    expect(outcome.failure!.code).toBe('execution_limit_exceeded');
  });

  it('does not sign an answer a browser could not hold exactly, and says so', async () => {
    const roundedResult: ModuleRunner = async () => ({ ok: true, output: '{"total": 9007199254740993}', memoryBytes: 0, capEnforced: true });
    const context = await setup({ runModule: roundedResult });
    const outcome = await context.exchange();
    assertOk(outcome);
    expect(outcome.status).toBe('failed');
    expect(outcome.failure!.message).toContain('9007199254740993');
    expect(outcome.chain.valid).toBe(true);
  });

  it('records an answer that is not JSON as a failure, and does not leave the deal waiting', async () => {
    for (const output of ['hello', '', '{"unfinished": ']) {
      const notJson: ModuleRunner = async () => ({ ok: true, output, memoryBytes: 0, capEnforced: true });
      const context = await setup({ runModule: notJson });
      const outcome = await context.exchange();
      assertOk(outcome);
      expect(outcome.status, JSON.stringify(output)).toBe('failed');
      expect(outcome.failure!.code).toBe('execution_failed');
      // The words are the node's parser's, which is the kernel's.
      expect(outcome.failure!.message, JSON.stringify(output)).toMatch(/ at line \d+ column \d+$/);
      expect(outcome.failure!.message).toBe(kernel.jsonError(output));
      expect(outcome.chain.valid).toBe(true);
      expect(outcome.artifacts.receipt.payload.result_hash).toBeUndefined();
    }
  });

  it('still answers when the runner itself breaks', async () => {
    const broken: ModuleRunner = async () => {
      throw new Error('the runner fell over');
    };
    const context = await setup({ runModule: broken });
    const outcome = await context.exchange();
    assertOk(outcome);
    expect(outcome).toMatchObject({ status: 'failed', failure: { code: 'execution_failed', message: 'the runner fell over' } });
  });
});

describe('Alice does not believe what she cannot check', () => {
  it('refuses a receipt over a hash that is not the result, and can tell it from an honest one', async () => {
    const context = await setup();
    context.provider.setCheat('wrong-result');
    const cheated = await context.exchange();
    expect(cheated.ok).toBe(false);
    if (cheated.ok) return;
    expect(cheated.refusedBy).toBe('alice');
    expect(cheated.stage).toBe('result');
    expect(cheated.reason).toContain('result_hash');
    // Bob's signature on that receipt is genuine; what fails is the match between the hash and the result.
    expect(verifier.verifyDocument(cheated.artifacts.receipt).status).toBe('verified');
    context.provider.setCheat('none');
    assertOk(await context.exchange());
  });

  it('refuses a result swapped on the way, though the receipt is honest', async () => {
    const context = await setup();
    const transport = tampering(context.wire.transport, (request, response) => {
      if (request.method === 'GET' && request.path.startsWith('/v1/provider/deals/')) {
        const record = response.body as any;
        if (record.status === 'succeeded') record.result = { sum: 14, product: 42 };
      }
    });
    const outcome = await context.exchange({ transport });
    expect(outcome).toMatchObject({ ok: false, refusedBy: 'alice', stage: 'result' });
  });

  it('refuses another deal\'s receipt, which Bob really signed', async () => {
    const context = await setup();
    const first = await context.exchange();
    assertOk(first);
    const oldReceipt = first.artifacts.receipt;
    const transport = tampering(context.wire.transport, (request, response) => {
      if (request.method === 'GET' && request.path.startsWith('/v1/provider/deals/')) {
        const record = response.body as any;
        if (record.receipt) record.receipt = oldReceipt;
      }
    });
    const outcome = await context.exchange({ transport, requester: context.requester });
    expect(outcome).toMatchObject({ ok: false, refusedBy: 'alice', stage: 'result', reason: 'The receipt is not for this deal.' });
  });

  it('refuses a quote for a different workload than the one she asked for', async () => {
    const context = await setup();
    const transport: Transport = async (request) => {
      if (request.method === 'POST' && request.path === '/v1/provider/quotes') {
        const other = { a: 1, b: 1 };
        const execution = { ...(request.body as any).execution, input: other, input_hash: kernel.hashJson(other) };
        return context.wire.transport({ ...request, body: { ...(request.body as any), execution } });
      }
      return context.wire.transport(request);
    };
    const outcome = await context.exchange({ transport });
    expect(outcome).toMatchObject({ ok: false, refusedBy: 'alice', stage: 'quote', reason: 'The quote is for a different workload than the one asked for.' });
  });

  it('refuses a descriptor that was changed on the way', async () => {
    const context = await setup();
    const transport = tampering(context.wire.transport, (request, response) => {
      if (request.path === '/v1/provider/descriptor') (response.body as any).payload.descriptor_seq += 1;
    });
    expect(await context.exchange({ transport })).toMatchObject({ ok: false, refusedBy: 'alice', stage: 'discover', reason: 'The descriptor does not verify.' });
  });

  it('refuses a deal record that is not the deal she signed', async () => {
    const context = await setup();
    const impostor = otherIdentity();
    const transport = tampering(context.wire.transport, (request, response) => {
      if (request.method === 'POST' && request.path === '/v1/provider/deals') {
        const record = response.body as any;
        record.deal = kernel.sign(impostor.seed_hex, 'deal', 1, record.deal.payload);
      }
    });
    expect(await context.exchange({ transport })).toMatchObject({ ok: false, refusedBy: 'alice', stage: 'deal' });
  });

  it('refuses a receipt that disagrees with the deal record about whether the function ran', async () => {
    const context = await setup();
    const transport = tampering(context.wire.transport, (request, response) => {
      if (request.method === 'GET' && request.path.startsWith('/v1/provider/deals/')) {
        const record = response.body as any;
        if (record.status === 'succeeded') record.status = 'failed';
      }
    });
    expect(await context.exchange({ transport })).toMatchObject({ ok: false, refusedBy: 'alice', stage: 'result' });
  });

  it('refuses a quote that has already expired', async () => {
    const context = await setup();
    const transport = tampering(context.wire.transport, (request) => {
      if (request.method === 'POST' && request.path === '/v1/provider/quotes') context.clock.advance(QUOTE_TTL_SECS + 1);
    });
    expect(await context.exchange({ transport })).toMatchObject({ ok: false, refusedBy: 'alice', stage: 'quote', reason: 'The quote has already expired.' });
  });

  it('gives up on a provider that never finishes, and says who did not answer', async () => {
    const hangs: ModuleRunner = () => new Promise(() => {});
    const context = await setup({ runModule: hangs });
    const outcome = await context.exchange({ maxPolls: 4, pollIntervalMs: 1 });
    expect(outcome).toMatchObject({ ok: false, refusedBy: 'bob', stage: 'result', reason: 'Bob has not finished.' });
  });

  it('waits for Bob as long as the quote\'s own time limit allows, and no longer', async () => {
    const hangs: ModuleRunner = () => new Promise(() => {});
    const context = await setup({ runModule: hangs, runtimeMs: 60 });
    const started = performance.now();
    const outcome = await context.exchange({ graceMs: 0, pollIntervalMs: 5 });
    const took = performance.now() - started;
    expect(outcome).toMatchObject({ ok: false, refusedBy: 'bob', stage: 'result', reason: 'Bob has not finished.' });
    expect(took).toBeGreaterThanOrEqual(55);
    expect(took).toBeLessThan(1500);
  });

  it('reports a deal Bob rejects with his reason', async () => {
    const context = await setup();
    const transport = tampering(context.wire.transport, (request, response) => {
      if (request.method === 'POST' && request.path === '/v1/provider/deals') Object.assign(response.body as object, { status: 'rejected', error: 'no capacity right now' });
    });
    expect(await context.exchange({ transport })).toMatchObject({ ok: false, refusedBy: 'bob', stage: 'deal', reason: 'no capacity right now' });
  });

  it('asks again in the next second when a repeat gets a quote that was already used', async () => {
    const context = await setup();
    assertOk(await context.exchange());
    context.wire.clear();
    // The clock has not moved and the request is the same, so Bob signs the same quote, and it backs one deal only.
    const repeated = await context.exchange();
    assertOk(repeated);
    expect(repeated.result).toEqual({ sum: 13, product: 42 });
    const steps = context.wire.entries.map((entry) => `${entry.method} ${entry.path.replace(/[0-9a-f]{32}$/, '<deal>')} ${entry.status}`);
    expect(steps.filter((step) => step.startsWith('POST /v1/provider/quotes'))).toEqual(['POST /v1/provider/quotes 201', 'POST /v1/provider/quotes 201']);
    expect(steps.filter((step) => step.startsWith('POST /v1/provider/deals'))).toEqual(['POST /v1/provider/deals 409', 'POST /v1/provider/deals 202']);
  });

  it('does not keep asking when the quote is refused every time', async () => {
    const context = await setup();
    const transport: Transport = async (request) =>
      request.method === 'POST' && request.path === '/v1/provider/deals' ? { status: 409, body: { error: 'quote already used by a different deal' } } : context.wire.transport(request);
    const outcome = await context.exchange({ transport });
    expect(outcome).toMatchObject({ ok: false, refusedBy: 'bob', stage: 'deal', reason: 'quote already used by a different deal' });
    expect(context.wire.entries.filter((entry) => entry.path === '/v1/provider/quotes')).toHaveLength(3);
  });

  it('refuses input a browser would round, before anything is sent', async () => {
    const context = await setup();
    const outcome = await context.exchange({ inputText: '{"a": 9007199254740993, "b": 1}' });
    expect(outcome).toMatchObject({ ok: false, stage: 'input', refusedBy: 'alice' });
    expect(context.wire.entries).toHaveLength(0);
    expect((await context.exchange({ inputText: '{"a": ' })).ok).toBe(false);
    expect(context.wire.entries).toHaveLength(0);
    // 1e999 is not an integer literal, but a browser reads it as Infinity and would write it back as null.
    expect(await context.exchange({ inputText: '{"a": 1e999, "b": 1}' })).toMatchObject({ ok: false, stage: 'input', reason: 'A number in the input is too large for a browser to hold.' });
    expect(await context.exchange({ inputText: '[1, [2, {"x": -1e999}]]' })).toMatchObject({ ok: false, stage: 'input' });
    expect(context.wire.entries).toHaveLength(0);
  });

  it('carries on after a refusal: the same Alice can call again', async () => {
    const context = await setup();
    await context.exchange({ inputText: '{"a": 9007199254740993, "b": 1}' });
    assertOk(await context.exchange());
  });
});

describe('Bob does not accept what he cannot check', () => {
  it('refuses a deal changed after it was signed, and runs nothing', async () => {
    const context = await setup();
    const outcome = await context.exchange({ alterDealAfterSigning: true });
    expect(outcome).toMatchObject({ ok: false, refusedBy: 'bob', stage: 'deal', reason: 'the deal does not verify: its signature does not match what was signed' });
    expect(context.events.filter((event) => event.type === 'executed')).toHaveLength(0);
  });

  it('refuses a quote request for a function that is not published, or with the wrong module hash, input hash or shape', async () => {
    const context = await setup();
    const { request } = await quoted(context);
    const post = (body: unknown) => context.provider.handle({ method: 'POST', path: '/v1/provider/quotes', body });
    const withExecution = (change: Record<string, unknown>) => ({ ...request, execution: { ...request.execution, ...change } });

    expect((await post({ ...request, execution: { ...request.execution, security: { mode: 'standard', service_id: 'nope' } } })).status).toBe(404);
    expect(await post(withExecution({ module_hash: '0'.repeat(64) }))).toMatchObject({ status: 409 });
    expect(await post(withExecution({ input: { a: 1, b: 1 } }))).toMatchObject({ status: 400, body: { error: expect.stringContaining('input_hash') } });
    expect(await post(withExecution({ entrypoint: { kind: 'handler', value: 'other' } }))).toMatchObject({ status: 400 });
    expect(await post(withExecution({ requested_access: ['network'] }))).toMatchObject({ status: 400, body: { error: expect.stringContaining('not granted') } });
    expect(await post(withExecution({ mounts: [{ path: '/data' }] }))).toMatchObject({ status: 400 });
    expect(await post({ ...request, requester_id: 'not-a-key' })).toMatchObject({ status: 400 });
    expect(await post({ ...request, kind: 'wasm' })).toMatchObject({ status: 400, body: { error: expect.stringContaining('published function') } });
    expect(await post({ ...request, offer_id: 'other' })).toMatchObject({ status: 404 });
    expect(await post(null)).toMatchObject({ status: 400 });
  });

  it('hands the function the canonical JSON text of its input, as a node does, whatever order the requester wrote it in', async () => {
    const received: string[] = [];
    const recording: ModuleRunner = async (_module, input) => {
      received.push(input);
      return { ok: true, output: '{"ok":true}', memoryBytes: 0, capEnforced: true };
    };
    const context = await setup({ runModule: recording });
    assertOk(await context.exchange({ inputText: '{ "b": 7,\n  "a": [3, {"y": 1, "x": 2}], "c": "é" }' }));
    expect(received).toEqual(['{"a":[3,{"x":2,"y":1}],"b":7,"c":"é"}']);
    // The input hash in the workload is the hash of exactly those bytes.
    const { execution } = await quoted(context, { b: 7, a: 1 });
    expect(execution.input_hash).toBe(kernel.hashBytes(new TextEncoder().encode('{"a":1,"b":7}')));
  });

  it('refuses a workload that carries no input', async () => {
    const context = await setup();
    const { request } = await quoted(context);
    const { input: _dropped, ...withoutInput } = request.execution;
    const response = await context.provider.handle({ method: 'POST', path: '/v1/provider/quotes', body: { ...request, execution: withoutInput } });
    expect(response).toMatchObject({ status: 400, body: { error: 'the workload has no input' } });
  });

  it('refuses input over the limit', async () => {
    const context = await setup();
    const big = { a: 'x'.repeat(PLAYGROUND_LIMITS.max_input_bytes + 1) };
    const { response } = await quoted(context, big);
    expect(response.status).toBe(413);
  });

  it('refuses a deal for a quote it did not issue, or one that was altered', async () => {
    const context = await setup();
    const { execution, quote } = await quoted(context);
    const foreign = otherIdentity();
    const foreignQuote = kernel.sign(foreign.seed_hex, 'quote', context.clock.now(), { ...quote.payload, provider_id: foreign.public_key });
    const notIssued = await postDeal(context, { quote: foreignQuote, deal: signedDeal(context, foreignQuote), kind: 'execution', execution });
    expect(notIssued).toMatchObject({ status: 400, body: { error: 'that quote was not issued by this provider' } });
    const altered = await postDeal(context, { quote: { ...quote, signature: '00'.repeat(64) }, deal: signedDeal(context, quote), kind: 'execution', execution });
    expect(altered).toMatchObject({ status: 400, body: { error: 'that quote was not issued by this provider' } });
    expect(await postDeal(context, undefined)).toMatchObject({ status: 400 });
    expect(await postDeal(context, { quote })).toMatchObject({ status: 400 });
  });

  it('refuses a deal signed by someone the quote was not issued to', async () => {
    const context = await setup();
    const { execution, quote } = await quoted(context);
    const stranger = otherIdentity();
    const response = await postDeal(context, { quote, deal: signedDeal(context, quote, {}, stranger), kind: 'execution', execution });
    expect(response).toMatchObject({ status: 400, body: { error: expect.stringContaining('requester the quote was issued to') } });
    const claimed = await postDeal(context, { quote, deal: signedDeal(context, quote, { requester_id: stranger.public_key }), kind: 'execution', execution });
    expect(claimed.status).toBe(400);
  });

  it('refuses a deal that commits to a different quote or workload than the one it comes with', async () => {
    const context = await setup();
    const { execution, quote } = await quoted(context);
    const wrongQuote = await postDeal(context, { quote, deal: signedDeal(context, quote, { quote_hash: '0'.repeat(64) }), kind: 'execution', execution });
    expect(wrongQuote).toMatchObject({ status: 400 });
    const wrongWorkload = await postDeal(context, { quote, deal: signedDeal(context, quote, { workload_hash: kernel.hashJson({ other: true }) }), kind: 'execution', execution });
    expect(wrongWorkload).toMatchObject({ status: 400, body: { error: expect.stringContaining('workload_hash') } });
    const changedAfterQuote = { ...execution, input: { a: 2, b: 2 }, input_hash: kernel.hashJson({ a: 2, b: 2 }) };
    const differentBody = await postDeal(context, { quote, deal: signedDeal(context, quote), kind: 'execution', execution: changedAfterQuote });
    expect(differentBody).toMatchObject({ status: 400, body: { error: expect.stringContaining('workload_hash') } });
    const otherProvider = await postDeal(context, { quote, deal: signedDeal(context, quote, { provider_id: otherIdentity().public_key }), kind: 'execution', execution });
    expect(otherProvider.status).toBe(400);
  });

  it('refuses a deal after its quote expired, or outside the admission window', async () => {
    const context = await setup();
    const { execution, quote } = await quoted(context);
    const late = await postDeal(context, { quote, deal: signedDeal(context, quote, { admission_deadline: quote.payload.expires_at + 1 }), kind: 'execution', execution });
    expect(late).toMatchObject({ status: 409, body: { error: expect.stringContaining('admission window') } });
    const deal = signedDeal(context, quote);
    context.clock.advance(QUOTE_TTL_SECS + 1);
    expect(await postDeal(context, { quote, deal, kind: 'execution', execution })).toMatchObject({ status: 409, body: { error: 'the quote has expired' } });
  });

  it('answers a deal it already has with that deal, and runs it once', async () => {
    const context = await setup();
    const { execution, quote } = await quoted(context);
    const deal = signedDeal(context, quote);
    const body = { quote, deal, kind: 'execution', execution, idempotency_key: 'same-key' };
    const first = await postDeal(context, body);
    expect(first.status).toBe(202);
    const again = await postDeal(context, body);
    expect(again.status).toBe(200);
    expect((again.body as any).deal_id).toBe((first.body as any).deal_id);
    const withoutKey = await postDeal(context, { quote, deal, kind: 'execution', execution });
    expect(withoutKey).toMatchObject({ status: 200, body: { deal_id: (first.body as any).deal_id } });
    const done = await finished(context, (first.body as any).deal_id);
    expect(done.status).toBe('succeeded');
    // Even after the quote has expired, asking again for the same deal returns it.
    context.clock.advance(QUOTE_TTL_SECS * 10);
    expect(await postDeal(context, body)).toMatchObject({ status: 200, body: { status: 'succeeded' } });
    expect(context.events.filter((event) => event.type === 'executed')).toHaveLength(1);
  });

  it('gives one quote one deal, and one idempotency key one deal', async () => {
    const context = await setup();
    const { execution, quote } = await quoted(context);
    const first = signedDeal(context, quote);
    const accepted = await postDeal(context, { quote, deal: first, kind: 'execution', execution, idempotency_key: 'k1' });
    expect(accepted.status).toBe(202);
    context.clock.advance(1);
    const second = signedDeal(context, quote);
    expect(second.hash).not.toBe(first.hash);
    expect(await postDeal(context, { quote, deal: second, kind: 'execution', execution, idempotency_key: 'k2' })).toMatchObject({
      status: 409,
      body: { error: 'quote already used by a different deal' },
    });
    expect(await postDeal(context, { quote, deal: second, kind: 'execution', execution, idempotency_key: 'k1' })).toMatchObject({
      status: 409,
      body: { error: 'idempotency key reused with different deal payload' },
    });

    const another = await quoted(context, { a: 3, b: 3 });
    expect(await postDeal(context, { quote: another.quote, deal: signedDeal(context, another.quote), kind: 'execution', execution: another.execution, idempotency_key: 'k1' })).toMatchObject({
      status: 409,
      body: { error: 'idempotency key reused with different deal payload' },
    });
    await finished(context, (accepted.body as any).deal_id);
  });

  it('knows its routes and answers the rest with a refusal, not a crash', async () => {
    const context = await setup();
    for (const path of ['/nothing', '/v1/provider/deals/zzz', '/v1/provider/deals/' + 'a'.repeat(32), '/v1/artifacts/short', '/v1/provider/services/a/b']) {
      expect((await get(context, path)).status, path).toBe(404);
    }
    expect((await context.provider.handle({ method: 'POST', path: '/v1/provider/nothing', body: {} })).status).toBe(404);
    expect((await context.provider.handle({ method: 'POST', path: '/v1/provider/quotes', body: 'text' })).status).toBe(400);
    expect((await context.provider.handle({ method: 'POST', path: '/v1/provider/deals', body: [] })).status).toBe(400);
  });
});

describe('Alice waits as long as a node does', () => {
  /** What a node's provider says while it works: the same record, but `running`, with no receipt or result yet. */
  const saying = (status: string, polls: number, extra: Record<string, unknown> = {}) => {
    let remaining = polls;
    return (request: HttpRequest, response: HttpResponse) => {
      if (request.method !== 'GET' || !/^\/v1\/provider\/deals\/[0-9a-f]+$/.test(request.path) || remaining <= 0) return;
      remaining -= 1;
      const record = response.body as Record<string, unknown>;
      for (const field of ['receipt', 'result', 'result_hash']) delete record[field];
      Object.assign(record, { status }, extra);
    };
  };

  it('keeps asking while the deal is running, which this page\'s own provider never says', async () => {
    const context = await setup();
    const transport = tampering(context.wire.transport, saying('running', 3));
    const outcome = await context.exchange({ transport });
    assertOk(outcome);
    expect(outcome.status).toBe('succeeded');
    expect(outcome.result).toEqual({ sum: 13, product: 42 });
    expect(outcome.chain.valid).toBe(true);
  });

  it('keeps asking through every state a node has before the end', async () => {
    for (const status of ['payment_pending', 'running', 'result_ready', 'settlement_pending']) {
      const context = await setup();
      const outcome = await context.exchange({ transport: tampering(context.wire.transport, saying(status, 2)) });
      assertOk(outcome);
      expect(outcome.status, status).toBe('succeeded');
    }
  });

  it('says what Bob gave as the reason when he turns a deal down after taking it', async () => {
    const context = await setup();
    const outcome = await context.exchange({ transport: tampering(context.wire.transport, saying('rejected', 1, { error: 'the provider is at capacity' })) });
    expect(outcome).toMatchObject({ ok: false, stage: 'result', refusedBy: 'bob', reason: 'the provider is at capacity' });
  });

  it('gives up on a deal that stays unfinished, and says so', async () => {
    const context = await setup();
    const outcome = await context.exchange({ transport: tampering(context.wire.transport, saying('running', 1_000_000)), maxPolls: 5 });
    expect(outcome).toMatchObject({ ok: false, stage: 'result', refusedBy: 'bob', reason: 'Bob has not finished.' });
  });

  it('calls a deal over exactly when a node does: succeeded, failed, or rejected', () => {
    // The states are the ones the node documents for a deal, read from its API description.
    const openapi = readFileSync(resolve(repoRoot, 'docs/openapi.yaml'), 'utf8');
    const record = openapi.slice(openapi.indexOf('    DealRecord:\n'));
    const block = record.slice(record.indexOf('        status:\n'), record.indexOf('        workload_kind:'));
    const states = Array.from(block.matchAll(/^\s+- (\w+)$/gm), (match) => match[1]);
    expect(states).toEqual(['accepted', 'payment_pending', 'running', 'result_ready', 'settlement_pending', 'succeeded', 'failed', 'rejected']);
    for (const status of states) expect(isFinished({ status: status as any }), status).toBe(['succeeded', 'failed', 'rejected'].includes(status));
    expect(isFinished({ status: 'a state a later node adds' as any })).toBe(false);
    // The node's own constants agree with its API description.
    const deals = readFileSync(resolve(repoRoot, 'src/deals.rs'), 'utf8');
    for (const status of states) expect(deals, status).toContain(`= "${status}";`);
  });
});

describe('the failure words are the node\'s', () => {
  it('classifies failures the way the node does', () => {
    expect(failureCodeFor({ error: 'Wasm module wall-clock timeout exceeded after 2s', timedOut: true })).toBe('execution_timed_out');
    expect(failureCodeFor({ error: 'Wasm module wall-clock timeout exceeded after 2s' })).toBe('execution_timed_out');
    expect(failureCodeFor({ error: 'deadline reached' })).toBe('execution_timed_out');
    expect(failureCodeFor({ error: 'Wasm module output size limit exceeded' })).toBe('execution_limit_exceeded');
    expect(failureCodeFor({ error: 'ran out of fuel' })).toBe('execution_limit_exceeded');
    expect(failureCodeFor({ error: 'unreachable' })).toBe('execution_failed');
  });

  it('keeps to the codes and trigger words in the node\'s own source', () => {
    const node = readFileSync(resolve(repoRoot, 'src/api/mod.rs'), 'utf8');
    const start = node.indexOf('fn classify_execution_failure');
    expect(start).toBeGreaterThan(-1);
    const body = node.slice(start, node.indexOf('\n}\n', start));
    for (const literal of ['"execution_timed_out"', '"execution_limit_exceeded"', '"execution_failed"', '"timeout"', '"deadline"', '"interrupt"', '"fuel"', '"execution limit"', '"limit exceeded"']) {
      expect(body, literal).toContain(literal);
    }
  });
});

describe('the playground speaks the wire a real node speaks', () => {
  const missing = (ours: string[], theirs: string[]) => theirs.filter((path) => !ours.includes(path));
  const extra = (ours: string[], theirs: string[]) => ours.filter((path) => !theirs.includes(path));
  const fixtureOffer = nodeExchange.offers.offers.find((offer: any) => offer.payload.offer_id === nodeExchange.service.service.offer_id);

  it('builds the workload the way the node hashed it', () => {
    const service = nodeExchange.service.service;
    const execution = executionFor(service, nodeExchange.input, kernel);
    expect(kernel.hashJson(execution)).toBe(nodeExchange.quote.payload.workload_hash);
    expect(kernel.hashJson(nodeExchange.input)).toBe(execution.input_hash);
  });

  it('hashes the captured result the way the node did', () => {
    expect(kernel.hashJson(nodeExchange.deal_final.result)).toBe(nodeExchange.deal_final.result_hash);
    expect(kernel.hashJson(nodeExchange.deal_final.result)).toBe(nodeExchange.deal_final.receipt.payload.result_hash);
  });

  it('serves every artifact and record with the fields the node\'s capture has, and no others', async () => {
    const context = await setup();
    const outcome = await context.exchange({ inputText: JSON.stringify(nodeExchange.input) });
    assertOk(outcome);
    const catalog = async (path: string) => (await get(context, path)).body as any;
    const first = (context.wire.entries.find((entry) => entry.method === 'POST' && entry.path === '/v1/provider/deals')!.response as any);
    const last = (await get(context, `/v1/provider/deals/${first.deal_id}`)).body as any;

    const same = (name: string, ours: unknown, theirs: unknown) => {
      expect({ name, missing: missing(shapeOf(ours), shapeOf(theirs)), extra: extra(shapeOf(ours), shapeOf(theirs)) }).toEqual({ name, missing: [], extra: [] });
    };
    // Envelopes and payloads, one for one.
    same('quote', outcome.artifacts.quote, nodeExchange.quote);
    same('deal', outcome.artifacts.deal, nodeExchange.deal_final.deal);
    same('receipt', outcome.artifacts.receipt, nodeExchange.deal_final.receipt);
    same('service offer', outcome.artifacts.offer, fixtureOffer);
    // A node's descriptor also lists the identities linked to the provider; a playground provider has none.
    const descriptorGap = missing(shapeOf(outcome.artifacts.descriptor), shapeOf(nodeExchange.descriptor));
    expect(descriptorGap.every((path) => path.startsWith('$.payload.linked_identities'))).toBe(true);
    expect(descriptorGap.length).toBeGreaterThan(0);
    expect(extra(shapeOf(outcome.artifacts.descriptor), shapeOf(nodeExchange.descriptor))).toEqual([]);
    // Records, and the routes' wrappers.
    same('deal record as created', first, nodeExchange.deal_created);
    same('deal record as finished', last, nodeExchange.deal_final);
    same('service record', (await catalog('/v1/provider/services/demo.adder')), nodeExchange.service);
    same('listed service', (await catalog('/v1/provider/services')).services[0], nodeExchange.services.services[0]);
    expect(Object.keys(await catalog('/v1/provider/offers'))).toEqual(Object.keys(nodeExchange.offers));
    expect(Object.keys(await catalog('/v1/provider/services'))).toEqual(Object.keys(nodeExchange.services));
    // The values a node fixes for a free Wasm service are the values here.
    expect(outcome.artifacts.receipt.payload.executor.abi_version).toBe(nodeExchange.deal_final.receipt.payload.executor.abi_version);
    expect(outcome.artifacts.receipt.payload.settlement_state).toBe(nodeExchange.deal_final.receipt.payload.settlement_state);
    expect(outcome.artifacts.receipt.payload.result_format).toBe(nodeExchange.deal_final.receipt.payload.result_format);
    expect(outcome.artifacts.receipt.payload.settlement_refs).toEqual(nodeExchange.deal_final.receipt.payload.settlement_refs);
    expect(outcome.artifacts.quote.payload.settlement_terms).toEqual(nodeExchange.quote.payload.settlement_terms);
    expect(outcome.artifacts.offer.payload.execution_profile).toMatchObject({ abi_version: fixtureOffer.payload.execution_profile.abi_version, package_kind: 'inline_module', runtime: 'wasm' });
    expect(outcome.artifacts.offer.payload.settlement_method).toBe(fixtureOffer.payload.settlement_method);
    expect(outcome.artifacts.offer.payload.price_schedule).toEqual(fixtureOffer.payload.price_schedule);
    expect((await catalog('/v1/provider/services/demo.adder')).service).toMatchObject({
      runtime: nodeExchange.service.service.runtime,
      package_kind: nodeExchange.service.service.package_kind,
      entrypoint_kind: nodeExchange.service.service.entrypoint_kind,
      entrypoint: nodeExchange.service.service.entrypoint,
      contract_version: nodeExchange.service.service.contract_version,
      mode: nodeExchange.service.service.mode,
      resource_kind: nodeExchange.service.service.resource_kind,
      publication_state: nodeExchange.service.service.publication_state,
    });
  });

  it('gives the same answer the node gave for the same function and input', async () => {
    // Rebuilds can embed host-specific Rust sysroot paths. Replay the exact retained bytes instead of treating a
    // fresh compilation as the historical module; the other sample and real-node tests still exercise fresh builds.
    const module = capturedNodeModule();
    const moduleHash = createHash('sha256').update(module).digest('hex');
    expect(moduleHash).toBe(nodeExchange.service.service.module_hash);
    expect(moduleHash).toBe(nodeExchange.deal_final.receipt.payload.executor.module_hash);
    const context = await setup({ publish: [] });
    await context.provider.publish({ serviceId: 'demo.adder', summary: samples.adder.summary, module });
    const outcome = await context.exchange({ inputText: JSON.stringify(nodeExchange.input) });
    assertOk(outcome);
    expect(outcome.result).toEqual(nodeExchange.deal_final.result);
    // The receipt must name the same bytes as the captured node, not just a function with the same answer.
    expect(outcome.artifacts.receipt.payload.executor.module_hash).toBe(nodeExchange.service.service.module_hash);
  });

  it('keeps different module bytes distinct even when they give the captured answer', async () => {
    // An ignored Wasm custom section changes provenance, not execution. Hashing must still include these bytes.
    const label = new TextEncoder().encode('different-build');
    const module = withSection(capturedNodeModule(), 0, [label.length, ...label]);
    const moduleHash = createHash('sha256').update(module).digest('hex');
    expect(moduleHash).not.toBe(nodeExchange.service.service.module_hash);
    const context = await setup({ publish: [] });
    await context.provider.publish({ serviceId: 'demo.adder', summary: samples.adder.summary, module });
    const outcome = await context.exchange({ inputText: JSON.stringify(nodeExchange.input) });
    assertOk(outcome);
    expect(outcome.result).toEqual(nodeExchange.deal_final.result);
    expect(outcome.artifacts.receipt.payload.executor.module_hash).toBe(moduleHash);
  });
});
