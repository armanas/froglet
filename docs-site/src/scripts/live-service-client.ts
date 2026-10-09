import { PUBLIC_DEMO, PUBLIC_MARKETPLACE_READ, PUBLIC_HTTP_PROFILE_FIELDS as profileFields, isPublishedServiceProfile as validProfile, publishedServiceProfiles, type PublicHttpServiceId, type PublishedServiceProfile } from '../data/public-demo-config';
import { dealPayloadFor, executionFor, findUnsafeInteger, hasNonFiniteNumber, isFinished } from './playground/protocol';
import type { ChainReport, DealRecord, Kernel, ServiceRecord, SignedArtifact, Transport, Verifier } from './playground/types';
import type { ExchangeArtifacts, Step } from './playground/consumer';

export { PUBLIC_HTTP_SERVICE_IDS, publishedServiceProfiles } from '../data/public-demo-config';
export type { PublicHttpServiceId, PublishedServiceProfile } from '../data/public-demo-config';
const publicHash = (value: unknown): value is string => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
const object = (value: unknown): value is Record<string, any> => value !== null && typeof value === 'object' && !Array.isArray(value);

export interface LiveRun {
  schema: 'froglet.public-demo.run.v1';
  providerId: string;
  input: unknown;
  request: { quote: SignedArtifact; deal: SignedArtifact; kind: 'wasm' | 'execution'; submission?: any; execution?: any; idempotency_key: string };
  artifacts: Omit<ExchangeArtifacts, 'receipt'>;
  dealId?: string;
  publishedService?: { profile: PublishedServiceProfile; revision: unknown };
}

export interface LiveDeps {
  kernel: Kernel;
  verifier: Verifier;
  transport: Transport;
  providerId: string;
  onStep?: (step: Step) => void;
  save?: (run: LiveRun) => void;
  sleep?: (ms: number) => Promise<void>;
  publishedServices?: readonly PublishedServiceProfile[];
  verifyPublication?: (revision: unknown, offer: unknown, descriptor: unknown) => { valid: boolean } | Promise<{ valid: boolean }>;
}

export type LiveResult =
  | { terminal: true; status: 'succeeded' | 'failed'; result?: unknown; failure?: string; artifacts: ExchangeArtifacts; chain: ChainReport; run: LiveRun }
  | { terminal: false; reason: string; run: LiveRun };

/** The verifier accepts an ordered artifact array, independent of this page's state. */
export function exportLiveEvidence(outcome: Extract<LiveResult, { terminal: true }>) {
  const a = outcome.artifacts;
  return { schema: 'froglet.public-demo.evidence.v1', input: outcome.run.input, result: outcome.result ?? null,
    status: outcome.status, ...(outcome.failure ? { failure: outcome.failure } : {}),
    ...(outcome.run.publishedService ? { publication_revision: outcome.run.publishedService.revision } : {}),
    artifacts: [a.descriptor, a.offer, a.quote, a.deal, a.receipt] };
}

const hex = (bytes: Uint8Array) => Array.from(bytes, b => b.toString(16).padStart(2, '0')).join('');
const randomHex = (bytes: number) => hex(crypto.getRandomValues(new Uint8Array(bytes)));
const reason = (response: { status: number; body: unknown }) => {
  const error = (response.body as { error?: unknown } | null)?.error;
  return typeof error === 'string' ? error : `The provider returned HTTP ${response.status}.`;
};
const need = (ok: unknown, message: string): void => { if (!ok) throw new Error(message); };

function parsedInput(inputText: string): unknown {
  need(!findUnsafeInteger(inputText), 'Use integers that a browser can represent exactly.');
  let input: unknown;
  try { input = JSON.parse(inputText); } catch { throw new Error('Enter valid JSON input.'); }
  need(!hasNonFiniteNumber(input), 'Use finite numbers.');
  return input;
}

function document(deps: LiveDeps, artifact: SignedArtifact, kind: string) {
  need(artifact?.artifact_type === kind && deps.verifier.verifyDocument(artifact).status === 'verified', `The ${kind} does not verify.`);
}

