import {
  JCS_JSON_FORMAT,
  PLAYGROUND_LIMITS,
  QUOTE_TTL_SECS,
  Refusal,
  RUN_JSON_ABI,
  SCHEMA_VERSION,
  WORKLOAD_KIND_EXECUTION,
  findUnsafeInteger,
} from './protocol';
import { admitModule, type MemoryInfo } from './wasm-module';
import type { DealRecord, HttpRequest, HttpResponse, Identity, Kernel, Limits, ModuleRunner, RunOutcome, ServiceRecord, SignedArtifact, Verifier } from './types';

// A Froglet provider that lives in this page: the routes and JSON of a node's provider API, for functions published as
// WebAssembly modules. It signs with the real kernel and checks what a requester sends with the real verifier. What is
// left out is what a free, in-memory exchange between two parties in one tab does not need: payments, access control, a
// Nostr link on the descriptor, and storage that survives a reload.

/** A deliberate fault, for showing that the other side catches it. */
export type Cheat = 'none' | 'wrong-result';

export interface ProviderDeps {
  kernel: Kernel;
  verifier: Verifier;
  runModule: ModuleRunner;
  /** Whole seconds since the epoch. */
  now?: () => number;
  randomHex?: (bytes: number) => string;
  limits?: Limits;
  /** Where the descriptor says the provider can be reached. Nothing is listening there; the parties talk in this page. */
  publicUrl?: string;
  identity?: Identity;
  onEvent?: (event: ProviderEvent) => void;
}

export type ProviderEvent =
  | { type: 'published'; service: PublishedService }
  | { type: 'executed'; dealId: string; serviceId: string; outcome: RunOutcome; ms: number };

export interface PublishInput {
  serviceId: string;
  summary: string;
  module: Uint8Array;
}

export interface PublishedService {
  record: ServiceRecord;
  sizeBytes: number;
  memory: MemoryInfo;
  descriptor: SignedArtifact;
  offer: SignedArtifact;
}

export interface Provider {
  readonly publicKey: string;
  readonly limits: Limits;
  publish(input: PublishInput): Promise<PublishedService>;
  handle(request: HttpRequest): Promise<HttpResponse>;
  setCheat(cheat: Cheat): void;
}

interface Hosted {
  record: ServiceRecord;
  /** The module as published: what its hash is of. */
  original: Uint8Array;
  /** The module as run: the same, with the memory cap applied. */
  capped: Uint8Array;
  memory: MemoryInfo;
}

const SERVICE_ID = /^[a-z0-9][a-z0-9._-]{0,63}$/;
const PUBLIC_KEY = /^[0-9a-f]{64}$/;

/**
 * The failure code a node puts in a receipt for a function that did not return a result. The three codes and the words
 * that select them are the node's own (`classify_execution_failure` in src/api/mod.rs).
 */
export function failureCodeFor(outcome: { error: string; timedOut?: boolean }): string {
  const message = outcome.error.toLowerCase();
  if (outcome.timedOut || message.includes('timeout') || message.includes('deadline') || message.includes('interrupt')) return 'execution_timed_out';
  if (message.includes('fuel') || message.includes('execution limit') || message.includes('limit exceeded')) return 'execution_limit_exceeded';
  return 'execution_failed';
}

const hex = (bytes: Uint8Array) => Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value));
const byteLength = (text: string) => new TextEncoder().encode(text).length;

