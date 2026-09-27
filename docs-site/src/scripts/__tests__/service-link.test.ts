import { readFileSync } from 'node:fs';
import sharp from 'sharp';
import { describe, expect, it, vi } from 'vitest';
import { resolveServiceLink, restoreServiceLinkCache } from '../../data/service-link';
import { renderServiceLinkHtml, renderServiceLinkMarkdown } from '../../data/service-link-page';

const provider = '11'.repeat(32);
const offerHash = '22'.repeat(32);
const descriptorHash = '33'.repeat(32);
const bindingHash = '44'.repeat(32);
const revisionHash = '55'.repeat(32);
const service = 'test-catalog';
const reference = {
  service: { provider_id: provider, service_id: service, offer_id: 'offer-1', binding_hash: bindingHash },
  publication_revision: { revision_hash: revisionHash, payload: {
    provider_id: provider, service_id: service, offer_id: 'offer-1', binding_hash: bindingHash, offer_hash: offerHash,
    runtime: 'builtin', service: { summary: 'Catalog of public specimens', starter: '{"op":"describe"}', input_schema: { type: 'object', properties: { op: { type: 'string' } }, required: ['op'] }, output_schema: { type: 'object' } },
    limits: { max_input_bytes: 4096, max_runtime_ms: 5000 },
    price: { settlement_method: 'none', currency: 'sat', base_amount_minor: 0, success_amount_minor: 0, offer_settlement_method: 'none' },
  } },
};
const offer = { hash: offerHash, payload: { provider_id: provider, offer_id: 'offer-1', descriptor_hash: descriptorHash, settlement_method: 'none', price_schedule: { base_fee_msat: 0, success_fee_msat: 0 } } };
const descriptor = { hash: descriptorHash, payload: { provider_id: provider } };
const response = (value: any) => {
  if (value?.hash && value?.payload) {
    const kind = value.payload.descriptor_hash ? 'offer' : 'descriptor';
    return new Response(JSON.stringify({ kind, hash: value.hash, document: { ...value, artifact_type: kind } }));
  }
  return new Response(JSON.stringify(value));
};
const origin = 'https://froglet.dev';
const verifier = vi.fn(() => ({ valid: true }));

function fetchSequence(...values: unknown[]) {
  return vi.fn().mockImplementation(() => Promise.resolve(response(values.shift())));
}