function workload(run: LiveRun, kernel: Kernel): string {
  if (run.request.kind === 'wasm') {
    const s = run.request.submission;
    need(s && /^[a-f0-9]+$/.test(s.module_bytes_hex) && s.module_bytes_hex.length % 2 === 0, 'The saved program is invalid.');
    const bytes = Uint8Array.from(s.module_bytes_hex.match(/../g), (h: string) => parseInt(h, 16));
    need(s.workload.module_hash === kernel.hashBytes(bytes) && s.workload.input_hash === kernel.hashJson(s.input), 'The saved program or input has changed.');
    need(kernel.hashJson(s.input) === kernel.hashJson(run.input), 'The saved input has changed.');
    return kernel.hashJson(s.workload);
  }
  need(run.request.execution?.input_hash === kernel.hashJson(run.input) && kernel.hashJson(run.request.execution.input) === kernel.hashJson(run.input), 'The saved input has changed.');
  return kernel.hashJson(run.request.execution);
}

/** An active offer may keep its immutable link to an earlier signed descriptor. */
async function descriptorForOffer(deps: LiveDeps, offer: SignedArtifact, current: SignedArtifact): Promise<SignedArtifact> {
  const expected = offer.payload.descriptor_hash;
  need(publicHash(expected), 'The offer has an invalid provider-description reference.');
  if (expected === current.hash) return current;
  const response = await deps.transport({ method: 'GET', path: '/v1/artifacts/' + expected });
  need(response.status === 200, reason(response));
  const record = response.body as { hash: unknown; kind: unknown; actor_id: unknown; document: SignedArtifact };
  need(record?.hash === expected && record.kind === 'descriptor' && record.actor_id === deps.providerId && record.document?.hash === expected && record.document.signer === deps.providerId && record.document.payload?.provider_id === deps.providerId, 'The referenced provider description has a different identity, type or hash.');
  document(deps, record.document, 'descriptor');
  need(currentDocument(record.document), 'The referenced provider description has expired.');
  return record.document;
}

/** Prepare and sign in the browser. Only public artifacts, never its private seed, enter the saved run. */
export async function prepareLiveRun(deps: LiveDeps, inputText: string, module?: Uint8Array): Promise<LiveRun> {
  const input = parsedInput(inputText);
  need(new TextEncoder().encode(deps.kernel.canonicalize(input)).length <= PUBLIC_DEMO.maxInputBytes, 'The input is too large.');
  if (module) need(module.length >= 8 && module.length <= PUBLIC_DEMO.maxModuleBytes && hex(module.subarray(0, 8)) === '0061736d01000000', 'Supply a bounded Wasm v1 program.');
  deps.onStep?.('discover');
  const descriptorResponse = await deps.transport({ method: 'GET', path: '/v1/provider/descriptor' });
  need(descriptorResponse.status === 200, reason(descriptorResponse));
  let descriptor = descriptorResponse.body as SignedArtifact;
  document(deps, descriptor, 'descriptor');
  need(descriptor.payload.provider_id === deps.providerId, 'This is a different provider.');
  const offersResponse = await deps.transport({ method: 'GET', path: '/v1/provider/offers' });
  need(offersResponse.status === 200, reason(offersResponse));
  const offerId = module ? PUBLIC_DEMO.computeOffer : PUBLIC_DEMO.catalogService;
  const offer = (offersResponse.body as { offers: SignedArtifact[] }).offers?.find(o => o.payload.offer_id === offerId);
  need(offer, 'The selected demo is not published.');
  document(deps, offer!, 'offer');
  need(offer!.payload.provider_id === deps.providerId, 'The offer belongs to a different provider.');
  descriptor = await descriptorForOffer(deps, offer!, descriptor);
  need(offer!.payload.provider_id === deps.providerId && offer!.payload.descriptor_hash === descriptor.hash, 'The offer is not bound to this provider description.');
  need(offer!.payload.settlement_method === 'none' && offer!.payload.price_schedule?.base_fee_msat === 0 && offer!.payload.price_schedule?.success_fee_msat === 0, 'This demo accepts free work only.');
  let spec: { kind: 'wasm'; submission: any } | { kind: 'execution'; execution: any };
  if (module) {
    const w = { schema_version: 'froglet/v1', workload_kind: 'compute.wasm.v1', abi_version: 'froglet.wasm.run_json.v1', module_format: 'application/wasm', module_hash: deps.kernel.hashBytes(module), input_format: 'application/json+jcs', input_hash: deps.kernel.hashJson(input), requested_capabilities: [] };
    spec = { kind: 'wasm', submission: { schema_version: 'froglet/v1', submission_type: 'wasm_submission', workload: w, module_bytes_hex: hex(module), input } };
  } else {
    const serviceResponse = await deps.transport({ method: 'GET', path: `/v1/provider/services/${PUBLIC_DEMO.catalogService}` });
    need(serviceResponse.status === 200, reason(serviceResponse));
    const service = (serviceResponse.body as { service: ServiceRecord }).service;
    need(service?.provider_id === deps.providerId && service.offer_id === offerId && service.runtime === 'builtin' && service.settlement_method === 'none', 'The catalog service does not match this free provider.');
    spec = { kind: 'execution', execution: { ...executionFor(service, input, deps.kernel), workload_kind: service.entrypoint, builtin_name: service.entrypoint } };
  }
  const expectedHash = deps.kernel.hashJson(spec.kind === 'wasm' ? spec.submission.workload : spec.execution);
  const requester = deps.kernel.newIdentity();
  deps.onStep?.('quote');
  const quoteResponse = await deps.transport({ method: 'POST', path: '/v1/provider/quotes', body: { offer_id: offerId, requester_id: requester.public_key, ...spec, max_price_sats: 0 } });
  need(quoteResponse.status === 201, reason(quoteResponse));
  const quote = quoteResponse.body as SignedArtifact;
  document(deps, quote, 'quote');
  const q = quote.payload;
  need(q.provider_id === deps.providerId && q.requester_id === requester.public_key && q.offer_hash === offer!.hash && q.descriptor_hash === descriptor.hash && q.workload_hash === expectedHash, 'The quote is for different work or another requester.');
  need(q.settlement_terms?.method === 'none' && q.settlement_terms.base_fee_msat === 0 && q.settlement_terms.success_fee_msat === 0 && !q.capabilities_granted?.length, 'The quote is not a free pure-computation demo.');
  const l = q.execution_limits;
  need(l && l.max_runtime_ms > 0 && l.max_runtime_ms <= PUBLIC_DEMO.maxRuntimeMs && l.max_memory_bytes <= PUBLIC_DEMO.maxMemoryBytes && l.fuel_limit <= PUBLIC_DEMO.maxFuel, 'The quote exceeds the demo resource bounds.');
  const now = Math.floor(Date.now() / 1000);
  need(q.expires_at >= now, 'The quote has expired; no job was submitted.');
  const deal = deps.kernel.sign(requester.seed_hex, 'deal', now, dealPayloadFor(quote, requester.public_key, randomHex(32)));
  const run: LiveRun = { schema: 'froglet.public-demo.run.v1', providerId: deps.providerId, input, request: { quote, deal, ...spec, idempotency_key: randomHex(16) }, artifacts: { descriptor, offer: offer!, quote, deal } };
  deps.save?.(run);
  return run;
}

