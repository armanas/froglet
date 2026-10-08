import { PUBLIC_DEMO, PUBLIC_DEMO_PREFIX, PUBLIC_MARKETPLACE_READ, publishedServiceProfiles, type PublishedServiceProfile } from './public-demo-config';

export interface PublicDemoEnv {
  FROGLET_PUBLIC_DEMO_ENABLED?: string;
  FROGLET_PUBLIC_DEMO_PROVIDER_ID?: string;
  FROGLET_PUBLIC_DEMO_ORIGIN?: string;
  FROGLET_PUBLIC_MARKETPLACE_READ_ENABLED?: string;
  FROGLET_PUBLIC_DEMO_PUBLISHED_SERVICES_ENABLED?: string;
  FROGLET_PUBLIC_DEMO_PUBLISHED_SERVICES_JSON?: string;
  PUBLIC_DEMO_RATE_LIMITER?: { limit(options: { key: string }): Promise<{ success: boolean }> };
}

type ReadOperation = keyof typeof PUBLIC_MARKETPLACE_READ.operations;
const nonnegativeInteger = (value: unknown): value is number => Number.isSafeInteger(value) && (value as number) >= 0;
const shortString = (value: unknown, max: number): value is string => typeof value === 'string' && value.length <= max;
const stringArray = (value: unknown, count: number, length: number): value is string[] => Array.isArray(value) && value.length <= count && value.every(x => shortString(x, length));
const requireValue = (ok: unknown) => { if (!ok) throw new Error('invalid public read response'); };

function readTarget(operation: ReadOperation, input: unknown): URL | undefined {
  if (!record(input)) return undefined;
  const allowed = operation === 'provider' ? ['provider_id'] : operation === 'search'
    ? ['provider_id', 'offer_kind', 'runtime', 'availability', 'limit', 'offset']
    : ['provider_id', 'limit', 'offset', 'updated_since'];
  if (!keys(input, allowed)) return undefined;
  if ((operation !== 'search' && !hex(input.provider_id)) || (input.provider_id !== undefined && !hex(input.provider_id))) return undefined;
  if (operation === 'provider') return new URL('/v1/providers/' + input.provider_id, PUBLIC_MARKETPLACE_READ.origin);
  const limit = input.limit === undefined ? PUBLIC_MARKETPLACE_READ.defaultPageSize : input.limit;
  const offset = input.offset === undefined ? 0 : input.offset;
  if (!nonnegativeInteger(limit) || limit < 1 || limit > PUBLIC_MARKETPLACE_READ.maxPageSize || !nonnegativeInteger(offset) || offset > PUBLIC_MARKETPLACE_READ.maxOffset) return undefined;
  if (input.updated_since !== undefined && !nonnegativeInteger(input.updated_since)) return undefined;
  if (input.offer_kind !== undefined && (typeof input.offer_kind !== 'string' || !/^[a-z0-9][a-z0-9._-]{0,63}$/.test(input.offer_kind))) return undefined;
  if (input.runtime !== undefined && !['builtin', 'wasm', 'any'].includes(input.runtime)) return undefined;
  if (input.availability !== undefined && !['discoverable', 'healthy', 'offline', 'unknown', 'all'].includes(input.availability)) return undefined;
  const target = new URL(operation === 'search' ? '/v1/offers' : '/v1/providers/' + input.provider_id + '/receipts', PUBLIC_MARKETPLACE_READ.origin);
  target.searchParams.set('limit', String(limit)); target.searchParams.set('offset', String(offset));
  for (const field of ['provider_id', 'offer_kind', 'runtime', 'availability', 'updated_since']) {
    if (input[field] !== undefined && (operation === 'search' || field === 'updated_since')) target.searchParams.set(field, String(input[field]));
  }
  return target;
}

function providerResult(body: unknown, expected: string) {
  requireValue(record(body) && body.provider_id === expected && hex(body.current_descriptor_hash) && record(body.descriptor) && record(body.trust));
  const b = body as Record<string, any>, d = b.descriptor, t = b.trust;
  requireValue(shortString(d.protocol_version, 64) && stringArray(d.service_kinds, 64, 128) && stringArray(d.execution_runtimes, 32, 64));
  requireValue(Array.isArray(d.transport_endpoints) && d.transport_endpoints.length <= 16 && d.transport_endpoints.every((e: unknown) => record(e) && shortString(e.transport, 64) && shortString(e.uri, 2048)));
  requireValue(nonnegativeInteger(t.success_count) && nonnegativeInteger(t.failure_count));
  return { provider: { provider_id: b.provider_id, current_descriptor_hash: b.current_descriptor_hash, protocol_version: d.protocol_version, service_kinds: d.service_kinds, execution_runtimes: d.execution_runtimes, transport_endpoints: d.transport_endpoints.map((e: Record<string, string>) => ({ transport: e.transport, uri: e.uri })), success_count: t.success_count, failure_count: t.failure_count } };
}

