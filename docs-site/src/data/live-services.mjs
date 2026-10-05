// Static website view only. No fetch, credential input or service execution.
const HASH = /^[a-f0-9]{64}$/;
const UTC = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?Z$/;
const need = (value, message) => { if (!value) throw new Error(message); };
const hash = value => typeof value === 'string' && HASH.test(value);
const instant = value => typeof value === 'string' && UTC.test(value) && Number.isFinite(Date.parse(value)) && new Date(value).toISOString() === value.replace(/(\.\d{1,3})?Z$/, (_, fraction) => (fraction ?? '.').padEnd(4, '0') + 'Z');
const integer = value => Number.isSafeInteger(value) && value >= 0;
const fields = (value, allowed, label) => need(value && typeof value === 'object' && !Array.isArray(value) && Object.keys(value).every(k => allowed.includes(k)), 'Unexpected ' + label + ' field; keep private records/credentials out of page data');
const json = value => JSON.stringify(value, null, 2);
const quote = value => "'" + String(value).replaceAll("'", "'\"'\"'") + "'";
function https(value, originOnly = false) {
  if (typeof value !== 'string') return false;
  try { const u = new URL(value); return u.protocol === 'https:' && !u.username && !u.password && !u.hash && (!originOnly || (u.pathname === '/' && !u.search)); }
  catch { return false; }
}
function example(value) {
  fields(value, ['input', 'expected_result', 'receipt_evidence_sha256'], 'example');
  need(value && Object.hasOwn(value, 'input') && Object.hasOwn(value, 'expected_result') && hash(value.receipt_evidence_sha256), 'Example needs exact input, expected result and signed-chain evidence');
}
function lookupInput(input) {
  need(input && typeof input === 'object' && !Array.isArray(input), 'Lookup input must be an object');
  const fields = input.op === 'describe' ? ['op'] : ['op', 'collection', 'columns', 'equals', 'limit', 'offset'];
  need(['describe', 'select'].includes(input.op) && Object.keys(input).every(k => fields.includes(k)), 'Only released describe/select input fields are supported');
  if (input.op === 'describe') return;
  need(typeof input.collection === 'string' && input.collection.length > 0, 'Select needs actual published collection');
  if (input.columns !== undefined) need(Array.isArray(input.columns) && input.columns.every(v => typeof v === 'string'), 'Columns must name published fields');
  if (input.limit !== undefined) need(Number.isSafeInteger(input.limit) && input.limit >= 1 && input.limit <= 100, 'Select limit must be 1–100');
  if (input.offset !== undefined) need(integer(input.offset) && input.offset <= 100000, 'Invalid select offset');
  if (input.equals !== undefined) need(input.equals && typeof input.equals === 'object' && !Array.isArray(input.equals) && Object.values(input.equals).every(v => v === null || ['string', 'number', 'boolean'].includes(typeof v)), 'Equals supports scalar comparisons only');
}
export function recordedSessionAccess(expiresAt, now = Date.now()) {
  need(instant(expiresAt) && Number.isFinite(now), 'Session marker needs a valid UTC expiry and device time');
  return now >= Date.parse(expiresAt) ? 'recorded-session-expired' : 'invitation-required';
}
export function servicesView(state, now = Date.now()) {
  need(state.schema === 'froglet.services.page.v1' && state.client_release === 'v0.4.7-beta.1', 'Wrong page contract/released client');
  need(Object.keys(state).every(k => ['schema', 'client_release', 'qualification', 'provider', 'access', 'capacity', 'ingress', 'services'].includes(k)), 'Unexpected page-data field: do not embed private reports or credentials');
  fields(state.access, ['request_url', 'session_expires_at'], 'access');
  fields(state.capacity, ['limits_state', 'limits', 'snapshot', 'public_status_endpoint'], 'capacity');
  fields(state.ingress, ['state', 'requests_per_second', 'burst', 'connections', 'measured_at', 'measurement_evidence_sha256'], 'ingress');
  fields(state.services, ['arithmetic', 'synthetic'], 'services');
  fields(state.services.arithmetic, ['state', 'program', 'example'], 'arithmetic');
  fields(state.services.synthetic, ['state', 'service_id', 'share_url', 'example'], 'synthetic');
  const q = state.qualification;
  const qualified = q !== null;
  if (qualified) {
    fields(q, ['passed', 'observed_at', 'root_evidence_sha256', 'independent_review_sha256'], 'qualification');
    fields(state.provider, ['id', 'origin', 'instance_count', 'metadata_url'], 'provider');
    need(q.passed === true && instant(q.observed_at) && hash(q.root_evidence_sha256) && hash(q.independent_review_sha256), 'Actual root and peer qualification with UTC observation required');
    need(state.provider && hash(state.provider.id) && https(state.provider.origin, true) && state.provider.instance_count === 1, 'Exact public provider and one shared provider instance required');
    need(https(state.provider.metadata_url) && new URL(state.provider.metadata_url).origin === new URL(state.provider.origin).origin && !new URL(state.provider.metadata_url).search, 'Actually checked public metadata URL on the provider origin required');
  } else {
    need(state.provider === null && Object.values(state.services).every(s => s.state === 'pending'), 'Pending page must not present a provider or observed service');
    need(state.access.request_url === null && state.access.session_expires_at === null, 'Pending page must not publish invitation instructions or a session expiry');
  }
  need(['proposed', 'observed'].includes(state.capacity.limits_state), 'Capacity state required');
  const limits = state.capacity.limits;
  need(Object.keys(limits).length === 3 && ['deals', 'quotes', 'reserved_runtime_ms'].every(k => integer(limits[k]) && limits[k] > 0), 'Finite shared allowance limits required');
  let remaining = null;
  const snapshot = state.capacity.snapshot;
  if (snapshot !== null) {
    fields(snapshot, ['observed_at', 'evidence_sha256', 'reserved_deals', 'issued_quotes', 'reserved_runtime_ms'], 'snapshot');
    need(qualified && state.capacity.limits_state === 'observed' && instant(snapshot.observed_at) && hash(snapshot.evidence_sha256), 'Remaining capacity requires an actual timestamped observation');
    need(['reserved_deals', 'issued_quotes', 'reserved_runtime_ms'].every(k => integer(snapshot[k])), 'Counter snapshot must use actual nonnegative integer totals');
    remaining = {deals: Math.max(0, limits.deals - snapshot.reserved_deals), quotes: Math.max(0, limits.quotes - snapshot.issued_quotes), reserved_runtime_ms: Math.max(0, limits.reserved_runtime_ms - snapshot.reserved_runtime_ms), observed_at: snapshot.observed_at};
  } else need(state.capacity.limits_state === 'proposed', 'Observed limits need their actual counter observation');
  // No non-secret current-status endpoint has been qualified. This proposal
  // intentionally does not poll one or turn snapshots into live counts.
  need(state.capacity.public_status_endpoint === null, 'Public status path needs separate actual qualification and implementation');
  need(['proposed', 'measured'].includes(state.ingress.state) && ['requests_per_second', 'burst', 'connections'].every(k => integer(state.ingress[k]) && state.ingress[k] > 0), 'Finite ingress values/status required');
  if (state.ingress.state === 'measured') need(qualified && instant(state.ingress.measured_at) && hash(state.ingress.measurement_evidence_sha256), 'Public client measurement required before rate limits become final');
  need(state.access.request_url === null || https(state.access.request_url), 'Invitation instructions must use an approved HTTPS link');
  need(state.access.session_expires_at === null || instant(state.access.session_expires_at), 'Session expiry must be explicit UTC');
  if (qualified) {
    need(state.capacity.limits_state === 'observed' && snapshot !== null && state.ingress.state === 'measured', 'Qualified examples need actual finite allowances and measured public ingress');
    need(instant(state.access.session_expires_at) && Date.parse(state.access.session_expires_at) > Date.parse(q.observed_at), 'Qualified session needs an explicit expiry after the recorded qualification');
  }
  const a = state.services.arithmetic, s = state.services.synthetic;
  for (const item of [a, s]) need(['pending', 'observed'].includes(item.state), 'Individual service status required');
  if (a.state === 'pending') need(a.program === null && a.example === null, 'Pending arithmetic must not retain a program or example');
  if (s.state === 'pending') need(s.service_id === null && s.share_url === null && s.example === null, 'Pending lookup must not retain a service ID, link or example');
  let arithmetic = null, synthetic = null;
  if (a.state === 'observed') {
    fields(a.program, ['sha256', 'bytes', 'abi', 'download_url'], 'program');
    need(qualified && a.program && hash(a.program.sha256) && Number.isSafeInteger(a.program.bytes) && a.program.bytes > 0 && a.program.bytes <= 262144 && a.program.abi === 'froglet.wasm.run_json.v1' && https(a.program.download_url), 'Actual supplied Wasm program bytes/ABI/download required');
    example(a.example);
    arithmetic = {mcp: json({action: 'run_compute', provider_id: state.provider.id, provider_url: state.provider.origin, wasm_module_path: '/absolute/path/adder.wasm', input: a.example.input, access_token_file: '/absolute/path/alice-invitation.token', idempotency_key: 'alice-arithmetic-1', max_price_sats: 0, response_format: 'compact'}), input: json(a.example.input), expected: json(a.example.expected_result), program: a.program};
  }
  if (s.state === 'observed') {
    need(qualified && typeof s.service_id === 'string' && /^[a-zA-Z0-9._-]+$/.test(s.service_id), 'Actual published terminology service ID required');
    need(s.share_url === null || https(s.share_url), 'Use only an actually checked share URL');
    example(s.example); lookupInput(s.example.input);
    synthetic = {mcp: json({action: 'invoke_service', service_id: s.service_id, provider_id: state.provider.id, provider_url: state.provider.origin, input: s.example.input, access_token_file: '/absolute/path/alice-invitation.token', idempotency_key: 'alice-terminology-1', max_price_sats: 0, response_format: 'compact'}), input: json(s.example.input), expected: json(s.example.expected_result), cli: [quote('/absolute/path/froglet-node') + ' invoke ' + quote(s.service_id) + ' ' + quote(JSON.stringify(s.example.input)), '  --provider-id ' + quote(state.provider.id) + ' --provider-url ' + quote(state.provider.origin), "  --access-token-file '/absolute/path/alice-invitation.token'", "  --idempotency-key 'alice-terminology-1' --max-price-sats 0 --json"].join(" \\\n"), share_url: s.share_url};
  }
  return {qualified, observed_at: q?.observed_at ?? null, access_state: qualified ? recordedSessionAccess(state.access.session_expires_at, now) : 'pending', remaining, arithmetic, synthetic};
}