const operationGrant = (profile: PublishedServiceProfile) => `net.http.operation.${profile.operationHash}`;
const exactGrant = (grants: unknown, profile: PublishedServiceProfile) => Array.isArray(grants) && grants.length === 1 && grants[0] === operationGrant(profile);
const sameProfile = (a: PublishedServiceProfile, b: PublishedServiceProfile) => profileFields.every(key => a[key as keyof PublishedServiceProfile] === b[key as keyof PublishedServiceProfile]);
const currentDocument = (doc: SignedArtifact) => doc.payload.expires_at === undefined || doc.payload.expires_at === null || Number.isSafeInteger(doc.payload.expires_at) && doc.payload.expires_at >= Math.floor(Date.now() / 1000);

function publishedInput(serviceId: PublicHttpServiceId, input: unknown): boolean {
  if (!object(input)) return false;
  const search = serviceId === 'marketplace-search', provider = serviceId === 'marketplace-provider';
  const allowed = provider ? ['provider_id'] : search ? ['provider_id', 'offer_kind', 'runtime', 'availability', 'limit', 'offset'] : ['provider_id', 'limit', 'offset', 'updated_since'];
  if (Object.keys(input).some(key => !allowed.includes(key)) || (!search && !publicHash(input.provider_id)) || (input.provider_id !== undefined && !publicHash(input.provider_id))) return false;
  if (input.limit !== undefined && (!Number.isSafeInteger(input.limit) || input.limit < 1 || input.limit > 20)) return false;
  if (input.offset !== undefined && (!Number.isSafeInteger(input.offset) || input.offset < 0 || input.offset > 1000)) return false;
  if (input.updated_since !== undefined && (!Number.isSafeInteger(input.updated_since) || input.updated_since < 0)) return false;
  return (input.offer_kind === undefined || typeof input.offer_kind === 'string' && /^[a-z0-9][a-z0-9._-]{0,63}$/.test(input.offer_kind)) && (input.runtime === undefined || ['builtin', 'wasm', 'any'].includes(input.runtime)) && (input.availability === undefined || ['discoverable', 'healthy', 'offline', 'unknown', 'all'].includes(input.availability));
}