function pageResult(operation: 'search' | 'receipts', body: unknown, input: Record<string, any>, target: URL) {
  requireValue(record(body) && Array.isArray(body.items) && record(body.pagination));
  const b = body as Record<string, any>, p = b.pagination;
  const limit = Number(target.searchParams.get('limit')), offset = Number(target.searchParams.get('offset'));
  requireValue(p.limit === limit && p.offset === offset && nonnegativeInteger(p.total) && b.items.length <= limit && b.items.length <= Math.max(0, p.total - offset));
  const items = b.items.map((item: unknown) => {
    requireValue(record(item) && hex(item.artifact_hash) && hex(item.provider_id) && (input.provider_id === undefined || item.provider_id === input.provider_id));
    const row = item as Record<string, any>;
    if (operation === 'receipts') {
      requireValue(hex(row.deal_hash) && shortString(row.deal_state, 64) && shortString(row.execution_state, 64) && shortString(row.runtime, 64) && nonnegativeInteger(row.finished_at));
      requireValue(input.updated_since === undefined || row.finished_at >= input.updated_since);
      return { artifact_hash: row.artifact_hash, provider_id: row.provider_id, deal_hash: row.deal_hash, status: row.deal_state, execution_state: row.execution_state, runtime: row.runtime, finished_at: row.finished_at };
    }
    requireValue(shortString(row.offer_id, 128) && shortString(row.offer_kind, 128) && shortString(row.runtime, 64) && shortString(row.package_kind, 64) && shortString(row.contract_version, 128) && shortString(row.settlement_method, 128));
    requireValue(nonnegativeInteger(row.base_fee_msat) && nonnegativeInteger(row.success_fee_msat) && (input.offer_kind === undefined || row.offer_kind === input.offer_kind) && (input.runtime === undefined || row.runtime === input.runtime));
    const a = row.availability;
    requireValue(record(a) && ['unknown', 'healthy', 'offline'].includes(a.status) && ['unknown', 'invitation_required', 'execution_checked'].includes(a.admission));
    requireValue((a.lease_expires_at === null || nonnegativeInteger(a.lease_expires_at)) && (a.last_renewed_at === null || nonnegativeInteger(a.last_renewed_at)));
    return { artifact_hash: row.artifact_hash, provider_id: row.provider_id, offer_id: row.offer_id, offer_kind: row.offer_kind, runtime: row.runtime, package_kind: row.package_kind, contract_version: row.contract_version, settlement_method: row.settlement_method, base_fee_msat: row.base_fee_msat, success_fee_msat: row.success_fee_msat, availability: { status: a.status, admission: a.admission, lease_expires_at: a.lease_expires_at, last_renewed_at: a.last_renewed_at } };
  });
  return { items, pagination: { limit, offset, total: p.total } };
}

/** Fixed, bounded reads for approved HTTP-operation publications; no node execution or credentials. */
export async function publicMarketplaceReadResponse(request: Request, env: PublicDemoEnv): Promise<Response | null> {
  const url = new URL(request.url), prefix = PUBLIC_MARKETPLACE_READ.prefix;
  if (url.pathname !== prefix && !url.pathname.startsWith(prefix + '/')) return null;
  if (url.search || url.pathname.includes('%') || url.pathname.includes('..')) return reply(400, 'invalid_route', 'Invalid catalog read route.');
  const operation = url.pathname.slice(prefix.length + 1);
  if (!Object.hasOwn(PUBLIC_MARKETPLACE_READ.operations, operation)) return reply(404, 'invalid_route', 'Unknown catalog read operation.');
  if (env.FROGLET_PUBLIC_MARKETPLACE_READ_ENABLED !== 'true' || !env.PUBLIC_DEMO_RATE_LIMITER) return reply(503, 'read_not_active', 'The public catalog read adapters are not active.');
  if (request.method !== 'POST') return new Response(JSON.stringify({ code: 'method_not_allowed', error: 'Send a JSON POST request.' }), { status: 405, headers: { ...headers, allow: 'POST' } });
  const origin = request.headers.get('origin');
  if (origin && origin !== url.origin) return reply(403, 'origin_refused', 'Use the catalog reads on this site.');
  if (request.headers.get('content-type')?.split(';')[0].trim().toLowerCase() !== 'application/json') return reply(415, 'invalid_input', 'Send JSON.');
  const control = new AbortController(), started = performance.now();
  let phase: 'admission' | 'request' | 'upstream' = 'admission';
  const expired = () => control.signal.aborted || performance.now() - started >= PUBLIC_MARKETPLACE_READ.timeoutMs;
  const timeoutReply = () => phase === 'admission' ? reply(503, 'read_not_active', 'The catalog admission control is unavailable.')
    : phase === 'request' ? reply(408, 'read_timeout', 'The catalog read input timed out.')
      : reply(502, 'catalog_unavailable', 'The public catalog returned no valid bounded read.');
  let timer: ReturnType<typeof setTimeout>;
  const deadline = new Promise<Response>(resolve => {
    timer = setTimeout(() => { control.abort(); cancelReadBody(request.body); resolve(timeoutReply()); }, PUBLIC_MARKETPLACE_READ.timeoutMs);
  });
  const work = async () => {
    try {
      let admitted: boolean;
      try { admitted = (await env.PUBLIC_DEMO_RATE_LIMITER!.limit({ key: 'froglet-public-demo:' + (request.headers.get('cf-connecting-ip') ?? 'unknown') })).success; }
      catch { return reply(503, 'read_not_active', 'The catalog admission control is unavailable.'); }
      // An admission result arriving after the response deadline cannot start
      // an API read. Aborting fetch alone cannot enforce this boundary.
      if (expired()) return timeoutReply();
      if (!admitted) return new Response(JSON.stringify({ code: 'catalog_busy', error: 'Please wait before making another request.' }), { status: 429, headers: { ...headers, 'retry-after': '10' } });
      phase = 'request';
      let input: unknown;
      try { input = JSON.parse(await boundedText(request.body, PUBLIC_MARKETPLACE_READ.maxRequestBytes, control.signal)); }
      catch (error) {
        if (expired()) return timeoutReply();
        return reply(error instanceof RangeError ? 413 : 400, 'invalid_input', 'The catalog read input is invalid or too large.');
      }
      if (expired()) return timeoutReply();
      const target = readTarget(operation as ReadOperation, input);
      if (!target) return reply(400, 'invalid_input', 'The input does not match the declared catalog read operation.');
      phase = 'upstream';
      const upstream = await fetch(new Request(target, { method: 'GET', headers: { accept: 'application/json' }, redirect: 'manual', signal: control.signal }));
      if (expired()) { cancelReadBody(upstream.body); return timeoutReply(); }
      if (operation === 'provider' && upstream.status === 404) { cancelReadBody(upstream.body); return new Response('{"provider":null}', { headers }); }
      if (!upstream.ok) {
        cancelReadBody(upstream.body);
        return upstream.status === 429 ? new Response(JSON.stringify({ code: 'catalog_busy', error: 'The public catalog allowance is busy or spent.' }), { status: 429, headers: { ...headers, 'retry-after': '10' } }) : reply(502, 'catalog_unavailable', 'The public catalog did not provide this read.');
      }
      const body: unknown = JSON.parse(await boundedText(upstream.body, PUBLIC_MARKETPLACE_READ.maxResponseBytes, control.signal));
      if (expired()) return timeoutReply();
      const result = operation === 'provider' ? providerResult(body, (input as Record<string, string>).provider_id) : pageResult(operation as 'search' | 'receipts', body, input as Record<string, any>, target);
      const output = JSON.stringify(result);
      if (new TextEncoder().encode(output).length > PUBLIC_MARKETPLACE_READ.maxResponseBytes) throw new RangeError('output ceiling');
      return expired() ? timeoutReply() : new Response(output, { headers });
    } catch { return reply(502, 'catalog_unavailable', 'The public catalog returned no valid bounded read.'); }
  };
  try { return await Promise.race([work(), deadline]); }
  finally { clearTimeout(timer!); }
}

