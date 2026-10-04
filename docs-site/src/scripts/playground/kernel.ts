import initKernel, * as kernelModule from '../../generated/kernel/froglet_wasm.js';
import * as verifierModule from '../../generated/verifier/froglet_verify.js';
import { loadVerifier } from '../receipt-verifier';
import type { ArtifactType, Kernel, Verifier } from './types';

// Adapters that give the generated WebAssembly bindings, which take and return strings, the typed shape the playground
// uses. They add no logic: every key, signature, hash, and verdict comes from the Rust code.

/** The exports of the froglet-wasm build. */
export interface KernelModule {
  new_identity(): string;
  public_key_from_seed(seedHex: string): string;
  sign_artifact_json(seedHex: string, artifactType: string, createdAt: number, payloadJson: string): string;
  canonical_sha256_json(json: string): string;
  canonicalize_json(json: string): string;
  sha256_bytes(bytes: Uint8Array): string;
}

/** The exports of the froglet-verify build that the playground uses. */
export interface VerifierModule {
  verify_document_json(documentJson: string, nowUnix?: number): string;
  validate_chain_json(documentsJson: string, nowUnix?: number): string;
}

export function kernelFrom(module: KernelModule): Kernel {
  return {
    newIdentity: () => JSON.parse(module.new_identity()),
    publicKey: (seedHex) => module.public_key_from_seed(seedHex),
    sign: (seedHex, type: ArtifactType, createdAt, payload) => JSON.parse(module.sign_artifact_json(seedHex, type, createdAt, JSON.stringify(payload))),
    hashJson: (value) => module.canonical_sha256_json(JSON.stringify(value)),
    canonicalize: (value) => module.canonicalize_json(JSON.stringify(value)),
    hashBytes: (bytes) => module.sha256_bytes(bytes),
    jsonError: (text) => {
      try {
        module.canonicalize_json(text);
        return null;
      } catch (error) {
        return String(error).replace(/^input is not JSON: /, '');
      }
    },
  };
}

export function verifierFrom(module: VerifierModule): Verifier {
  return {
    verifyDocument: (document) => JSON.parse(module.verify_document_json(JSON.stringify(document), undefined)),
    validateChain: (documents) => JSON.parse(module.validate_chain_json(JSON.stringify(documents), undefined)),
  };
}

let kernelLoading: Promise<Kernel> | undefined;

/** Loads the signing kernel once; a failed load can be retried. */
export function loadKernel(): Promise<Kernel> {
  return (kernelLoading ??= initKernel()
    .then(() => kernelFrom(kernelModule))
    .catch((error) => {
      kernelLoading = undefined;
      throw error;
    }));
}

/** Loads the verifier once (the page's panel shares it) and adapts it. */
export async function loadPlaygroundVerifier(): Promise<Verifier> {
  await loadVerifier();
  return verifierFrom(verifierModule);
}