function publishedQuote(quote: SignedArtifact, profile: PublishedServiceProfile) {
  const q = quote.payload, l = q.execution_limits, t = q.settlement_terms;
  need(q.provider_id === PUBLIC_DEMO.providerId && q.descriptor_hash === profile.descriptorHash && q.offer_hash === profile.offerHash && q.workload_kind === 'compute.execution.v1' && exactGrant(q.capabilities_granted, profile), 'The quote grants different work or capabilities.');
  need(t?.method === 'none' && t.base_fee_msat === 0 && t.success_fee_msat === 0, 'This service accepts free work only.');
  need(l && Number.isSafeInteger(l.max_runtime_ms) && l.max_runtime_ms > 0 && l.max_runtime_ms <= PUBLIC_DEMO.maxRuntimeMs && Number.isSafeInteger(l.max_memory_bytes) && l.max_memory_bytes > 0 && l.max_memory_bytes <= PUBLIC_DEMO.maxMemoryBytes && Number.isSafeInteger(l.fuel_limit) && l.fuel_limit > 0 && l.fuel_limit <= PUBLIC_DEMO.maxFuel && Number.isSafeInteger(l.max_input_bytes) && l.max_input_bytes > 0 && l.max_input_bytes <= PUBLIC_MARKETPLACE_READ.maxRequestBytes && Number.isSafeInteger(l.max_output_bytes) && l.max_output_bytes > 0 && l.max_output_bytes <= PUBLIC_MARKETPLACE_READ.maxResponseBytes, 'The quote exceeds the published tool bounds.');
}

function publishedRevision(revision: any, profile: PublishedServiceProfile) {
  const p = profile;
  need(revision?.revision_hash === p.revisionHash && revision.payload?.provider_id === PUBLIC_DEMO.providerId && revision.payload.service_id === p.serviceId && revision.payload.offer_id === p.offerId && revision.payload.offer_hash === p.offerHash && revision.payload.binding_hash === p.bindingHash && revision.payload.runtime === 'wasm' && revision.payload.package_kind === 'inline_module' && revision.payload.service?.entrypoint === p.entrypoint && revision.payload.service.entrypoint_kind === 'module' && revision.payload.service.contract_version === 'froglet.wasm.host_json.v1' && exactGrant(revision.payload.service.capabilities, p), 'The publication revision does not match this tool.');
}