// Cancellation is attempted without extending the read/transport deadline or
// delaying a known redirect/error response.
function cancelReadBody(body: { cancel(): Promise<unknown> } | null | undefined): void {
  try { if (body) void body.cancel().catch(() => {}); } catch { /* Best effort; no credential diagnostics. */ }
}

const MAX_REQUEST_BYTES = 786432;
const MAX_RESPONSE_BYTES = 1048576;
const headers = { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'no-store', 'x-content-type-options': 'nosniff' };
const reply = (status: number, code: string, error: string) => new Response(JSON.stringify({ code, error }), { status, headers });
const hex = (s: unknown) => typeof s === 'string' && /^[a-f0-9]{64}$/.test(s);
const record = (x: unknown): x is Record<string, any> => x !== null && typeof x === 'object' && !Array.isArray(x);
const keys = (x: Record<string, any>, names: string[]) => Object.keys(x).every(k => names.includes(k));

async function boundedText(body: ReadableStream<Uint8Array> | null, ceiling: number, signal?: AbortSignal): Promise<string> {
  if (!body) return '';
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  let abort: (() => void) | undefined;
  const deadline = signal && new Promise<never>((_resolve, reject) => {
    abort = () => reject(new DOMException('read deadline', 'AbortError'));
    if (signal.aborted) abort(); else signal.addEventListener('abort', abort, { once: true });
  });
  try {
    for (;;) {
      const { done, value } = await (deadline ? Promise.race([reader.read(), deadline]) : reader.read());
      if (done) break;
      size += value.byteLength;
      if (size > ceiling) throw new RangeError('body ceiling');
      chunks.push(value);
    }
  } catch (error) {
    if (signal) cancelReadBody(reader); else await reader.cancel().catch(() => {});
    throw error;
  } finally { if (signal && abort) signal.removeEventListener('abort', abort); reader.releaseLock(); }
  const bytes = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) { bytes.set(chunk, at); at += chunk.byteLength; }
  return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
}

function finiteJson(x: unknown, depth = 0): boolean {
  if (depth > 64) return false;
  if (typeof x === 'number') return Number.isFinite(x) && (!Number.isInteger(x) || Number.isSafeInteger(x));
  if (Array.isArray(x)) return x.every(value => finiteJson(value, depth + 1));
  if (record(x)) return Object.values(x).every(value => finiteJson(value, depth + 1));
  return true;
}

const exactKeys = (value: Record<string, any>, names: string[]) => Object.keys(value).length === names.length && keys(value, names);
const operationGrant = (p: PublishedServiceProfile) => 'net.http.operation.' + p.operationHash;
const exactGrant = (grants: unknown, p: PublishedServiceProfile) => Array.isArray(grants) && grants.length === 1 && grants[0] === operationGrant(p);

