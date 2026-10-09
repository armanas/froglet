// The shapes the playground's two sides pass to each other. They are the ones a node's provider API uses; the tests check
// them against responses captured from a real node (froglet-wasm/tests/fixtures/node_service_exchange.json).

export type ArtifactType = 'descriptor' | 'offer' | 'quote' | 'deal' | 'receipt';

export interface SignedArtifact<Payload = Record<string, any>> {
  artifact_type: ArtifactType;
  schema_version: string;
  signer: string;
  created_at: number;
  payload_hash: string;
  hash: string;
  payload: Payload;
  signature: string;
}

export interface Identity {
  seed_hex: string;
  public_key: string;
}

/** The signing half of the kernel: froglet-protocol compiled to WebAssembly as `froglet-wasm`. */
export interface Kernel {
  newIdentity(): Identity;
  publicKey(seedHex: string): string;
  sign(seedHex: string, type: ArtifactType, createdAt: number, payload: unknown): SignedArtifact;
  /** SHA-256 of the RFC 8785 canonical form: how payload, workload, input, and result hashes are made. */
  hashJson(value: unknown): string;
  /** The RFC 8785 canonical text of a JSON value: the exact bytes a node hands to a Wasm function as its input. */
  canonicalize(value: unknown): string;
  /** SHA-256 of raw bytes: how a module hash is made. */
  hashBytes(bytes: Uint8Array): string;
  /** Why `text` is not JSON, in the words a node uses (its Rust parser's), or null when it is JSON. */
  jsonError(text: string): string | null;
}

export interface DocumentReport {
  artifact_type: string;
  hash: string;
  signer: string;
  status: 'verified' | 'envelope_only' | 'invalid';
  envelope_valid: boolean;
  semantics: { outcome: string; error?: string; reason?: string };
  detail?: string[];
}

export interface ChainReport {
  valid: boolean;
  chain_evaluated: boolean;
  structure_errors?: string[];
  error?: string;
  artifacts: DocumentReport[];
}

/** The verifying half of the kernel: froglet-verify compiled to WebAssembly. */
export interface Verifier {
  verifyDocument(document: unknown): DocumentReport;
  validateChain(documents: unknown[]): ChainReport;
}

export interface Limits {
  fuel_limit: number;
  max_input_bytes: number;
  max_memory_bytes: number;
  max_output_bytes: number;
  max_runtime_ms: number;
}

/** What a node lists at GET /v1/provider/services/{id}. */
export interface ServiceRecord {
  service_id: string;
  offer_id: string;
  offer_kind: string;
  resource_kind: string;
  summary: string;
  runtime: string;
  package_kind: string;
  entrypoint_kind: string;
  entrypoint: string;
  contract_version: string;
  mode: string;
  price_sats: number;
  base_fee_msat: number;
  success_fee_msat: number;
  settlement_method: string;
  publication_state: string;
  provider_id: string;
  module_hash: string;
  binding_hash: string;
  capabilities?: string[];
  mounts?: unknown[];
}

/** The states a node's deal passes through (docs/openapi.yaml, DealRecord). This provider only ever says accepted, succeeded, or failed. */
export type DealStatus = 'accepted' | 'payment_pending' | 'running' | 'result_ready' | 'settlement_pending' | 'succeeded' | 'failed' | 'rejected';

/** What a node returns for a deal: when it is created, and again once it has finished. */
export interface DealRecord {
  created_at: number;
  deal: SignedArtifact;
  deal_id: string;
  idempotency_key?: string;
  quote: SignedArtifact;
  status: DealStatus;
  updated_at: number;
  workload_kind: string;
  receipt?: SignedArtifact;
  result?: unknown;
  result_hash?: string;
  error?: string;
}

export interface HttpRequest {
  method: 'GET' | 'POST';
  path: string;
  body?: unknown;
}

export interface HttpResponse {
  status: number;
  body: unknown;
}

export type Transport = (request: HttpRequest) => Promise<HttpResponse>;

export type RunOutcome =
  | { ok: true; output: string; memoryBytes: number; capEnforced: boolean }
  | { ok: false; error: string; timedOut?: boolean };

/**
 * Runs one module on one input under the limits, and reports what came out. `input` is the canonical JSON text, which is
 * what a function receives on a node, so it sees the same bytes in both places.
 */
export type ModuleRunner = (module: Uint8Array, input: string, limits: Limits) => Promise<RunOutcome>;
