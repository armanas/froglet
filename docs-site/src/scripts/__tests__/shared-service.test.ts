import { describe, expect, it, vi } from 'vitest';
import { sharedService, serviceReference } from '../../data/shared-service';

const provider = '00'.repeat(32);
const service = { provider_id: provider, service_id: 'catalog', offer_id: 'offer', binding_hash: 'cc'.repeat(32), summary: 'Catalog', starter: '{"op":"describe"}', price_sats: 0, base_fee_msat: 0, success_fee_msat: 0, settlement_method: 'none' };
const revision = { revision_hash: 'aa'.repeat(32), payload: { provider_id: provider, service_id: 'catalog', offer_id: 'offer', binding_hash: service.binding_hash, offer_hash: 'bb'.repeat(32) } };
const response = (value: unknown) => new Response(JSON.stringify(value));

describe('shared service observation', () => {
  it('derives only the identity-bound first-party relay origin', () => {
    expect(serviceReference(provider, 'catalog').providerUrl).toBe(`https://${'a'.repeat(52)}.relay.froglet.dev`);
    expect(serviceReference(provider, 'catalog', 'relay-candidate.froglet.dev').providerUrl).toBe(`https://${'a'.repeat(52)}.relay-candidate.froglet.dev`);
    expect(() => serviceReference('https://127.0.0.1', 'catalog')).toThrow();
    expect(() => serviceReference(provider, '../admin')).toThrow();
    expect(() => serviceReference(provider, 'catalog', 'localhost')).toThrow();
  });
  it('checks the candidate offer only against its configured first-party marketplace', async () => {
    const offer = { provider_id: provider, offer_id: 'offer', artifact_hash: revision.payload.offer_hash, availability: { status: 'healthy', lease_expires_at: Date.now() / 1000 + 60 } };
    const fetcher = vi.fn().mockResolvedValueOnce(response({ service, publication_revision: revision })).mockResolvedValueOnce(response(offer));
    const result = await sharedService(provider, 'catalog', fetcher, { relaySuffix: 'relay-candidate.froglet.dev', marketplaceUrl: 'https://marketplace-candidate.froglet.dev' });
    expect(result.marketplace).toBe('active');
    expect(fetcher.mock.calls[0][0]).toBe(`https://${'a'.repeat(52)}.relay-candidate.froglet.dev/v1/provider/services/catalog`);
    expect(fetcher.mock.calls[1][0]).toBe(`https://marketplace-candidate.froglet.dev/v1/offers/${revision.payload.offer_hash}`);
  });
  it('keeps public reachability separate from marketplace admission and execution', async () => {
    const fetcher = vi.fn().mockResolvedValueOnce(response({ service, publication_revision: revision })).mockRejectedValueOnce(new Error('offline'));
    const result = await sharedService(provider, 'catalog', fetcher);
    expect(result.status).toBe('reachable'); expect(result.marketplace).toBe('not_verified'); expect(result.requesterExecution).toBe('not_run');
    expect(result.exampleInput).toEqual({ op: 'describe' });
    expect(fetcher.mock.calls[0][1].redirect).toBe('manual');
  });
  it('refuses a redirect rather than following provider-controlled metadata', async () => {
    const fetcher = vi.fn().mockResolvedValue(new Response(null, { status: 302, headers: { location: 'https://example.invalid' } }));
    await expect(sharedService(provider, 'catalog', fetcher)).rejects.toThrow('Read unavailable (302)');
    expect(fetcher).toHaveBeenCalledTimes(1);
  });
  it('requires an unexpired exact-offer health lease before showing activation', async () => {
    const offer = { provider_id: provider, offer_id: 'offer', artifact_hash: revision.payload.offer_hash, availability: { status: 'healthy', lease_expires_at: Date.now() / 1000 - 1 } };
    const fetcher = vi.fn().mockResolvedValueOnce(response({ service, publication_revision: revision })).mockResolvedValueOnce(response(offer));
    expect((await sharedService(provider, 'catalog', fetcher)).marketplace).toBe('pending_or_offline');
  });
  it('rejects metadata from another identity or revision before marketplace lookup', async () => {
    const fetcher = vi.fn().mockResolvedValue(response({ service: { ...service, provider_id: '11'.repeat(32) } }));
    await expect(sharedService(provider, 'catalog', fetcher)).rejects.toThrow('different service identity');
    expect(fetcher).toHaveBeenCalledTimes(1);
  });
});

describe('recipient page recovery', () => {
  async function mount(fetcher: typeof fetch, reference = `?provider=${provider}&service=catalog`) {
    vi.useFakeTimers(); vi.resetModules();
    document.body.innerHTML = ['service-title','service-provider','availability','service-summary','service-revision','service-marketplace','service-example','checked-at','copy-status']
      .map(id => `<p id="${id}"></p>`).join('') + '<textarea id="recipient-prompt" readonly></textarea><button id="copy-service-prompt">Copy</button><button id="refresh-service">Refresh</button>';
    vi.stubGlobal('location', { search: reference });
    vi.stubGlobal('fetch', fetcher);
    vi.stubGlobal('navigator', { clipboard: { writeText: vi.fn().mockRejectedValue(new Error('denied')) } });
    await import('../shared-service-page');
    await vi.advanceTimersByTimeAsync(0);
  }
  function cleanup() { vi.clearAllTimers(); vi.useRealTimers(); vi.unstubAllGlobals(); document.body.innerHTML = ''; }
  it('keeps the full recipient prompt selectable when clipboard access is denied', async () => {
    try {
      await mount(vi.fn().mockRejectedValue(new Error('offline')));
      document.getElementById('copy-service-prompt')!.click();
      await vi.advanceTimersByTimeAsync(0);
      const prompt = document.getElementById('recipient-prompt') as HTMLTextAreaElement;
      expect(document.activeElement).toBe(prompt);
      expect(prompt.selectionEnd - prompt.selectionStart).toBe(prompt.value.length);
      expect(prompt.value).toContain(provider);
      expect(prompt.value).toContain('Do not accept a paid offer');
      expect(document.getElementById('copy-status')!.textContent).toContain('device’s copy command');
    } finally { cleanup(); }
  });
  it('retains selected revision as stale when a later availability read fails', async () => {
    try {
      const fetcher = vi.fn().mockResolvedValueOnce(response({ summary:'Catalog', revision:revision.revision_hash, marketplace:'active', leaseExpiresAt:Date.now()/1000+60, checkedAt:new Date().toISOString(), free:true, exampleInput:{op:'describe'} }))
        .mockRejectedValueOnce(new Error('offline'));
      await mount(fetcher);
      expect(document.getElementById('service-marketplace')!.textContent).toContain('Active at the last check');
      document.getElementById('refresh-service')!.click();
      await vi.advanceTimersByTimeAsync(0);
      expect(document.getElementById('availability')!.textContent).toContain('STALE');
      expect(document.getElementById('service-marketplace')!.textContent).toBe('Current activation not verified');
      expect(document.getElementById('service-revision')!.textContent).toBe(revision.revision_hash);
    } finally { cleanup(); }
  });
  it('does not fetch or offer a recipient prompt for an invalid reference', async () => {
    try {
      const fetcher = vi.fn(); await mount(fetcher, '?provider=invalid&service=catalog');
      expect(fetcher).not.toHaveBeenCalled();
      expect((document.getElementById('copy-service-prompt') as HTMLButtonElement).disabled).toBe(true);
    } finally { cleanup(); }
  });
});