/** Operator-owned public pins, never a caller-supplied host, program or credential. */
function configuredProfiles(env: PublicDemoEnv, providerId: string): PublishedServiceProfile[] {
  if (env.FROGLET_PUBLIC_DEMO_PUBLISHED_SERVICES_ENABLED !== 'true' || providerId !== PUBLIC_DEMO.providerId) return [];
  const text = env.FROGLET_PUBLIC_DEMO_PUBLISHED_SERVICES_JSON;
  if (typeof text !== 'string' || new TextEncoder().encode(text).length > 8192) return [];
  try {
    const value: unknown = JSON.parse(text);
    if (!record(value) || !exactKeys(value, ['providerId', 'profiles'])) return [];
    const profiles = publishedServiceProfiles({ providerId: value.providerId, publishedServices: { enabled: true, profiles: value.profiles } });
    if (new Set(profiles.map(p => p.offerHash)).size !== 3 || new Set(profiles.map(p => p.revisionHash)).size !== 3) return [];
    return profiles;
  } catch { return []; }
}

function workProfile(body: Record<string, any>, profiles: readonly PublishedServiceProfile[]): PublishedServiceProfile | undefined {
  return body.kind === 'execution' && record(body.execution?.security) ? profiles.find(p => p.serviceId === body.execution.security.service_id) : undefined;
}

function namedWork(body: Record<string, any>, profile: PublishedServiceProfile): boolean {
  const e = body.execution;
  return body.kind === 'execution' && record(e) && exactKeys(e, ['schema_version', 'workload_kind', 'runtime', 'package_kind', 'entrypoint', 'contract_version', 'input_format', 'input_hash', 'security', 'input', 'module_hash', 'requested_access']) &&
    e.schema_version === 'froglet/v1' && e.workload_kind === 'compute.execution.v1' && e.runtime === 'wasm' && e.package_kind === 'inline_module' &&
    e.contract_version === 'froglet.wasm.host_json.v1' && e.input_format === 'application/json+jcs' && hex(e.input_hash) && e.module_hash === profile.bindingHash &&
    record(e.entrypoint) && exactKeys(e.entrypoint, ['kind', 'value']) && e.entrypoint.kind === 'module' && e.entrypoint.value === profile.entrypoint &&
    record(e.security) && exactKeys(e.security, ['mode', 'service_id']) && e.security.mode === 'standard' && e.security.service_id === profile.serviceId &&
    exactGrant(e.requested_access, profile) && new TextEncoder().encode(JSON.stringify(e.input)).length <= PUBLIC_MARKETPLACE_READ.maxRequestBytes &&
    Boolean(readTarget(profile.serviceId.slice('marketplace-'.length) as ReadOperation, e.input));
}

function namedLimits(limits: unknown): boolean {
  return record(limits) && exactKeys(limits, ['max_runtime_ms', 'max_memory_bytes', 'fuel_limit', 'max_input_bytes', 'max_output_bytes']) &&
    Number.isSafeInteger(limits.max_runtime_ms) && limits.max_runtime_ms > 0 && limits.max_runtime_ms <= PUBLIC_DEMO.maxRuntimeMs &&
    Number.isSafeInteger(limits.max_memory_bytes) && limits.max_memory_bytes > 0 && limits.max_memory_bytes <= PUBLIC_DEMO.maxMemoryBytes &&
    Number.isSafeInteger(limits.fuel_limit) && limits.fuel_limit > 0 && limits.fuel_limit <= PUBLIC_DEMO.maxFuel &&
    Number.isSafeInteger(limits.max_input_bytes) && limits.max_input_bytes > 0 && limits.max_input_bytes <= PUBLIC_MARKETPLACE_READ.maxRequestBytes &&
    Number.isSafeInteger(limits.max_output_bytes) && limits.max_output_bytes > 0 && limits.max_output_bytes <= PUBLIC_MARKETPLACE_READ.maxResponseBytes;
}

// These are transport policy checks, not cryptographic verification. Native
// admission checks signatures and actual commitments; the browser checks its
// received evidence independently with the Rust verifier.
function envelopeShape(value: unknown, kind: 'quote' | 'deal'): value is Record<string, any> {
  return record(value) && exactKeys(value, ['schema_version', 'artifact_type', 'created_at', 'hash', 'payload_hash', 'payload', 'signer', 'signature']) &&
    value.schema_version === 'froglet/v1' && value.artifact_type === kind && nonnegativeInteger(value.created_at) && hex(value.hash) && hex(value.payload_hash) && hex(value.signer) &&
    typeof value.signature === 'string' && /^[a-f0-9]{128}$/.test(value.signature) && record(value.payload);
}

function namedQuoteClaims(quote: unknown, profile: PublishedServiceProfile): quote is Record<string, any> {
  if (!envelopeShape(quote, 'quote')) return false;
  const q = quote.payload, t = q.settlement_terms;
  return exactKeys(q, ['provider_id', 'requester_id', 'descriptor_hash', 'offer_hash', 'expires_at', 'workload_kind', 'workload_hash', 'capabilities_granted', 'settlement_terms', 'execution_limits']) &&
    quote.signer === PUBLIC_DEMO.providerId && q.provider_id === PUBLIC_DEMO.providerId && hex(q.requester_id) && q.descriptor_hash === profile.descriptorHash && q.offer_hash === profile.offerHash &&
    nonnegativeInteger(q.expires_at) && q.workload_kind === 'compute.execution.v1' && hex(q.workload_hash) && exactGrant(q.capabilities_granted, profile) && namedLimits(q.execution_limits) &&
    record(t) && exactKeys(t, ['method', 'destination_identity', 'base_fee_msat', 'success_fee_msat', 'max_base_invoice_expiry_secs', 'max_success_hold_expiry_secs', 'min_final_cltv_expiry']) &&
    t.method === 'none' && t.destination_identity === '' && t.base_fee_msat === 0 && t.success_fee_msat === 0 && t.max_base_invoice_expiry_secs === 0 && t.max_success_hold_expiry_secs === 0 && t.min_final_cltv_expiry === 0;
}

