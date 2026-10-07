import { PUBLIC_DEMO, PUBLIC_DEMO_PREFIX, PUBLIC_MARKETPLACE_READ } from './public-demo-config';

export interface PublicDemoEnv {
  FROGLET_PUBLIC_DEMO_ENABLED?: string;
  FROGLET_PUBLIC_DEMO_PROVIDER_ID?: string;
  FROGLET_PUBLIC_DEMO_ORIGIN?: string;
  FROGLET_PUBLIC_MARKETPLACE_READ_ENABLED?: string;
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

// Cancellation is attempted but its acknowledgement never extends a read's
// deadline or delays a known 404/error. The classic C7 path retains its wait.
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

function allowedPost(path: string, body: unknown, providerId: string): boolean {
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
  if (path === '/config' && request.method === 'GET') return new Response(JSON.stringify({ providerId, providerOrigin, limits: PUBLIC_DEMO }), { headers });
  const get = request.method === 'GET' && (['/health', '/demo/status', '/v1/provider/descriptor', '/v1/provider/offers', `/v1/provider/services/${PUBLIC_DEMO.catalogService}`].includes(path) || /^\/v1\/provider\/deals\/[a-zA-Z0-9_-]{1,128}$/.test(path));
  const post = request.method === 'POST' && ['/v1/provider/quotes', '/v1/provider/deals'].includes(path);
  if (!get && !post) return reply(404, 'invalid_route', 'This operation is not part of the public demo.');
  const origin = request.headers.get('origin');
  if (origin && origin !== url.origin) return reply(403, 'origin_refused', 'Use the demo on this site.');
  let admitted: boolean;
  try { admitted = (await env.PUBLIC_DEMO_RATE_LIMITER.limit({ key: 'froglet-public-demo:' + (request.headers.get('cf-connecting-ip') ?? 'unknown') })).success; }
  catch { return reply(503, 'demo_not_active', 'The demo admission control is unavailable.'); }
  if (!admitted) return new Response(JSON.stringify({ code: 'demo_busy', error: 'Please wait before making another request.' }), { status: 429, headers: { ...headers, 'retry-after': '10' } });
  let text: string | undefined;
  if (post) {
    if (request.headers.get('content-type')?.split(';')[0].trim().toLowerCase() !== 'application/json') return reply(415, 'invalid_input', 'Send JSON.');
    try { text = await boundedText(request.body, MAX_REQUEST_BYTES); }
    catch { return reply(413, 'invalid_input', 'The request is too large or invalid.'); }
    let body: unknown;
    try { body = JSON.parse(text); } catch { return reply(400, 'invalid_input', 'The request is not valid JSON.'); }
    if (!allowedPost(path, body, providerId)) return reply(400, 'invalid_input', 'The request does not match a free bounded demo operation.');
  }
  const control = new AbortController();
  const timer = setTimeout(() => control.abort(), 15000);
  try {
    const upstream = await fetch(new Request(providerOrigin + path, { method: request.method, headers: { accept: 'application/json', ...(post ? { 'content-type': 'application/json' } : {}) }, ...(text === undefined ? {} : { body: text }), redirect: 'manual', signal: control.signal }));
    if (upstream.status >= 300 && upstream.status < 400) { await upstream.body?.cancel(); return reply(502, 'provider_unavailable', 'The provider returned an unexpected redirect.'); }
    const body = await boundedText(upstream.body, MAX_RESPONSE_BYTES);
    try { JSON.parse(body); } catch { return reply(502, 'provider_unavailable', 'The provider returned an invalid response.'); }
    return new Response(body, { status: upstream.status, headers });
  } catch { return reply(502, 'provider_unavailable', 'The provider did not answer. Retry the same job rather than submitting another.'); }
  finally { clearTimeout(timer); }
}
