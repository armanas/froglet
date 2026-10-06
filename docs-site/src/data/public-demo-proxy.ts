import { PUBLIC_DEMO, PUBLIC_DEMO_PREFIX } from './public-demo-config';

export interface PublicDemoEnv {
  FROGLET_PUBLIC_DEMO_ENABLED?: string;
  FROGLET_PUBLIC_DEMO_PROVIDER_ID?: string;
  FROGLET_PUBLIC_DEMO_ORIGIN?: string;
  PUBLIC_DEMO_RATE_LIMITER?: { limit(options: { key: string }): Promise<{ success: boolean }> };
}

const MAX_REQUEST_BYTES = 786432;
const MAX_RESPONSE_BYTES = 1048576;
const headers = { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'no-store', 'x-content-type-options': 'nosniff' };
const reply = (status: number, code: string, error: string) => new Response(JSON.stringify({ code, error }), { status, headers });
const hex = (s: unknown) => typeof s === 'string' && /^[a-f0-9]{64}$/.test(s);
const record = (x: unknown): x is Record<string, any> => x !== null && typeof x === 'object' && !Array.isArray(x);
const keys = (x: Record<string, any>, names: string[]) => Object.keys(x).every(k => names.includes(k));

async function boundedText(body: ReadableStream<Uint8Array> | null, ceiling: number): Promise<string> {
  if (!body) return '';
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > ceiling) throw new RangeError('body ceiling');
      chunks.push(value);
    }
  } catch (error) {
    await reader.cancel().catch(() => {});
    throw error;
  } finally { reader.releaseLock(); }
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