function namedPost(path: string, body: Record<string, any>, profile: PublishedServiceProfile): boolean {
  if (!finiteJson(body) || !namedWork(body, profile)) return false;
  if (path === '/v1/provider/quotes') return exactKeys(body, ['offer_id', 'requester_id', 'kind', 'execution', 'max_price_sats']) && body.offer_id === profile.offerId && hex(body.requester_id) && body.max_price_sats === 0;
  if (!exactKeys(body, ['quote', 'deal', 'kind', 'execution', 'idempotency_key']) || typeof body.idempotency_key !== 'string' || !/^[a-zA-Z0-9_.:-]{1,128}$/.test(body.idempotency_key) || !namedQuoteClaims(body.quote, profile) || !envelopeShape(body.deal, 'deal')) return false;
  const q = body.quote, d = body.deal, p = d.payload;
  return exactKeys(p, ['provider_id', 'requester_id', 'quote_hash', 'workload_hash', 'success_payment_hash', 'admission_deadline', 'completion_deadline', 'acceptance_deadline']) &&
    p.provider_id === PUBLIC_DEMO.providerId && p.requester_id === q.payload.requester_id && d.signer === p.requester_id && p.quote_hash === q.hash && p.workload_hash === q.payload.workload_hash && hex(p.success_payment_hash) &&
    nonnegativeInteger(p.admission_deadline) && nonnegativeInteger(p.completion_deadline) && nonnegativeInteger(p.acceptance_deadline) && p.admission_deadline <= q.payload.expires_at && p.admission_deadline <= p.completion_deadline && p.completion_deadline <= p.acceptance_deadline &&
    new TextEncoder().encode(JSON.stringify(body.execution.input)).length <= q.payload.execution_limits.max_input_bytes;
}

function sameJson(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (Array.isArray(a) && Array.isArray(b)) return a.length === b.length && a.every((value, i) => sameJson(value, b[i]));
  if (record(a) && record(b)) return Object.keys(a).length === Object.keys(b).length && Object.keys(a).every(key => Object.hasOwn(b, key) && sameJson(a[key], b[key]));
  return false;
}

const publicBuildText = (value: unknown) => typeof value === 'string' && value.length > 0 && new TextEncoder().encode(value).length <= 128 && !/[\u0000-\u001f\u007f-\u009f]/.test(value);
function publicBuildEvidence(value: unknown, profile: PublishedServiceProfile): boolean {
  if (value === undefined) return true; // Existing readable revisions may predate build evidence.
  if (!record(value) || !keys(value, ['schema_version', 'builder', 'builder_version', 'source_digest', 'artifact_digest', 'dependency_mode', 'components', 'hermetic'])) return false;
  const components = value.components === undefined ? [] : value.components;
  return value.schema_version === 'froglet.publication-build-evidence.v1' && publicBuildText(value.builder) && publicBuildText(value.builder_version) && hex(value.source_digest) && value.artifact_digest === profile.moduleHash &&
    ['none', 'locked', 'unresolved'].includes(value.dependency_mode) && typeof value.hermetic === 'boolean' && !(value.hermetic && value.dependency_mode === 'unresolved') &&
    Array.isArray(components) && components.length <= 128 && (value.dependency_mode === 'locked' ? components.length > 0 : components.length === 0) &&
    components.every(c => record(c) && exactKeys(c, ['role', 'name', 'version', 'digest']) && publicBuildText(c.role) && publicBuildText(c.name) && publicBuildText(c.version) && hex(c.digest));
}

function publicVerificationEvidence(value: unknown): boolean {
  return record(value) && keys(value, ['input_hash', 'result_hash', 'expected_output_matched']) && hex(value.input_hash) && hex(value.result_hash) && (value.expected_output_matched === undefined || value.expected_output_matched === true);
}

function publicStarter(value: unknown, profile: PublishedServiceProfile): boolean {
  if (typeof value !== 'string' || new TextEncoder().encode(value).length > PUBLIC_MARKETPLACE_READ.maxRequestBytes) return false;
  try { return Boolean(readTarget(profile.serviceId.slice('marketplace-'.length) as ReadOperation, JSON.parse(value))); }
  catch { return false; }
}

