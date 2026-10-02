import { dealPayloadFor, executionFor, findUnsafeInteger, hasNonFiniteNumber, isFinished } from './protocol';
import type { ChainReport, DealRecord, HttpResponse, Identity, Kernel, ServiceRecord, SignedArtifact, Transport, Verifier } from './types';

// A Froglet consumer that lives in this page. It asks for a function by name, signs a deal, waits for the receipt, and
// checks what came back before it believes any of it, as a node's own requester does: the artifacts must verify, the
// quote must be for what was asked, and the receipt must commit to the result that was handed over.

export type Step = 'discover' | 'quote' | 'deal' | 'result' | 'verify';

export interface ExchangeDeps {
  kernel: Kernel;
  verifier: Verifier;
  transport: Transport;
  requester: Identity;
  serviceId: string;
  /** The input as typed, so integers a browser would round can be caught before they are read. */
  inputText: string;
  /** A deliberate fault: change the deal after signing it. */
  alterDealAfterSigning?: boolean;
  now?: () => number;
  randomHex?: (bytes: number) => string;
  sleep?: (ms: number) => Promise<void>;
  onStep?: (step: Step) => void;
  pollIntervalMs?: number;
  /** Stop asking after this many polls, whatever the time limit says. Mostly for tests. */
  maxPolls?: number;
  /** How long to wait for Bob past the quote's max_runtime_ms. */
  graceMs?: number;
  /** Two identical requests in one second get one quote, which backs one deal; this waits for the next second. */
  waitForNextSecond?: () => Promise<void>;
}

export interface ExchangeArtifacts {
  descriptor: SignedArtifact;
  offer: SignedArtifact;
  quote: SignedArtifact;
  deal: SignedArtifact;
  receipt: SignedArtifact;
}

export type ExchangeOutcome =
  | {
      ok: true;
      /** Whether Bob's function ran. Either way the evidence verified. */
      status: 'succeeded' | 'failed';
      result?: unknown;
      failure?: { code: string; message: string };
      artifacts: ExchangeArtifacts;
      chain: ChainReport;
    }
  | {
      ok: false;
      stage: 'input' | Step;
      /** Who turned the exchange down. */
      refusedBy: 'alice' | 'bob';
      reason: string;
      artifacts: Partial<ExchangeArtifacts>;
    };

const hex = (bytes: Uint8Array) => Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');

const message = (response: HttpResponse) => {
  const error = (response.body as { error?: unknown } | null)?.error;
  return typeof error === 'string' ? error : `HTTP ${response.status}`;
};