describe('agent-readable service link', () => {
  it('separates invitation access from free pricing without sharing credentials', async () => {
    const fetcher = fetchSequence({ ...reference, execution_access: 'invite' }, offer, descriptor);
    const view = await resolveServiceLink(provider, service, origin, fetcher, {}, verifier);
    expect(view.availability.execution_access).toBe('invite');
    expect(view.availability.requester_execution).toBe('not_run');
    expect(view.instructions.native_invoke).toContain('--access-token-file <invitation-file>');
    expect(view.instructions.recipient_prompt).toContain('Never place a credential');
    expect(view.links.share).toBe(`${origin}/s/${provider}/${service}`);
    expect(renderServiceLinkHtml(view)).toContain('Invitation required');
    expect(renderServiceLinkMarkdown(view)).toContain('Access: Invitation required');
    const stale = await restoreServiceLinkCache(JSON.stringify(view), provider, service, origin, verifier);
    expect(stale?.availability.execution_access).toBe('unknown');
    expect(stale?.instructions.native_invoke).toBeNull();
  });
  it('does not offer a call command for a private provider', async () => {
    const view = await resolveServiceLink(provider, service, origin,
      fetchSequence({ ...reference, execution_access: 'private' }, offer, descriptor), {}, verifier);
    expect(view.instructions.native_invoke).toBeNull();
    expect(renderServiceLinkHtml(view)).toContain('Only the provider can run this service');
  });
  it('builds HTML, Markdown and JSON from one signed active revision', async () => {
    const fetcher = fetchSequence(reference, offer, descriptor);
    const view = await resolveServiceLink(provider, service, origin, fetcher, {}, verifier);
    expect(view.availability.state).toBe('published_reachable');
    expect(view.evidence.verification_state).toBe('verified');
    expect(view.contract?.price.kind).toBe('free');
    expect(view.instructions.native_invoke).toContain('froglet-node invoke test-catalog -');
    expect(view.presentation.example_input).toEqual({ op: 'describe' });
    expect(view.links.manifest).toBe(`${origin}/s/${provider}/${service}/manifest.json`);
    expect(fetcher.mock.calls.map(call => call[0])).toEqual([
      expect.stringMatching(/^https:\/\/[a-z2-7]{52}\.relay\.froglet\.dev\/v1\/provider\/services\/test-catalog$/),
      expect.stringMatching(new RegExp(`/v1/artifacts/${offerHash}$`)),
      expect.stringMatching(new RegExp(`/v1/artifacts/${descriptorHash}$`)),
    ]);
    expect(fetcher.mock.calls.every(call => call[1].redirect === 'manual')).toBe(true);
    const html = renderServiceLinkHtml(view);
    const markdown = renderServiceLinkMarkdown(view);
    expect(html).toContain('Catalog of public specimens · Froglet service');
    expect(html).toContain('<h1>Catalog of public specimens</h1>');
    expect(html).toContain('name="robots" content="noindex"');
    expect(html).toContain('rel="alternate"');
    expect(html).toContain(view.links.manifest);
    expect(markdown).toContain('Catalog of public specimens');
    expect(markdown).toContain(view.links.manifest);
    expect(markdown).toContain('Price: Free');
    expect(JSON.stringify(view)).toContain('froglet.service-link.v1');
  });

  it('provides a compact service-specific preview and a public square PNG without JavaScript', async () => {
    const view = await resolveServiceLink(provider, service, origin, fetchSequence(reference, offer, descriptor), {}, verifier);
    const page = new DOMParser().parseFromString(renderServiceLinkHtml(view), 'text/html');
    const meta = (name: string) => page.querySelector(`meta[property="${name}"],meta[name="${name}"]`)?.getAttribute('content');
    expect(meta('twitter:card')).toBe('summary');
    expect(meta('og:title')).toBe('🐸 Froglet — Catalog of public specimens');
    expect(meta('twitter:title')).toBe(meta('og:title'));
    expect(meta('og:description')).toBe(view.presentation.summary);
    expect(meta('twitter:description')).toBe(meta('og:description'));
    expect(meta('og:url')).toBe(view.links.share);
    expect(page.querySelector('link[rel="canonical"]')?.getAttribute('href')).toBe(view.links.share);
    expect(page.querySelector('link[rel="icon"]')?.getAttribute('href')).toBe('/favicon.svg');
    expect(meta('og:image')).toBe(`${origin}/og/service.png`);
    expect(meta('twitter:image')).toBe(meta('og:image'));
    expect(meta('og:image:alt')).toBe('Froglet frog mark');
    expect(meta('twitter:image:alt')).toBe(meta('og:image:alt'));
    expect(meta('og:image:type')).toBe('image/png');
    const image = await sharp(readFileSync('public/og/service.png')).metadata();
    expect(image.format).toBe('png');
    expect(image.width).toBe(512);
    expect(image.height).toBe(512);
    expect(meta('og:image:width')).toBe(String(image.width));
    expect(meta('og:image:height')).toBe(String(image.height));
  });

  it('keeps previews short, escaped, and honest about unavailable services', async () => {
    const view = await resolveServiceLink(provider, service, origin, fetchSequence(reference, offer, descriptor), {}, verifier);
    view.presentation.title = '"/><script>alert(1)</script> ' + '🐸'.repeat(80);
    view.presentation.summary = 'A catalog.\n\t' + '🧬'.repeat(200);
    const getPreview = (value: typeof view) => new DOMParser().parseFromString(renderServiceLinkHtml(value), 'text/html');
    const page = getPreview(view);
    expect(page.querySelectorAll('script')).toHaveLength(2);
    expect(page.querySelector('meta[property="og:title"]')?.getAttribute('content')).toContain('"/><script>');
    const summary = page.querySelector('meta[property="og:description"]')?.getAttribute('content') ?? '';
    expect(Array.from(summary)).toHaveLength(140);
    expect(summary).not.toMatch(/[\n\t\uFFFD]/);
    expect(summary.endsWith('…')).toBe(true);
    const stale = { ...view, availability: { ...view.availability, state: 'published_unreachable' as const } };
    const staleDescription = getPreview(stale).querySelector('meta[property="og:description"]')?.getAttribute('content') ?? '';
    expect(staleDescription).toMatch(/^Availability unconfirmed\. /);
    expect(Array.from(staleDescription).length).toBeLessThanOrEqual(140);
    const unknown = { ...view, availability: { ...view.availability, state: 'unknown' as const } };
    expect(getPreview(unknown).querySelector('meta[property="og:description"]')?.getAttribute('content')).toContain('could not be checked');
  });

  it('escapes untrusted publisher text in initial HTML and JSON-LD', async () => {
    const poisoned = structuredClone(reference);
    poisoned.publication_revision.payload.service.summary = '<script>alert(1)</script> public catalog';
    const view = await resolveServiceLink(provider, service, origin, fetchSequence(poisoned, offer, descriptor), {}, verifier);
    const html = renderServiceLinkHtml(view);
    expect(html).not.toContain('<script>alert(1)</script>');
    expect(html).toContain('&lt;script&gt;alert(1)&lt;/script&gt;');
    expect(html).toContain('\\u003cscript');
  });

  it('does not describe changed price or invalid signatures as callable', async () => {
    const changed = structuredClone(offer);
    changed.payload.price_schedule.base_fee_msat = 1000;
    const mismatch = await resolveServiceLink(provider, service, origin, fetchSequence(reference, changed, descriptor), {}, verifier);
    expect(mismatch.contract).toBeNull();
    expect(mismatch.availability.state).toBe('unknown');

    const invalid = await resolveServiceLink(provider, service, origin, fetchSequence(reference, offer, descriptor), {}, () => ({ valid: false, reason: 'signature invalid' }));
    expect(invalid.evidence.verification_state).toBe('invalid');
    expect(invalid.links.call).toBeNull();
    expect(invalid.availability.state).toBe('unknown');

    const verifierUnavailable = await resolveServiceLink(provider, service, origin, fetchSequence(reference, offer, descriptor), {}, () => { throw new Error('Wasm unavailable'); });
    expect(verifierUnavailable.availability.state).toBe('unknown');
    expect(verifierUnavailable.contract).toBeNull();
  });

  it('omits a starter when its schema does not allow it and treats outages as unknown', async () => {
    const bad = structuredClone(reference);
    bad.publication_revision.payload.service.starter = '{"op":42}';
    const view = await resolveServiceLink(provider, service, origin, fetchSequence(bad, offer, descriptor), {}, verifier);
    expect(view.presentation.example_input).toBeNull();
    const offline = await resolveServiceLink(provider, service, origin, vi.fn().mockRejectedValue(new Error('offline')));
    expect(offline.availability.state).toBe('unknown');
    expect(offline.contract).toBeNull();
    expect(offline.instructions.native_invoke).toBeNull();
    expect(renderServiceLinkHtml(offline)).toContain('Current publication and availability could not be established');
  });

  it('shows a signed starter for a supported oneOf schema only when one branch matches', async () => {
    const data = structuredClone(reference);
    data.publication_revision.payload.service.input_schema = { oneOf: [
      { type: 'object', properties: { op: { const: 'describe' } }, required: ['op'], additionalProperties: false },
      { type: 'object', properties: { op: { const: 'select' }, collection: { type: 'string' } }, required: ['op', 'collection'], additionalProperties: false },
    ] };
    const valid = await resolveServiceLink(provider, service, origin, fetchSequence(data, offer, descriptor), {}, verifier);
    expect(valid.presentation.example_input).toEqual({ op: 'describe' });
    data.publication_revision.payload.service.starter = '{"op":"delete"}';
    const invalid = await resolveServiceLink(provider, service, origin, fetchSequence(data, offer, descriptor), {}, verifier);
    expect(invalid.presentation.example_input).toBeNull();
  });

  it('explains older generic catalog summaries using their signed collection schema', async () => {
    const data = structuredClone(reference);
    data.publication_revision.payload.service.summary = `Shared ${service}`;
    data.publication_revision.payload.service.output_schema = { type: 'object', 'x-froglet-collections': { products: ['id', 'name', 'price_eur'] } };
    const view = await resolveServiceLink(provider, service, origin, fetchSequence(data, offer, descriptor), {}, verifier);
    expect(view.presentation.title).toBe('Test Catalog');
    expect(view.presentation.summary).toContain('1 published table: products');
    expect(renderServiceLinkHtml(view)).toContain('<code>price_eur</code>');
    expect(renderServiceLinkHtml(view)).toContain('a summary count does not imply access');
    expect(renderServiceLinkMarkdown(view)).toContain('"products":["id","name","price_eur"]');
  });

  it('rechecks cached signatures and refuses a mismatched cached identity', async () => {
    const view = await resolveServiceLink(provider, service, origin, fetchSequence(reference, offer, descriptor), {}, verifier);
    expect(await restoreServiceLinkCache(JSON.stringify(view), provider, service, origin, () => ({ valid: false }))).toBeNull();
    expect(await restoreServiceLinkCache(JSON.stringify(view), '66'.repeat(32), service, origin, verifier)).toBeNull();
    const stale = await restoreServiceLinkCache(JSON.stringify(view), provider, service, origin, verifier);
    expect(stale?.availability.state).toBe('published_unreachable');
    expect(stale?.availability.marketplace_admission).toBe('not_verified');
    expect(stale?.instructions.native_invoke).toBeNull();
    // Older cached pages predate the readable catalog-title fallback.
    view.presentation.title = 'Query published catalog data: products (id, name, price_eur)';
    view.presentation.summary = view.presentation.title;
    const legacy = await restoreServiceLinkCache(JSON.stringify(view), provider, service, origin, verifier);
    expect(legacy?.presentation.title).toBe('Test Catalog');
    expect(legacy?.presentation.summary).toBe('Open the service details to inspect its inputs, outputs, and availability.');
    expect(renderServiceLinkHtml(legacy!)).toContain('🐸 Froglet — Test Catalog');
  });
});