function namedMetadata(body: unknown, profile: PublishedServiceProfile): boolean {
  if (!record(body) || !keys(body, ['service', 'execution_access', 'publication_revision']) || !finiteJson(body) || !['open', 'trial'].includes(body.execution_access)) return false;
  const s = body.service, r = body.publication_revision, p = r?.payload, i = p?.service, price = p?.price;
  const operation = PUBLIC_MARKETPLACE_READ.operations[profile.serviceId.slice('marketplace-'.length) as ReadOperation];
  return record(s) && keys(s, ['service_id', 'offer_id', 'offer_kind', 'resource_kind', 'project_id', 'summary', 'runtime', 'package_kind', 'entrypoint_kind', 'entrypoint', 'contract_version', 'mounts', 'capabilities', 'mode', 'price_sats', 'base_fee_msat', 'success_fee_msat', 'settlement_method', 'price_currency', 'publication_state', 'provider_id', 'module_hash', 'binding_hash', 'starter', 'input_schema', 'output_schema']) &&
    s.provider_id === PUBLIC_DEMO.providerId && s.service_id === profile.serviceId && s.offer_id === profile.offerId && s.offer_kind === 'compute.execution.v1' && s.resource_kind === 'service' && (s.price_currency === undefined || s.price_currency === 'sat') && s.publication_state === 'active' &&
    s.runtime === 'wasm' && s.package_kind === 'inline_module' && s.entrypoint_kind === 'module' && s.entrypoint === profile.entrypoint && s.contract_version === 'froglet.wasm.host_json.v1' && s.mode === 'sync' &&
    s.module_hash === profile.moduleHash && s.binding_hash === profile.bindingHash && exactGrant(s.capabilities, profile) && (s.mounts === undefined || Array.isArray(s.mounts) && s.mounts.length === 0) &&
    s.price_sats === 0 && s.base_fee_msat === 0 && s.success_fee_msat === 0 && s.settlement_method === 'none' &&
    record(r) && exactKeys(r, ['revision_hash', 'signer_pubkey', 'signature', 'payload']) && r.revision_hash === profile.revisionHash && r.signer_pubkey === PUBLIC_DEMO.providerId && typeof r.signature === 'string' && /^[a-f0-9]{128}$/.test(r.signature) &&
    record(p) && keys(p, ['schema_version', 'provider_id', 'service_id', 'offer_id', 'offer_hash', 'binding_hash', 'package_digest', 'runtime', 'package_kind', 'build_evidence', 'service', 'limits', 'price', 'local_verification']) &&
    p.schema_version === 'froglet.publication-revision.v1' && p.provider_id === PUBLIC_DEMO.providerId && p.service_id === profile.serviceId && p.offer_id === profile.offerId && p.offer_hash === profile.offerHash &&
    p.binding_hash === profile.bindingHash && p.package_digest === profile.moduleHash && p.runtime === 'wasm' && p.package_kind === 'inline_module' &&
    record(i) && keys(i, ['project_id', 'summary', 'starter', 'source_kind', 'entrypoint_kind', 'entrypoint', 'contract_version', 'mode', 'mounts', 'capabilities', 'input_schema', 'output_schema']) &&
    i.entrypoint_kind === 'module' && i.entrypoint === profile.entrypoint && i.contract_version === 'froglet.wasm.host_json.v1' && i.mode === 'sync' && exactGrant(i.capabilities, profile) && (i.mounts === undefined || Array.isArray(i.mounts) && i.mounts.length === 0) && namedLimits(p.limits) &&
    i.source_kind === 'wasm' && (s.project_id === undefined || publicBuildText(s.project_id)) && s.project_id === i.project_id && shortString(s.summary, 8192) && s.summary === i.summary && publicStarter(s.starter, profile) && s.starter === i.starter &&
    sameJson(s.input_schema, operation.inputSchema) && sameJson(i.input_schema, operation.inputSchema) && sameJson(s.output_schema, operation.outputSchema) && sameJson(i.output_schema, operation.outputSchema) && publicBuildEvidence(p.build_evidence, profile) && publicVerificationEvidence(p.local_verification) &&
    record(price) && exactKeys(price, ['settlement_method', 'currency', 'base_amount_minor', 'success_amount_minor', 'offer_settlement_method']) && price.currency === 'sat' && price.settlement_method === 'none' && price.offer_settlement_method === 'none' && price.base_amount_minor === 0 && price.success_amount_minor === 0;
}

function allowedWork(body: Record<string, any>): boolean {
  if (body.kind === 'wasm') {
    const s = body.submission;
    if (!record(s) || !keys(s, ['schema_version', 'submission_type', 'workload', 'module_bytes_hex', 'input']) || s.schema_version !== 'froglet/v1' || s.submission_type !== 'wasm_submission') return false;
    const w = s.workload;
    return record(w) && keys(w, ['schema_version', 'workload_kind', 'abi_version', 'module_format', 'module_hash', 'input_format', 'input_hash', 'requested_capabilities']) &&
      w.schema_version === 'froglet/v1' && w.workload_kind === 'compute.wasm.v1' && w.abi_version === 'froglet.wasm.run_json.v1' &&
      w.module_format === 'application/wasm' && w.input_format === 'application/json+jcs' && hex(w.module_hash) && hex(w.input_hash) &&
      Array.isArray(w.requested_capabilities) && w.requested_capabilities.length === 0 &&
      typeof s.module_bytes_hex === 'string' && /^0061736d01000000(?:[a-f0-9]{2})*$/.test(s.module_bytes_hex) && s.module_bytes_hex.length <= PUBLIC_DEMO.maxModuleBytes * 2 &&
      new TextEncoder().encode(JSON.stringify(s.input)).length <= PUBLIC_DEMO.maxInputBytes;
  }
  if (body.kind === 'execution') {
    const e = body.execution;
    if (!record(e) || !keys(e, ['schema_version', 'workload_kind', 'runtime', 'package_kind', 'entrypoint', 'contract_version', 'input_format', 'input_hash', 'security', 'input', 'module_hash', 'builtin_name'])) return false;
    return e.schema_version === 'froglet/v1' && e.workload_kind === PUBLIC_DEMO.catalogService && e.builtin_name === PUBLIC_DEMO.catalogService && e.runtime === 'builtin' && e.package_kind === 'builtin' &&
      e.contract_version === 'froglet.builtin.data_query.json.v1' && e.input_format === 'application/json+jcs' && hex(e.input_hash) && hex(e.module_hash) &&
      record(e.entrypoint) && keys(e.entrypoint, ['kind', 'value']) && e.entrypoint.kind === 'builtin' && e.entrypoint.value === PUBLIC_DEMO.catalogService &&
      record(e.security) && keys(e.security, ['mode', 'service_id']) && e.security.mode === 'standard' && e.security.service_id === PUBLIC_DEMO.catalogService &&
      new TextEncoder().encode(JSON.stringify(e.input)).length <= PUBLIC_DEMO.maxInputBytes;
  }
  return false;
}