export async function runExchange(deps: ExchangeDeps): Promise<ExchangeOutcome> {
  const { kernel, verifier, transport, requester } = deps;
  const now = deps.now ?? (() => Math.floor(Date.now() / 1000));
  const randomHex = deps.randomHex ?? ((bytes: number) => hex(crypto.getRandomValues(new Uint8Array(bytes))));
  const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms)));
  const artifacts: Partial<ExchangeArtifacts> = {};
  const stop = (stage: 'input' | Step, refusedBy: 'alice' | 'bob', reason: string): ExchangeOutcome => ({ ok: false, stage, refusedBy, reason, artifacts: { ...artifacts } });
  const get = (path: string) => transport({ method: 'GET', path });

  // What Alice asks for, checked before anything is sent.
  const unsafe = findUnsafeInteger(deps.inputText);
  if (unsafe) return stop('input', 'alice', `${unsafe} is too large for a browser to hold exactly. Use integers within ±9007199254740991.`);
  let input: unknown;
  try {
    input = JSON.parse(deps.inputText);
  } catch {
    return stop('input', 'alice', 'The input is not valid JSON.');
  }
  if (hasNonFiniteNumber(input)) return stop('input', 'alice', 'A number in the input is too large for a browser to hold.');

  // 1. Find the provider and the function.
  deps.onStep?.('discover');
  const descriptorResponse = await get('/v1/provider/descriptor');
  if (descriptorResponse.status !== 200) return stop('discover', 'bob', message(descriptorResponse));
  const descriptor = descriptorResponse.body as SignedArtifact;
  if (verifier.verifyDocument(descriptor).status !== 'verified') return stop('discover', 'alice', 'The descriptor does not verify.');
  artifacts.descriptor = descriptor;
  const offersResponse = await get('/v1/provider/offers');
  if (offersResponse.status !== 200) return stop('discover', 'bob', message(offersResponse));
  const serviceResponse = await get(`/v1/provider/services/${encodeURIComponent(deps.serviceId)}`);
  if (serviceResponse.status !== 200) return stop('discover', 'bob', message(serviceResponse));
  const service = (serviceResponse.body as { service: ServiceRecord }).service;
  const offer = (offersResponse.body as { offers: SignedArtifact[] }).offers.find((candidate) => candidate.payload.offer_id === service.offer_id);
  if (!offer) return stop('discover', 'bob', `There is no offer for ${service.service_id}.`);
  if (verifier.verifyDocument(offer).status !== 'verified') return stop('discover', 'alice', 'The offer does not verify.');
  artifacts.offer = offer;

  // 2 and 3. Ask for a quote and check it is for what was asked, then commit to it with the deal, which is the one artifact
  // the requester signs. A quote backs one deal, and one second's quote for one request is one quote, so a repeat within the
  // same second is told the quote is used: wait for the next second and ask again.
  const execution = executionFor(service, input, kernel);
  const workloadHash = kernel.hashJson(execution);
  const waitForNextSecond = deps.waitForNextSecond ?? (() => sleep(1010 - (Date.now() % 1000)));
  let quote!: SignedArtifact;
  let deal!: SignedArtifact;
  let record!: DealRecord;
  for (let attempt = 0; ; attempt++) {
    deps.onStep?.('quote');
    const quoteResponse = await transport({ method: 'POST', path: '/v1/provider/quotes', body: { offer_id: offer.payload.offer_id, requester_id: requester.public_key, kind: 'execution', execution, max_price_sats: 0 } });
    if (quoteResponse.status !== 201) return stop('quote', 'bob', message(quoteResponse));
    quote = quoteResponse.body as SignedArtifact;
    if (verifier.verifyDocument(quote).status !== 'verified') return stop('quote', 'alice', 'The quote does not verify.');
    const q = quote.payload;
    if (q.provider_id !== descriptor.payload.provider_id) return stop('quote', 'alice', 'The quote is from a different provider than the descriptor.');
    if (q.requester_id !== requester.public_key) return stop('quote', 'alice', 'The quote is for a different requester.');
    if (q.workload_hash !== workloadHash) return stop('quote', 'alice', 'The quote is for a different workload than the one asked for.');
    if (q.offer_hash !== offer.hash || q.descriptor_hash !== descriptor.hash) return stop('quote', 'alice', 'The quote does not point at the offer and descriptor Alice read.');
    if (q.expires_at < now()) return stop('quote', 'alice', 'The quote has already expired.');
    artifacts.quote = quote;

    deps.onStep?.('deal');
    const signed = kernel.sign(requester.seed_hex, 'deal', now(), dealPayloadFor(quote, requester.public_key, kernel.hashBytes(crypto.getRandomValues(new Uint8Array(32)))));
    // The fault: a change after signing, which the signature no longer covers.
    deal = deps.alterDealAfterSigning ? { ...signed, created_at: signed.created_at + 1 } : signed;
    const dealResponse = await transport({ method: 'POST', path: '/v1/provider/deals', body: { quote, deal, kind: 'execution', execution, idempotency_key: randomHex(16) } });
    if (dealResponse.status === 409 && message(dealResponse).includes('quote already used') && attempt < 2) {
      await waitForNextSecond();
      continue;
    }
    if (dealResponse.status !== 200 && dealResponse.status !== 202) return stop('deal', 'bob', message(dealResponse));
    record = dealResponse.body as DealRecord;
    break;
  }
  if (record.deal.hash !== deal.hash || record.quote.hash !== quote.hash) return stop('deal', 'alice', 'Bob returned a different deal than the one Alice signed.');
  if (record.status === 'rejected') return stop('deal', 'bob', record.error || 'Bob rejected the deal.');
  artifacts.deal = deal;

  // 4. Wait for Bob to finish, as long as his own limit for the function allows, then check the receipt against the result.
  deps.onStep?.('result');
  const giveUpAt = performance.now() + (quote.payload.execution_limits.max_runtime_ms as number) + (deps.graceMs ?? 5000);
  for (let polls = 0; !isFinished(record); polls++) {
    if (polls >= (deps.maxPolls ?? Infinity) || performance.now() > giveUpAt) return stop('result', 'bob', 'Bob has not finished.');
    await sleep(deps.pollIntervalMs ?? 50);
    const polled = await get(`/v1/provider/deals/${record.deal_id}`);
    if (polled.status !== 200) return stop('result', 'bob', message(polled));
    record = polled.body as DealRecord;
  }
  if (record.status === 'rejected') return stop('result', 'bob', record.error || 'Bob rejected the deal.');
  const receipt = record.receipt;
  if (!receipt) return stop('result', 'bob', 'Bob reports the deal finished but sent no receipt.');
  artifacts.receipt = receipt;
  const r = receipt.payload;
  if (r.provider_id !== descriptor.payload.provider_id) return stop('result', 'alice', 'The receipt is from a different provider.');
  if (r.requester_id !== requester.public_key) return stop('result', 'alice', 'The receipt is for a different requester.');
  if (r.deal_hash !== deal.hash || r.quote_hash !== quote.hash) return stop('result', 'alice', 'The receipt is not for this deal.');
  const finished: 'succeeded' | 'failed' = record.status === 'succeeded' ? 'succeeded' : 'failed';
  if (r.deal_state !== finished) return stop('result', 'alice', 'The receipt and the deal record disagree about whether the function ran.');
  if (finished === 'succeeded') {
    const consistent = record.result !== undefined && receipt.payload.result_hash === record.result_hash && record.result_hash === kernel.hashJson(record.result);
    if (!consistent) return stop('result', 'alice', 'The receipt, result_hash, and result do not match: Bob signed a hash of something other than the result he handed over.');
  }

  // 5. Check the whole chain of evidence offline.
  deps.onStep?.('verify');
  const chain = verifier.validateChain([descriptor, offer, quote, deal, receipt]);
  if (!chain.valid) {
    const failing = chain.artifacts.filter((artifact) => artifact.status === 'invalid').map((artifact) => artifact.artifact_type);
    return stop('verify', 'alice', failing.length ? `The ${failing.join(' and ')} does not verify.` : 'The evidence does not form a valid chain.');
  }
  const all = { descriptor, offer, quote, deal, receipt };
  return finished === 'succeeded'
    ? { ok: true, status: 'succeeded', result: record.result, artifacts: all, chain }
    : { ok: true, status: 'failed', failure: { code: String(r.failure_code ?? 'execution_failed'), message: String(r.failure_message ?? record.error ?? '') }, artifacts: all, chain };
}
