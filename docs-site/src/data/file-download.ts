import type { ServiceLinkView } from './service-link';

export interface FileMetadata {
  filename: string; media_type: string; size_bytes: number; sha256: string;
  expires_at: number; max_downloads: number; max_transfer_bytes: number;
}
export function fileMetadata(contract: ServiceLinkView['contract']): FileMetadata | null {
  if (contract?.contract_version !== 'froglet.builtin.file_download.v1' || contract.price.kind !== 'free') return null;
  const value = (contract.output_schema as { const?: FileMetadata } | null)?.const;
  if (!value || typeof value.filename !== 'string' || !/^[A-Za-z0-9_-][A-Za-z0-9._ -]{0,127}$/.test(value.filename)
    || value.filename.includes('..') || value.filename.endsWith(' ') || typeof value.sha256 !== 'string' || !/^[a-f0-9]{64}$/.test(value.sha256)
    || typeof value.media_type !== 'string' || !/^[a-z0-9!#$&^_.+-]+\/[a-z0-9!#$&^_.+-]+$/.test(value.media_type)
    || value.media_type.length > 128 || !Number.isSafeInteger(value.size_bytes) || value.size_bytes < 0 || value.size_bytes > 8*1024*1024
    || !Number.isSafeInteger(value.expires_at) || value.expires_at <= 0 || value.expires_at > 8640000000000
    || !Number.isSafeInteger(value.max_downloads) || value.max_downloads < 1 || value.max_downloads > 1000000
    || !Number.isSafeInteger(value.max_transfer_bytes) || value.max_transfer_bytes < Math.max(1,value.size_bytes)) return null;
  return value;
}
export function downloadAvailability(view: ServiceLinkView): string | null {
  const file = fileMetadata(view.contract);
  if (!file || view.evidence.verification_state !== 'verified') return 'File metadata has not been verified.';
  if (view.availability.state !== 'published_reachable') return 'The provider is offline or current availability is unconfirmed.';
  if (file.expires_at <= Date.now()/1000) return 'This file share has expired.';
  if (!['open','trial','invite'].includes(view.availability.execution_access ?? 'unknown')) return 'Download access is unavailable on this page. Contact the provider.';
  if (!view.links.download) return 'A verified download endpoint is unavailable.';
  return null;
}