function allowedPost(path: string, body: unknown, providerId: string, profiles: readonly PublishedServiceProfile[] = []): boolean {
  if (record(body)) {
    const profile = workProfile(body, profiles);
    if (profile) return namedPost(path, body, profile);
  }
  if (!record(body) || !finiteJson(body) || !allowedWork(body)) return false;
  const offerId = body.kind === 'wasm' ? PUBLIC_DEMO.computeOffer : PUBLIC_DEMO.catalogService;
  if (path === '/v1/provider/quotes') return keys(body, ['offer_id', 'requester_id', 'kind', 'submission', 'execution', 'max_price_sats']) && body.offer_id === offerId && hex(body.requester_id) && body.max_price_sats === 0;
  if (!keys(body, ['quote', 'deal', 'kind', 'submission', 'execution', 'idempotency_key', 'payment']) || body.payment != null || typeof body.idempotency_key !== 'string' || !/^[a-zA-Z0-9_.:-]{1,128}$/.test(body.idempotency_key)) return false;
  const q = body.quote;
  const d = body.deal;
  if (!record(q) || !record(d) || q.artifact_type !== 'quote' || d.artifact_type !== 'deal' || !record(q.payload) || !record(d.payload)) return false;
  const p = q.payload;
  const limits = p.execution_limits;
  const terms = p.settlement_terms;
  const ioLimit = body.kind === 'wasm' ? PUBLIC_DEMO.maxInputBytes : 1048576;
  return p.provider_id === providerId && p.workload_kind === (body.kind === 'wasm' ? 'compute.wasm.v1' : PUBLIC_DEMO.catalogService) &&
    record(terms) && terms.method === 'none' && terms.base_fee_msat === 0 && terms.success_fee_msat === 0 &&
    (!p.capabilities_granted || Array.isArray(p.capabilities_granted) && p.capabilities_granted.length === 0) && d.payload.provider_id === providerId &&
    record(limits) && Number.isSafeInteger(limits.max_runtime_ms) && limits.max_runtime_ms > 0 && limits.max_runtime_ms <= PUBLIC_DEMO.maxRuntimeMs &&
    Number.isSafeInteger(limits.max_memory_bytes) && limits.max_memory_bytes >= 0 && limits.max_memory_bytes <= PUBLIC_DEMO.maxMemoryBytes &&
    Number.isSafeInteger(limits.fuel_limit) && limits.fuel_limit >= 0 && limits.fuel_limit <= PUBLIC_DEMO.maxFuel &&
    Number.isSafeInteger(limits.max_input_bytes) && limits.max_input_bytes > 0 && limits.max_input_bytes <= ioLimit &&
    Number.isSafeInteger(limits.max_output_bytes) && limits.max_output_bytes > 0 && limits.max_output_bytes <= ioLimit;
}