export function createProvider(deps: ProviderDeps): Provider {
  const { kernel, verifier, runModule } = deps;
  const limits = deps.limits ?? PLAYGROUND_LIMITS;
  const now = deps.now ?? (() => Math.floor(Date.now() / 1000));
  const randomHex = deps.randomHex ?? ((bytes: number) => hex(crypto.getRandomValues(new Uint8Array(bytes))));
  const publicUrl = deps.publicUrl ?? 'https://provider.playground.invalid';
  const identity = deps.identity ?? kernel.newIdentity();
  const providerId = identity.public_key;
  const sign = (type: Parameters<Kernel['sign']>[1], payload: unknown) => kernel.sign(identity.seed_hex, type, now(), payload);

  const services = new Map<string, Hosted>();
  const quotes = new Map<string, SignedArtifact>();
  const deals = new Map<string, DealRecord>();
  const dealsByKey = new Map<string, string>();
  const dealsByHash = new Map<string, string>();
  const quoteUse = new Map<string, string>();
  const pending = new Map<string, { hosted: Hosted; inputText: string }>();
  const artifacts = new Map<string, SignedArtifact>();
  let descriptor: SignedArtifact | undefined;
  let offers: SignedArtifact[] = [];
  let descriptorSeq = 0;
  let cheat: Cheat = 'none';

  // ── The catalog: a descriptor, and an offer for each function. It is signed again whenever a function is published. ──
  function rebuildCatalog() {
    descriptorSeq += 1;
    descriptor = sign('descriptor', {
      capabilities: { execution_runtimes: ['wasm'], max_concurrent_deals: 4, service_kinds: [WORKLOAD_KIND_EXECUTION] },
      descriptor_seq: descriptorSeq,
      protocol_version: SCHEMA_VERSION,
      provider_id: providerId,
      transport_endpoints: [{ features: ['quote_http', 'artifact_fetch', 'receipt_poll'], priority: 10, transport: 'https', uri: publicUrl }],
    });
    offers = [...services.values()].map(({ record }) =>
      sign('offer', {
        descriptor_hash: descriptor!.hash,
        execution_profile: { abi_version: RUN_JSON_ABI, contract_version: RUN_JSON_ABI, ...limits, package_kind: 'inline_module', runtime: 'wasm' },
        offer_id: record.offer_id,
        offer_kind: WORKLOAD_KIND_EXECUTION,
        price_schedule: { base_fee_msat: 0, success_fee_msat: 0 },
        provider_id: providerId,
        quote_ttl_secs: QUOTE_TTL_SECS,
        settlement_method: 'none',
      }),
    );
    for (const artifact of [descriptor, ...offers]) artifacts.set(artifact.hash, artifact);
  }

  async function publish({ serviceId, summary, module }: PublishInput): Promise<PublishedService> {
    if (!SERVICE_ID.test(serviceId)) throw new Refusal(400, 'a service id is lowercase letters, digits, dots, dashes, and underscores');
    const admitted = await admitModule(module, limits);
    const moduleHash = kernel.hashBytes(module);
    const record: ServiceRecord = {
      service_id: serviceId,
      offer_id: serviceId,
      offer_kind: WORKLOAD_KIND_EXECUTION,
      resource_kind: 'service',
      summary,
      runtime: 'wasm',
      package_kind: 'inline_module',
      entrypoint_kind: 'handler',
      entrypoint: 'run',
      contract_version: RUN_JSON_ABI,
      mode: 'sync',
      price_sats: 0,
      base_fee_msat: 0,
      success_fee_msat: 0,
      settlement_method: 'none',
      publication_state: 'active',
      provider_id: providerId,
      module_hash: moduleHash,
      binding_hash: moduleHash,
    };
    services.set(serviceId, { record, original: module, capped: admitted.bytes, memory: admitted.memory });
    rebuildCatalog();
    const published: PublishedService = {
      record: clone(record),
      sizeBytes: module.length,
      memory: admitted.memory,
      descriptor: descriptor!,
      offer: offers.find((offer) => offer.payload.offer_id === serviceId)!,
    };
    deps.onEvent?.({ type: 'published', service: published });
    return published;
  }

  // ── What a requester sends: a call to a published function by name, checked against what was published ──
  function checkExecution(spec: any) {
    const execution = spec?.kind === 'execution' ? spec.execution : undefined;
    if (!execution) throw new Refusal(400, 'only calls to a published function are served (kind "execution")');
    const serviceId = execution.security?.service_id;
    const hosted = typeof serviceId === 'string' ? services.get(serviceId) : undefined;
    if (!hosted || execution.security.mode !== 'standard') throw new Refusal(404, 'no such service');
    const record = hosted.record;
    const matches =
      execution.schema_version === SCHEMA_VERSION &&
      execution.workload_kind === WORKLOAD_KIND_EXECUTION &&
      execution.runtime === record.runtime &&
      execution.package_kind === record.package_kind &&
      execution.entrypoint?.kind === record.entrypoint_kind &&
      execution.entrypoint?.value === record.entrypoint &&
      execution.contract_version === record.contract_version &&
      execution.input_format === JCS_JSON_FORMAT;
    if (!matches) throw new Refusal(400, 'the workload does not match the published service');
    if (execution.module_hash !== record.binding_hash) throw new Refusal(409, 'module_hash does not match the published function');
    if (execution.requested_access?.length || execution.mounts?.length) throw new Refusal(400, 'access and mounts are not granted');
    if (execution.input === undefined) throw new Refusal(400, 'the workload has no input');
    // A function receives the canonical JSON text of its input, as on a node, and input_hash is the hash of those bytes.
    const inputText = kernel.canonicalize(execution.input);
    if (kernel.hashBytes(new TextEncoder().encode(inputText)) !== execution.input_hash) throw new Refusal(400, 'input_hash does not match the input');
    if (byteLength(inputText) > limits.max_input_bytes) throw new Refusal(413, 'the input is larger than max_input_bytes');
    return { hosted, inputText, workloadHash: kernel.hashJson(execution) };
  }

  function quote(body: any): HttpResponse {
    if (!PUBLIC_KEY.test(body?.requester_id ?? '')) throw new Refusal(400, 'requester_id must be a 32-byte x-only public key in hex');
    const { hosted, workloadHash } = checkExecution(body);
    if (body.offer_id !== hosted.record.offer_id) throw new Refusal(404, 'no such offer for this function');
    const offer = offers.find((candidate) => candidate.payload.offer_id === body.offer_id)!;
    const artifact = sign('quote', {
      descriptor_hash: descriptor!.hash,
      execution_limits: limits,
      expires_at: now() + QUOTE_TTL_SECS,
      offer_hash: offer.hash,
      provider_id: providerId,
      requester_id: body.requester_id,
      settlement_terms: { base_fee_msat: 0, destination_identity: '', max_base_invoice_expiry_secs: 0, max_success_hold_expiry_secs: 0, method: 'none', min_final_cltv_expiry: 0, success_fee_msat: 0 },
      workload_hash: workloadHash,
      workload_kind: WORKLOAD_KIND_EXECUTION,
    });
    quotes.set(artifact.hash, artifact);
    artifacts.set(artifact.hash, artifact);
    return { status: 201, body: artifact };
  }

  function createDeal(body: any): HttpResponse {
    const key: string | undefined = typeof body?.idempotency_key === 'string' && body.idempotency_key ? body.idempotency_key : undefined;
    const { quote: sentQuote, deal } = body ?? {};
    if (!sentQuote || typeof sentQuote !== 'object' || !deal || typeof deal !== 'object') throw new Refusal(400, 'the request must carry a quote and a deal');
    const issued = quotes.get(sentQuote.hash);
    if (!issued || issued.signature !== sentQuote.signature) throw new Refusal(400, 'that quote was not issued by this provider');

    const report = verifier.verifyDocument(deal);
    if (report.status !== 'verified') {
      throw new Refusal(400, report.envelope_valid ? `the deal is not valid: ${report.semantics.error ?? report.semantics.reason ?? 'it fails its rules'}` : 'the deal does not verify: its signature does not match what was signed');
    }
    const terms = deal.payload;
    if (deal.signer !== terms.requester_id || terms.requester_id !== issued.payload.requester_id) throw new Refusal(400, 'the deal is not signed by the requester the quote was issued to');
    if (terms.provider_id !== providerId || terms.quote_hash !== issued.hash) throw new Refusal(400, 'the deal does not commit to this provider and quote');
    const { hosted, inputText, workloadHash } = checkExecution(body);
    if (terms.workload_hash !== issued.payload.workload_hash || workloadHash !== terms.workload_hash) throw new Refusal(400, 'the deal\'s workload_hash does not match the workload sent with it');

    // As on a node: the same deal sent again is the deal that already exists, at any time, so a requester that lost the
    // answer can ask again; an idempotency key belongs to one deal; and a quote backs one deal.
    const existingId = dealsByHash.get(deal.hash);
    if (existingId) {
      if (key && dealsByKey.has(key) && dealsByKey.get(key) !== existingId) throw new Refusal(409, 'idempotency key reused with different deal payload');
      return { status: 200, body: clone(deals.get(existingId)) };
    }
    if (key && dealsByKey.has(key)) throw new Refusal(409, 'idempotency key reused with different deal payload');
    if (quoteUse.has(issued.hash)) throw new Refusal(409, 'quote already used by a different deal');
    if (now() > issued.payload.expires_at) throw new Refusal(409, 'the quote has expired');
    if (terms.admission_deadline > issued.payload.expires_at || now() > terms.admission_deadline) throw new Refusal(409, 'the deal is outside its admission window');

    const dealId = randomHex(16);
    const record: DealRecord = {
      created_at: now(),
      deal,
      deal_id: dealId,
      ...(key ? { idempotency_key: key } : {}),
      quote: issued,
      status: 'accepted',
      updated_at: now(),
      workload_kind: issued.payload.workload_kind,
    };
    deals.set(dealId, record);
    dealsByHash.set(deal.hash, dealId);
    quoteUse.set(issued.hash, deal.hash);
    if (key) dealsByKey.set(key, dealId);
    artifacts.set(deal.hash, deal);
    pending.set(dealId, { hosted, inputText });
    void execute(record);
    return { status: 202, body: clone(record) };
  }

  // ── Running the function and signing what happened, whether it worked or not ──
  async function execute(record: DealRecord) {
    const { hosted, inputText } = pending.get(record.deal_id)!;
    pending.delete(record.deal_id);
    const startedAt = now();
    const clock = performance.now();
    let outcome = await runModule(hosted.capped, inputText, limits).catch((error): RunOutcome => ({ ok: false, error: error instanceof Error ? error.message : String(error) }));
    let result: unknown;
    if (outcome.ok) {
      const unsafe = findUnsafeInteger(outcome.output);
      if (unsafe) outcome = { ok: false, error: `the result contains ${unsafe}, an integer a browser cannot hold exactly` };
      else {
        // As on a node, the answer has to be JSON. A function that returns anything else has failed, and the receipt says so
        // in the words the node's parser uses: the kernel's, which is the same parser.
        const notJson = kernel.jsonError(outcome.output);
        if (notJson !== null) outcome = { ok: false, error: notJson };
        else {
          try {
            result = JSON.parse(outcome.output);
          } catch (error) {
            // The kernel's parser is the stricter one, so this is not expected. A deal must never be left waiting, though.
            outcome = { ok: false, error: error instanceof Error ? error.message : String(error) };
          }
        }
      }
    }
    deps.onEvent?.({ type: 'executed', dealId: record.deal_id, serviceId: hosted.record.service_id, outcome, ms: performance.now() - clock });

    let resultHash: string | undefined;
    if (outcome.ok) {
      // A dishonest provider signs a hash of something other than the result it hands over.
      resultHash = kernel.hashJson(cheat === 'wrong-result' ? { tampered: true } : result);
    }
    const finishedAt = now();
    const succeeded = outcome.ok;
    const receipt = sign('receipt', {
      deal_hash: record.deal.hash,
      deal_state: succeeded ? 'succeeded' : 'failed',
      execution_state: succeeded ? 'succeeded' : 'failed',
      executor: { abi_version: RUN_JSON_ABI, module_hash: hosted.record.module_hash, runtime: 'wasm', runtime_version: 'froglet-playground' },
      finished_at: finishedAt,
      limits_applied: limits,
      provider_id: providerId,
      quote_hash: record.quote.hash,
      requester_id: record.deal.payload.requester_id,
      ...(succeeded
        ? { result_format: JCS_JSON_FORMAT, result_hash: resultHash }
        : { failure_code: outcome.ok ? '' : failureCodeFor(outcome), failure_message: outcome.ok ? '' : outcome.error }),
      settlement_refs: { base_fee: { amount_msat: 0, invoice_hash: '', payment_hash: '', state: 'canceled' }, destination_identity: '', method: 'none', success_fee: { amount_msat: 0, invoice_hash: '', payment_hash: '', state: 'canceled' } },
      settlement_state: 'none',
      started_at: startedAt,
    });
    artifacts.set(receipt.hash, receipt);
    Object.assign(record, {
      status: succeeded ? 'succeeded' : 'failed',
      updated_at: finishedAt,
      receipt,
      ...(succeeded ? { result, result_hash: resultHash } : { error: outcome.ok === false ? outcome.error : '' }),
    });
  }

  // ── The routes ──
  async function route(request: HttpRequest): Promise<HttpResponse> {
    const { method, path, body } = request;
    if (method === 'POST' && path === '/v1/provider/quotes') return quote(body);
    if (method === 'POST' && path === '/v1/provider/deals') return createDeal(body);
    if (method !== 'GET') throw new Refusal(404, 'not found');
    if (path === '/health') return { status: 200, body: { service: 'froglet', status: 'ok' } };
    if (!descriptor) throw new Refusal(503, 'nothing is published yet');
    if (path === '/v1/provider/descriptor') return { status: 200, body: descriptor };
    if (path === '/v1/provider/offers') return { status: 200, body: { offers } };
    if (path === '/v1/provider/services') return { status: 200, body: { services: [...services.values()].map(({ record }) => record) } };
    let match: RegExpMatchArray | null;
    if ((match = path.match(/^\/v1\/provider\/services\/([^/]+)$/))) {
      const hosted = services.get(decodeURIComponent(match[1]));
      if (!hosted) throw new Refusal(404, 'service not found');
      return { status: 200, body: { service: hosted.record, execution_access: 'open' } };
    }
    if ((match = path.match(/^\/v1\/provider\/deals\/([0-9a-f]+)$/))) {
      const record = deals.get(match[1]);
      if (!record) throw new Refusal(404, 'deal not found');
      return { status: 200, body: clone(record) };
    }
    if ((match = path.match(/^\/v1\/artifacts\/([0-9a-f]{64})$/))) {
      const artifact = artifacts.get(match[1]);
      if (!artifact) throw new Refusal(404, 'artifact not found');
      return { status: 200, body: artifact };
    }
    throw new Refusal(404, 'not found');
  }

  return {
    publicKey: providerId,
    limits,
    publish,
    setCheat: (next) => {
      cheat = next;
    },
    async handle(request) {
      try {
        const response = await route(request);
        return { status: response.status, body: clone(response.body) };
      } catch (error) {
        if (error instanceof Refusal) return { status: error.status, body: { error: error.message } };
        return { status: 500, body: { error: error instanceof Error ? error.message : String(error) } };
      }
    },
  };
}
