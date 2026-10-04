import type { DealRecord, Kernel, Limits, ServiceRecord, SignedArtifact } from './types';

export const SCHEMA_VERSION = 'froglet/v1';
export const WORKLOAD_KIND_EXECUTION = 'compute.execution.v1';
export const RUN_JSON_ABI = 'froglet.wasm.run_json.v1';
export const JCS_JSON_FORMAT = 'application/json+jcs';
export const WASM_PAGE_BYTES = 65_536;
export const QUOTE_TTL_SECS = 30;
/** The largest module a node accepts (its wasm_hex_limit is 524288 hex characters). */
export const MAX_MODULE_BYTES = 262_144;

/**
 * What Bob offers here: the limits a node offers, except the time limit, which is short so that a function that never
 * finishes is quick to show. Browsers cannot count fuel, so `fuel_limit` is only what the offer says, not something
 * this provider enforces; the time limit and the memory cap are.
 */
export const PLAYGROUND_LIMITS: Limits = {
  fuel_limit: 50_000_000,
  max_input_bytes: 131_072,
  max_memory_bytes: 8_388_608,
  max_output_bytes: 131_072,
  max_runtime_ms: 2_000,
};

/** A refusal carries the HTTP status the provider answers with. */
export class Refusal extends Error {
  constructor(readonly status: number, message: string) {
    super(message);
  }
}

const MAX_SAFE = 9_007_199_254_740_991n;

/**
 * The first integer literal in JSON text that a JavaScript reader would round, or null. Outside ±(2^53 − 1) a browser
 * cannot hold an integer exactly, so the same request would hash and answer differently from a Rust node, which reads
 * integers exactly. Both sides here refuse such numbers rather than let the two disagree in silence.
 */
export function findUnsafeInteger(json: string): string | null {
  let inString = false;
  for (let i = 0; i < json.length; i++) {
    const char = json[i];
    if (inString) {
      if (char === '\\') i += 1;
      else if (char === '"') inString = false;
      continue;
    }
    if (char === '"') {
      inString = true;
    } else if (char === '-' || (char >= '0' && char <= '9')) {
      let end = i + 1;
      while (end < json.length && json[end] >= '0' && json[end] <= '9') end += 1;
      const digits = json.slice(i, end).replace('-', '');
      const isInteger = json[end] !== '.' && json[end] !== 'e' && json[end] !== 'E';
      if (isInteger && digits !== '' && BigInt(digits) > MAX_SAFE) return json.slice(i, end);
      while (end < json.length && /[0-9.eE+-]/.test(json[end])) end += 1;
      i = end - 1;
    }
  }
  return null;
}

/** Whether a parsed value holds a number JavaScript could not hold: `1e999` reads as Infinity and would be written as `null`. */
export function hasNonFiniteNumber(value: unknown): boolean {
  if (typeof value === 'number') return !Number.isFinite(value);
  if (Array.isArray(value)) return value.some(hasNonFiniteNumber);
  if (value && typeof value === 'object') return Object.values(value).some(hasNonFiniteNumber);
  return false;
}

/**
 * The workload that calls a published function by name, built from its service record the way the project's JS client
 * does (`buildServiceAddressedExecution`). Its hash is the `workload_hash` in the quote and the deal.
 */
export function executionFor(service: ServiceRecord, input: unknown, kernel: Kernel) {
  return {
    schema_version: SCHEMA_VERSION,
    workload_kind: WORKLOAD_KIND_EXECUTION,
    runtime: service.runtime,
    package_kind: service.package_kind,
    entrypoint: { kind: service.entrypoint_kind, value: service.entrypoint },
    contract_version: service.contract_version,
    input_format: JCS_JSON_FORMAT,
    input_hash: kernel.hashJson(input),
    security: { mode: 'standard', service_id: service.service_id },
    input,
    module_hash: service.binding_hash,
  };
}

/**
 * The requester's commitment to a free quote, with the deadlines a node's requester gives it: admission at the quote's
 * expiry, then the time the execution may take.
 */
export function dealPayloadFor(quote: SignedArtifact, requesterId: string, successPaymentHash: string) {
  const admission = quote.payload.expires_at as number;
  const window = Math.max(1, Math.ceil((quote.payload.execution_limits.max_runtime_ms as number) / 1000));
  return {
    requester_id: requesterId,
    provider_id: quote.payload.provider_id,
    quote_hash: quote.hash,
    workload_hash: quote.payload.workload_hash,
    success_payment_hash: successPaymentHash,
    admission_deadline: admission,
    completion_deadline: admission + window,
    acceptance_deadline: admission + window,
  };
}

/** A deal is over when it has succeeded, failed, or been rejected. A node also says `running` while it executes, and other states while a payment settles. */
export const isFinished = (record: Pick<DealRecord, 'status'>) => record.status === 'succeeded' || record.status === 'failed' || record.status === 'rejected';