/** Fixed-provider, credential-free transport. Signed artifacts remain the native node's responsibility. */
export async function publicDemoResponse(request: Request, env: PublicDemoEnv): Promise<Response | null> {
  const url = new URL(request.url);
  if (url.pathname !== PUBLIC_DEMO_PREFIX && !url.pathname.startsWith(PUBLIC_DEMO_PREFIX + '/')) return null;
  if (env.FROGLET_PUBLIC_DEMO_ENABLED !== 'true') return reply(503, 'demo_not_active', 'The hosted demo is not active yet.');
  if (!env.PUBLIC_DEMO_RATE_LIMITER) return reply(503, 'demo_not_active', 'The demo admission control is unavailable.');
  const providerId = env.FROGLET_PUBLIC_DEMO_PROVIDER_ID ?? PUBLIC_DEMO.providerId;
  const providerOrigin = env.FROGLET_PUBLIC_DEMO_ORIGIN ?? PUBLIC_DEMO.origin;
  if (!hex(providerId) || providerOrigin !== PUBLIC_DEMO.origin) return reply(503, 'demo_not_active', 'The demo provider configuration is invalid.');
  if (url.search || url.pathname.includes('%') || url.pathname.includes('..')) return reply(400, 'invalid_route', 'Invalid demo route.');
  const path = url.pathname.slice(PUBLIC_DEMO_PREFIX.length);
  const profiles = configuredProfiles(env, providerId);
  if (path === '/config' && request.method === 'GET') return new Response(JSON.stringify({ providerId, providerOrigin, limits: PUBLIC_DEMO, publishedServices: { enabled: profiles.length === 3, profiles } }), { headers });
  const metadataProfile = profiles.find(p => path === '/v1/provider/services/' + p.serviceId);
  // An active offer may reference an earlier signed descriptor; the ingress serves only current publication records.
  const artifactHash = /^\/v1\/artifacts\/([0-9a-f]{64})$/.exec(path)?.[1];
  const get = request.method === 'GET' && (['/health', '/demo/status', '/v1/provider/descriptor', '/v1/provider/offers', `/v1/provider/services/${PUBLIC_DEMO.catalogService}`].includes(path) || Boolean(metadataProfile) || Boolean(artifactHash) || /^\/v1\/provider\/deals\/[a-zA-Z0-9_-]{1,128}$/.test(path));
  const post = request.method === 'POST' && ['/v1/provider/quotes', '/v1/provider/deals'].includes(path);
  if (!get && !post) return reply(404, 'invalid_route', 'This operation is not part of the public demo.');
  const origin = request.headers.get('origin');
  if (origin && origin !== url.origin) return reply(403, 'origin_refused', 'Use the demo on this site.');
  const control = new AbortController(), started = performance.now();
  let phase: 'admission' | 'request' | 'upstream' = 'admission';
  const expired = () => control.signal.aborted || performance.now() - started >= 15000;
  const timeoutReply = () => phase === 'admission' ? reply(503, 'demo_not_active', 'The demo admission control is unavailable.')
    : phase === 'request' ? reply(408, 'request_timeout', 'The demo request input timed out.')
      : reply(502, 'provider_unavailable', 'The provider did not answer. Retry the same job rather than submitting another.');
  let timer: ReturnType<typeof setTimeout>;
  const deadline = new Promise<Response>(resolve => {
    timer = setTimeout(() => { control.abort(); cancelReadBody(request.body); resolve(timeoutReply()); }, 15000);
  });
  const work = async () => {
    try {
      let admitted: boolean;
      try { admitted = (await env.PUBLIC_DEMO_RATE_LIMITER!.limit({ key: 'froglet-public-demo:' + (request.headers.get('cf-connecting-ip') ?? 'unknown') })).success; }
      catch { return reply(503, 'demo_not_active', 'The demo admission control is unavailable.'); }
      if (expired()) return timeoutReply(); // Late admission must not start a native request.
      if (!admitted) return new Response(JSON.stringify({ code: 'demo_busy', error: 'Please wait before making another request.' }), { status: 429, headers: { ...headers, 'retry-after': '10' } });
      let text: string | undefined;
      let namedBody: Record<string, any> | undefined;
      if (post) {
        phase = 'request';
        if (request.headers.get('content-type')?.split(';')[0].trim().toLowerCase() !== 'application/json') return reply(415, 'invalid_input', 'Send JSON.');
        try { text = await boundedText(request.body, MAX_REQUEST_BYTES, control.signal); }
        catch { return expired() ? timeoutReply() : reply(413, 'invalid_input', 'The request is too large or invalid.'); }
        if (expired()) return timeoutReply();
        let body: unknown;
        try { body = JSON.parse(text); } catch { return reply(400, 'invalid_input', 'The request is not valid JSON.'); }
        if (!allowedPost(path, body, providerId, profiles)) return reply(400, 'invalid_input', 'The request does not match a free bounded demo operation.');
        if (record(body) && workProfile(body, profiles)) namedBody = body;
      }
      if (expired()) return timeoutReply();
      phase = 'upstream';
      const upstream = await fetch(new Request(providerOrigin + path, { method: request.method, headers: { accept: 'application/json', ...(post ? { 'content-type': 'application/json' } : {}) }, ...(text === undefined ? {} : { body: text }), redirect: 'manual', signal: control.signal }));
      if (expired()) { cancelReadBody(upstream.body); return timeoutReply(); }
      if (upstream.status >= 300 && upstream.status < 400) { cancelReadBody(upstream.body); return reply(502, 'provider_unavailable', 'The provider returned an unexpected redirect.'); }
      const body = await boundedText(upstream.body, MAX_RESPONSE_BYTES, control.signal);
      if (expired()) return timeoutReply();
      let result: unknown;
      try { result = JSON.parse(body); } catch { return reply(502, 'provider_unavailable', 'The provider returned an invalid response.'); }
      if (upstream.status === 200 && metadataProfile && !namedMetadata(result, metadataProfile)) return reply(502, 'provider_unavailable', 'The provider description does not match the configured public tool.');
      if (upstream.status === 200 && artifactHash && !(record(result) && result.hash === artifactHash)) return reply(502, 'provider_unavailable', 'The provider returned a different artifact.');
      if (upstream.status === 201 && namedBody && path === '/v1/provider/quotes') {
        const profile = workProfile(namedBody, profiles)!;
        if (!finiteJson(result) || !namedQuoteClaims(result, profile) || result.payload.requester_id !== namedBody.requester_id || result.payload.expires_at < Math.floor(Date.now() / 1000)) return reply(502, 'provider_unavailable', 'The provider quote does not match the configured public tool.');
      }
      return expired() ? timeoutReply() : new Response(body, { status: upstream.status, headers });
    } catch { return timeoutReply(); }
  };
  try { return await Promise.race([work(), deadline]); }
  finally { clearTimeout(timer!); }
}
