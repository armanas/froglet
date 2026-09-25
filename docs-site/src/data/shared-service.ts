/** Public read plane for the first-party relay journey. Never accepts a URL to proxy. */
export interface SharedServiceEndpoints {
  relaySuffix?: string;
  marketplaceUrl?: string;
}

export function serviceReference(provider: string, service: string, relaySuffix = 'relay.froglet.dev') {
  if (!/^[a-f0-9]{64}$/.test(provider) || !/^[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/.test(service)) {
    throw new Error('This link needs a valid provider identity and service name. Ask the sender for their Froglet share link.');
  }
  if (!/^relay(?:-[a-z0-9]+)*\.froglet\.dev$/.test(relaySuffix)) throw new Error('Invalid first-party relay suffix.');
  const alphabet = 'abcdefghijklmnopqrstuvwxyz234567';
  let bits = 0, buffer = 0, label = '';
  for (let i = 0; i < provider.length; i += 2) {
    buffer = (buffer << 8) | parseInt(provider.slice(i, i + 2), 16);
    bits += 8;
    while (bits >= 5) { bits -= 5; label += alphabet[(buffer >>> bits) & 31]; }
  }
  if (bits) label += alphabet[(buffer << (5 - bits)) & 31];
  return { provider, service, providerUrl: `https://${label}.${relaySuffix}` };
}

async function readJson(url: string, fetcher: typeof fetch): Promise<any> {
  const response = await fetcher(url, { redirect: 'manual', signal: AbortSignal.timeout(6000), headers: { accept: 'application/json' } });
  if (!response.ok || !response.body) throw new Error(`Read unavailable (${response.status})`);
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const part = await reader.read();
      if (part.done) break;
      size += part.value.byteLength;
      if (size > 1024 * 1024) throw new Error('Service metadata exceeds 1 MiB');
      chunks.push(part.value);
    }
  } finally { await reader.cancel(); }
  const bytes = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
  return JSON.parse(new TextDecoder().decode(bytes));
}

export async function sharedService(provider: string, serviceId: string, fetcher: typeof fetch = fetch, endpoints: SharedServiceEndpoints = {}) {
  const reference = serviceReference(provider, serviceId, endpoints.relaySuffix);
  const marketplaceUrl = endpoints.marketplaceUrl ?? 'https://marketplace.froglet.dev';
  if (!/^https:\/\/marketplace(?:-[a-z0-9]+)*\.froglet\.dev$/.test(marketplaceUrl)) throw new Error('Invalid first-party marketplace origin.');
  const checkedAt = new Date().toISOString();
  const data = await readJson(`${reference.providerUrl}/v1/provider/services/${encodeURIComponent(serviceId)}`, fetcher);
  const service = data.service;
  const revision = data.publication_revision;
  if (service?.provider_id !== provider || service?.service_id !== serviceId) throw new Error('The provider returned a different service identity.');
  if (revision && (revision.payload?.provider_id !== provider || revision.payload?.service_id !== serviceId || revision.payload?.offer_id !== service.offer_id || revision.payload?.binding_hash !== service.binding_hash || !/^[a-f0-9]{64}$/.test(revision.revision_hash) || !/^[a-f0-9]{64}$/.test(revision.payload?.offer_hash))) {
    throw new Error('The selected revision does not match this service.');
  }
  let exampleInput: unknown = null;
  if (typeof service.starter === 'string') {
    try { exampleInput = JSON.parse(service.starter); } catch { /* Never execute starter text. */ }
  }
  let marketplace = 'not_verified';
  let leaseExpiresAt: number | null = null;
  if (revision) {
    try {
      const offer = await readJson(`${marketplaceUrl}/v1/offers/${revision.payload.offer_hash}`, fetcher);
      if (offer.provider_id === provider && offer.offer_id === service.offer_id && offer.artifact_hash === revision.payload.offer_hash) {
        leaseExpiresAt = Number.isFinite(offer.availability?.lease_expires_at) ? offer.availability.lease_expires_at : null;
        marketplace = offer.availability?.status === 'healthy' && leaseExpiresAt !== null && leaseExpiresAt * 1000 > Date.now() ? 'active' : 'pending_or_offline';
      }
    } catch { /* Public reachability alone never proves admission. */ }
  }
  return { ...reference, checkedAt, status: 'reachable', summary: String(service.summary ?? '').slice(0, 1000),
    revision: revision?.revision_hash ?? null, offerHash: revision?.payload?.offer_hash ?? null, exampleInput,
    free: service.price_sats === 0 && service.base_fee_msat === 0 && service.success_fee_msat === 0 && service.settlement_method === 'none',
    marketplace, leaseExpiresAt, requesterExecution: 'not_run',
    evidenceBoundary: 'This page reports current HTTP and marketplace observations. Your agent verifies provider signatures and runs a separate requester call.' };
}