/** Call only one of the site's three explicitly enabled, pinned HTTP-operation publications. */
export async function preparePublishedServiceRun(deps: LiveDeps, serviceId: string, inputText: string): Promise<LiveRun> {
  const profiles = publishedServiceProfiles({ providerId: deps.providerId, publishedServices: { enabled: true, profiles: deps.publishedServices } });
  const profile = profiles.find(p => p.serviceId === serviceId);
  need(profile && deps.verifyPublication, 'This published tool is not enabled on this site.');
  const p = profile!, input = parsedInput(inputText);
  need(publishedInput(p.serviceId, input) && new TextEncoder().encode(deps.kernel.canonicalize(input)).length <= PUBLIC_MARKETPLACE_READ.maxRequestBytes, 'Enter a supported bounded public catalog query.');
  deps.onStep?.('discover');
  const d = await deps.transport({ method: 'GET', path: '/v1/provider/descriptor' });
  need(d.status === 200, reason(d)); const descriptor = d.body as SignedArtifact; document(deps, descriptor, 'descriptor');
  need(descriptor.hash === p.descriptorHash && descriptor.payload.provider_id === PUBLIC_DEMO.providerId && currentDocument(descriptor), 'The provider description does not match this current published tool.');
  const offers = await deps.transport({ method: 'GET', path: '/v1/provider/offers' }); need(offers.status === 200, reason(offers));
  const offer = (offers.body as { offers: SignedArtifact[] }).offers?.find(o => o.payload.offer_id === p.offerId);
  need(offer, 'The selected tool is not published.'); document(deps, offer!, 'offer');
  need(offer!.hash === p.offerHash && offer!.payload.provider_id === PUBLIC_DEMO.providerId && offer!.payload.descriptor_hash === p.descriptorHash && offer!.payload.offer_kind === 'compute.execution.v1' && currentDocument(offer!) && offer!.payload.settlement_method === 'none' && offer!.payload.price_schedule?.base_fee_msat === 0 && offer!.payload.price_schedule.success_fee_msat === 0, 'The offer does not match the current pinned free tool.');
  const answer = await deps.transport({ method: 'GET', path: '/v1/provider/services/' + p.serviceId }); need(answer.status === 200, reason(answer));
  const { service, publication_revision: revision } = answer.body as { service: ServiceRecord; publication_revision: any };
  need(service?.provider_id === PUBLIC_DEMO.providerId && service.service_id === p.serviceId && service.offer_id === p.offerId && service.offer_kind === 'compute.execution.v1' && service.runtime === 'wasm' && service.package_kind === 'inline_module' && service.entrypoint_kind === 'module' && service.entrypoint === p.entrypoint && service.contract_version === 'froglet.wasm.host_json.v1' && service.mode === 'sync' && service.publication_state === 'active' && service.module_hash === p.moduleHash && service.binding_hash === p.bindingHash && exactGrant(service.capabilities, p) && (!service.mounts || service.mounts.length === 0) && service.settlement_method === 'none' && service.base_fee_msat === 0 && service.success_fee_msat === 0, 'The native service metadata does not match the approved tool.');
  publishedRevision(revision, p);
  need((await deps.verifyPublication!(revision, offer, descriptor)).valid, 'The signed publication revision does not verify.');
  const execution = executionFor(service, input, deps.kernel, service.capabilities!);
  const requester = deps.kernel.newIdentity(); deps.onStep?.('quote');
  const quoted = await deps.transport({ method: 'POST', path: '/v1/provider/quotes', body: { offer_id: p.offerId, requester_id: requester.public_key, kind: 'execution', execution, max_price_sats: 0 } }); need(quoted.status === 201, reason(quoted));
  const quote = quoted.body as SignedArtifact; document(deps, quote, 'quote'); publishedQuote(quote, p);
  need(quote.payload.requester_id === requester.public_key && quote.payload.workload_hash === deps.kernel.hashJson(execution), 'The quote is for different input or another requester.');
  const now = Math.floor(Date.now() / 1000); need(Number.isSafeInteger(quote.payload.expires_at) && quote.payload.expires_at >= now, 'The quote has expired; no job was submitted.');
  const deal = deps.kernel.sign(requester.seed_hex, 'deal', now, dealPayloadFor(quote, requester.public_key, randomHex(32)));
  const run: LiveRun = { schema: 'froglet.public-demo.run.v1', providerId: deps.providerId, input, request: { kind: 'execution', execution, quote, deal, idempotency_key: randomHex(16) }, artifacts: { descriptor, offer: offer!, quote, deal }, publishedService: { profile: p, revision } };
  deps.save?.(run); return run;
}

function validateSavedRun(deps: LiveDeps, run: LiveRun) {
  need(run.schema === 'froglet.public-demo.run.v1' && run.providerId === deps.providerId, 'The saved job belongs to a different provider.');
  for (const [kind, value] of Object.entries(run.artifacts)) document(deps, value, kind);
  const { descriptor, offer, quote, deal } = run.artifacts;
  const q = quote.payload;
  if (run.publishedService) {
    const p = run.publishedService.profile, e = run.request.execution;
    need(run.providerId === PUBLIC_DEMO.providerId && validProfile(p) && e?.runtime === 'wasm' && e.package_kind === 'inline_module' && e.contract_version === 'froglet.wasm.host_json.v1' && e.entrypoint?.kind === 'module' && e.entrypoint.value === p.entrypoint && e.module_hash === p.bindingHash && e.security?.mode === 'standard' && e.security.service_id === p.serviceId && exactGrant(e.requested_access, p), 'The saved published tool authority has changed.');
    publishedRevision(run.publishedService.revision, p);
    publishedQuote(quote, p);
    if (!run.dealId) need(deps.publishedServices?.some(current => sameProfile(current, p)), 'This unsubmitted tool is no longer enabled.');
  }
  need(descriptor.payload.provider_id === deps.providerId && offer.payload.provider_id === deps.providerId && q.provider_id === deps.providerId && deal.payload.provider_id === deps.providerId, 'The saved provider binding is invalid.');
  need(q.descriptor_hash === descriptor.hash && q.offer_hash === offer.hash && q.workload_hash === workload(run, deps.kernel) && deal.payload.workload_hash === q.workload_hash && deal.payload.quote_hash === quote.hash && deal.payload.requester_id === q.requester_id, 'The saved commitments do not match.');
  need(deps.kernel.canonicalize(run.request.quote) === deps.kernel.canonicalize(quote) && deps.kernel.canonicalize(run.request.deal) === deps.kernel.canonicalize(deal), 'The saved submission does not match its evidence.');
  need(/^[a-f0-9]{32}$/.test(run.request.idempotency_key), 'The saved retry key is invalid.');
}

