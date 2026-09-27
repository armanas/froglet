import wasm from '../generated/verifier/froglet_verify_bg.wasm';
import { initSync, verify_service_link_evidence_json } from '../generated/verifier/froglet_verify.js';

let initialized = false;

export function verifyServiceLinkEvidence(revision: unknown, offer: unknown, descriptor: unknown) {
  if (!initialized) {
    initSync({ module: wasm });
    initialized = true;
  }
  const report = JSON.parse(verify_service_link_evidence_json(JSON.stringify({ publication_revision: revision, offer, descriptor })));
  return { valid: report.valid === true, reason: typeof report.reason === 'string' ? report.reason : undefined };
}
