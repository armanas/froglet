// Public metadata only. No signing seed, runtime token or provider credential belongs here.
export const PUBLIC_DEMO = {
  providerId: 'c7a15140cc28833978197bdef7daf30e419502f186f59ff88584f453531ea894',
  origin: 'https://froglet-public-beta-20261006.fly.dev',
  computeOffer: 'execute.compute',
  catalogService: 'synthetic-terminology-demo',
  maxModuleBytes: 262144,
  maxInputBytes: 131072,
  maxMemoryBytes: 8388608,
  maxRuntimeMs: 2000,
  maxFuel: 50000000,
} as const;

export const PUBLIC_DEMO_PREFIX = '/api/public-demo';

// These adapters read the existing public catalog. They never reach node
// execution, operator routes, a caller-selected host, or database credentials.
const integer = { type: 'integer', minimum: 0, maximum: Number.MAX_SAFE_INTEGER };
// The immutable HTTP-operation schema compiler supports length bounds, not
// regex/combinators. The Worker separately enforces lowercase hex and names.
const providerId = { type: 'string', minLength: 64, maxLength: 64 };
const text = (maxLength: number) => ({ type: 'string', maxLength });
const object = (properties: Record<string, unknown>, required = Object.keys(properties)) => ({ type: 'object', additionalProperties: false, properties, required });
const nullable = (schema: { type: string; [key: string]: unknown }) => ({ ...schema, type: [schema.type, 'null'] });
const paging = { limit: { type: 'integer', minimum: 1, maximum: 20 }, offset: { type: 'integer', minimum: 0, maximum: 1000 } };
const pagination = object({ ...paging, total: integer });
const availability = object({ status: { type: 'string', enum: ['unknown', 'healthy', 'offline'] }, admission: { type: 'string', enum: ['unknown', 'invitation_required', 'execution_checked'] }, lease_expires_at: nullable(integer), last_renewed_at: nullable(integer) });
const offer = object({ artifact_hash: providerId, provider_id: providerId, offer_id: text(128), offer_kind: text(128), runtime: text(64), package_kind: text(64), contract_version: text(128), settlement_method: text(128), base_fee_msat: integer, success_fee_msat: integer, availability });
const receipt = object({ artifact_hash: providerId, provider_id: providerId, deal_hash: providerId, status: text(64), execution_state: text(64), runtime: text(64), finished_at: integer });

export const PUBLIC_MARKETPLACE_READ = {
  origin: 'https://marketplace.froglet.dev',
  prefix: '/api/marketplace-read',
  maxRequestBytes: 2048,
  maxResponseBytes: 131072,
  maxPageSize: 20,
  defaultPageSize: 10,
  maxOffset: 1000,
  timeoutMs: 1000,
  operations: {
    provider: {
      inputSchema: object({ provider_id: providerId }),
      outputSchema: object({ provider: nullable(object({ provider_id: providerId, current_descriptor_hash: providerId, protocol_version: text(64), service_kinds: { type: 'array', maxItems: 64, items: text(128) }, execution_runtimes: { type: 'array', maxItems: 32, items: text(64) }, transport_endpoints: { type: 'array', maxItems: 16, items: object({ transport: text(64), uri: text(2048) }) }, success_count: integer, failure_count: integer })) }),
    },
    search: {
      inputSchema: object({ provider_id: providerId, offer_kind: { type: 'string', minLength: 1, maxLength: 64 }, runtime: { type: 'string', enum: ['builtin', 'wasm', 'any'] }, availability: { type: 'string', enum: ['discoverable', 'healthy', 'offline', 'unknown', 'all'] }, ...paging }, []),
      outputSchema: object({ items: { type: 'array', maxItems: 20, items: offer }, pagination }),
    },
    receipts: {
      inputSchema: object({ provider_id: providerId, ...paging, updated_since: integer }, ['provider_id']),
      outputSchema: object({ items: { type: 'array', maxItems: 20, items: receipt }, pagination }),
    },
  },
} as const;