function finished(deps: LiveDeps, run: LiveRun, record: DealRecord): LiveResult {
  const { quote, deal } = run.artifacts;
  need(record.deal?.hash === deal.hash && record.quote?.hash === quote.hash, 'The returned job is not the signed job.');
  need(record.status === 'succeeded' || record.status === 'failed', 'The provider refused this job.');
  const receipt = record.receipt;
  need(receipt, 'The result has no signed receipt.');
  document(deps, receipt!, 'receipt');
  const r = receipt!.payload;
  need(r.provider_id === deps.providerId && r.requester_id === deal.payload.requester_id && r.deal_hash === deal.hash && r.quote_hash === quote.hash && r.deal_state === record.status, 'The receipt belongs to different work.');
  if (record.status === 'succeeded') need(record.result !== undefined && record.result_hash === deps.kernel.hashJson(record.result) && r.result_hash === record.result_hash, 'The result does not match the signed receipt.');
  if (run.request.kind === 'wasm') need(r.executor?.module_hash === run.request.submission.workload.module_hash, 'The receipt names a different program.');
  if (run.publishedService) need(r.executor?.runtime === 'wasm' && r.executor.abi_version === 'froglet.wasm.host_json.v1' && r.executor.module_hash === run.publishedService.profile.moduleHash && exactGrant(r.executor.capabilities_granted, run.publishedService.profile), 'The receipt names different code or execution capabilities.');
  deps.onStep?.('verify');
  const artifacts = { ...run.artifacts, receipt: receipt! };
  const chain = deps.verifier.validateChain([artifacts.descriptor, artifacts.offer, artifacts.quote, artifacts.deal, artifacts.receipt]);
  need(chain.valid && chain.chain_evaluated, 'The signed exchange does not verify.');
  return { terminal: true, status: record.status, ...(record.status === 'succeeded' ? { result: record.result } : { failure: String(r.failure_code ?? 'execution_failed') }), artifacts, chain, run };
}

/** Retries use the saved signed body and key. A known accepted job is read, never submitted again. */
export async function resumeLiveRun(deps: LiveDeps, run: LiveRun, maxPolls = 16): Promise<LiveResult> {
  validateSavedRun(deps, run);
  if (run.publishedService) need(deps.verifyPublication && (await deps.verifyPublication(run.publishedService.revision, run.artifacts.offer, run.artifacts.descriptor)).valid, 'The saved publication revision does not verify.');
  const sleep = deps.sleep ?? (ms => new Promise(resolve => setTimeout(resolve, ms)));
  try {
    let record: DealRecord;
    if (run.dealId) {
      deps.onStep?.('result');
      const response = await deps.transport({ method: 'GET', path: `/v1/provider/deals/${encodeURIComponent(run.dealId)}` });
      if (response.status !== 200) return { terminal: false, reason: reason(response), run };
      record = response.body as DealRecord;
    } else {
      deps.onStep?.('deal');
      const response = await deps.transport({ method: 'POST', path: '/v1/provider/deals', body: run.request });
      if (response.status !== 200 && response.status !== 202) return { terminal: false, reason: reason(response), run };
      record = response.body as DealRecord;
      need(record.deal?.hash === run.artifacts.deal.hash && record.quote?.hash === run.artifacts.quote.hash && typeof record.deal_id === 'string' && /^[a-zA-Z0-9_-]{1,128}$/.test(record.deal_id), 'The provider returned a different job.');
      run.dealId = record.deal_id;
      deps.save?.(run);
    }
    deps.onStep?.('result');
    for (let attempt = 0; !isFinished(record); attempt++) {
      if (attempt >= maxPolls) return { terminal: false, reason: 'The result is not available yet. Retry this same job.', run };
      await sleep(750);
      const response = await deps.transport({ method: 'GET', path: `/v1/provider/deals/${encodeURIComponent(run.dealId!)}` });
      if (response.status !== 200) return { terminal: false, reason: reason(response), run };
      record = response.body as DealRecord;
      need(record.deal_id === run.dealId, 'The provider returned a different job.');
    }
    return finished(deps, run, record);
  } catch (error) {
    return { terminal: false, reason: error instanceof Error ? error.message : 'The provider did not answer. Retry this same job.', run };
  }
}
