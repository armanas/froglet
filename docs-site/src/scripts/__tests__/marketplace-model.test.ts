import { describe, expect, it } from 'vitest';
import type { MarketplaceOfferSummary } from '../../data/live-snapshot';
import { PUBLIC_DEMO } from '../../data/public-demo-config';
import {
  CATEGORY_LABELS,
  CATEGORY_ORDER,
  builtInTitle,
  categoryOf,
  describeOffer,
  formatAge,
  hasShareLink,
  isFree,
  offerSignature,
  offerTitle,
  priceLabel,
  priceSortKey,
  publicDemoLink,
  isPublicBetaOffer,
  serviceAvailability,
} from '../marketplace-model';

const provider = 'ab'.repeat(32);

function offer(overrides: Partial<MarketplaceOfferSummary> = {}): MarketplaceOfferSummary {
  return {
    providerId: provider, offerId: 'sample', offerKind: 'catalog', runtime: 'builtin', packageKind: 'builtin',
    settlementMethod: 'none', baseFeeMsat: 0, successFeeMsat: 0, artifactHash: 'c'.repeat(64), ...overrides,
  };
}

describe('share links', () => {
  it('accepts only a well-formed first-party share path', () => {
    expect(hasShareLink(offer({ sharePath: `/s/${provider}/hla-catalog` }))).toBe(true);
    expect(hasShareLink(offer({ sharePath: `/s/${provider}/a.b_c-d` }))).toBe(true);
    expect(hasShareLink(offer({ sharePath: `/s/${provider}/${'a'.repeat(128)}` }))).toBe(true);
    expect(hasShareLink(offer())).toBe(false);
  });

  it.each([
    ['a script URL', 'javascript:alert(1)'],
    ['a protocol-relative URL', `//evil.example/s/${provider}/x`],
    ['an absolute URL', `https://evil.example/s/${provider}/x`],
    ['a path traversal', `/s/${provider}/../etc`],
    ['a name that starts with a dot', `/s/${provider}/.hidden`],
    ['an empty name', `/s/${provider}/`],
    ['a name with a space', `/s/${provider}/x y`],
    ['a name with a query string', `/s/${provider}/x?next=//evil.example`],
    ['a name with a fragment', `/s/${provider}/x#frag`],
    ['a name that is too long', `/s/${provider}/${'a'.repeat(129)}`],
    ['an uppercase provider id', `/s/${provider.toUpperCase()}/x`],
    ['a short provider id', `/s/${provider.slice(2)}/x`],
  ])('rejects %s', (_label, sharePath) => {
    expect(hasShareLink(offer({ sharePath }))).toBe(false);
    expect(categoryOf(offer({ sharePath }))).toBe('other');
  });
});

describe('public beta actions', () => {
  const compute = () => offer({ providerId: PUBLIC_DEMO.providerId, offerId: PUBLIC_DEMO.computeOffer, offerKind: 'compute.wasm.v1', runtime: 'wasm', packageKind: 'inline_module' });

  it('maps only the supplied program and selected catalog operations to the existing anonymous demo', () => {
    expect(publicDemoLink(compute())).toBe('/services/#try-it');
    expect(publicDemoLink(offer({ providerId: PUBLIC_DEMO.providerId, offerId: PUBLIC_DEMO.catalogService, offerKind: PUBLIC_DEMO.catalogService, runtime: 'builtin', packageKind: 'builtin' }))).toBe('/services/#try-it');
    expect(serviceAvailability(compute()).ready).toBe(false);
  });

  it.each([
    { providerId: provider }, { offerId: 'execute.compute.generic' },
    { offerKind: 'compute.execution.v1' }, { runtime: 'container' }, { packageKind: 'oci' }, { packageKind: 'wasm' },
    { settlementMethod: 'lightning' }, { baseFeeMsat: 1 }, { successFeeMsat: 1 }, { pricingKnown: false },
    { availability: { admission: 'invitation_required', status: 'healthy', lastCheckedAt: 100, leaseExpiresAt: 200 } },
  ])('refuses a different identity, workload or access policy: %j', change => {
    expect(publicDemoLink({ ...compute(), ...change })).toBeUndefined();
  });
});

describe('curated public beta and exact published actions', () => {
  const profile = { serviceId: 'marketplace-provider', offerId: 'marketplace-provider', offerHash: 'd'.repeat(64), bindingHash: '2'.repeat(64), revisionHash: 'f'.repeat(64), operationHash: '1'.repeat(64), moduleHash: '2'.repeat(64), descriptorHash: '3'.repeat(64), entrypoint: 'run' };
  const named = () => offer({ providerId: PUBLIC_DEMO.providerId, offerId: profile.offerId, offerKind: 'compute.execution.v1', runtime: 'wasm', packageKind: 'inline_module', artifactHash: profile.offerHash, availability: { status: 'healthy', admission: 'execution_checked', lastCheckedAt: 100, leaseExpiresAt: 200 } });

  it('keeps only current supported C7 forms in the default scope, without inventing readiness', () => {
    expect(isPublicBetaOffer(offer({ providerId: PUBLIC_DEMO.providerId, offerId: PUBLIC_DEMO.computeOffer, offerKind: 'compute.wasm.v1', runtime: 'wasm', packageKind: 'inline_module' }))).toBe(true);
    expect(isPublicBetaOffer(offer({ providerId: PUBLIC_DEMO.providerId, offerId: PUBLIC_DEMO.catalogService, offerKind: PUBLIC_DEMO.catalogService, runtime: 'builtin', packageKind: 'builtin' }))).toBe(true);
    expect(isPublicBetaOffer(named())).toBe(true);
    expect(isPublicBetaOffer(named())).toBe(true);
    expect(isPublicBetaOffer({ ...named(), providerId: provider })).toBe(false);
    expect(isPublicBetaOffer({ ...named(), artifactHash: 'a'.repeat(64) })).toBe(true);
    expect(isPublicBetaOffer(offer({ providerId: PUBLIC_DEMO.providerId, offerId: 'events.query' }))).toBe(false);
  });

  it('links the exact configured free named offer only while execution checking remains current', () => {
    expect(publicDemoLink(named(), [profile], false, 150_000)).toBe('/services/?service=marketplace-provider#try-it');
    expect(publicDemoLink(named(), [], false, 150_000)).toBeUndefined();
    expect(publicDemoLink(named(), [profile], true, 150_000)).toBeUndefined();
    expect(publicDemoLink(named(), [profile], false, 200_000)).toBeUndefined();
  });

  it.each([
    { providerId: provider }, { artifactHash: 'a'.repeat(64) }, { offerId: 'events.query' }, { serviceId: 'events.query' },
    { offerKind: 'compute.wasm.v1' }, { runtime: 'builtin' }, { packageKind: 'builtin' },
    { settlementMethod: 'lightning' }, { pricingKnown: false }, { baseFeeMsat: 1 }, { successFeeMsat: 1 },
    { availability: { status: 'unknown', admission: 'execution_checked', lastCheckedAt: 100, leaseExpiresAt: 200 } },
    { availability: { status: 'healthy', admission: 'unknown', lastCheckedAt: 100, leaseExpiresAt: 200 } },
    { availability: { status: 'healthy', admission: 'invitation_required', lastCheckedAt: 100, leaseExpiresAt: 200 } },
    { availability: { status: 'healthy', admission: 'execution_checked', lastCheckedAt: 250, leaseExpiresAt: 300 } },
  ])('refuses a wrong binding, workload, price or execution lease: %j', change => {
    expect(publicDemoLink({ ...named(), ...change }, [profile], false, 150_000)).toBeUndefined();
  });
});

describe('categoryOf', () => {
  it('puts anything with a share link under shared services, whatever its id says', () => {
    expect(categoryOf(offer({ offerId: 'marketplace.search', sharePath: `/s/${provider}/hla-catalog` }))).toBe('service');
  });

  it.each([
    ['marketplace.search', 'catalog', 'marketplace'],
    ['events.query', 'catalog', 'events'],
    ['execute.compute', 'catalog', 'compute'],
    ['plain', 'compute.wasm.v1', 'compute'],
    ['plain', 'marketplace.provider', 'marketplace'],
    ['plain', 'events.query', 'events'],
    ['Marketplace.Search', 'catalog', 'marketplace'],
    ['plain', 'catalog', 'other'],
    ['', '', 'other'],
  ] as const)('classifies %s / %s as %s', (offerId, offerKind, expected) => {
    expect(categoryOf(offer({ offerId, offerKind }))).toBe(expected);
  });

  it('lists shared services first and labels every category once', () => {
    expect(CATEGORY_ORDER[0]).toBe('service');
    const labels = CATEGORY_ORDER.map((category) => CATEGORY_LABELS[category]);
    expect(labels.every(Boolean)).toBe(true);
    expect(new Set(labels).size).toBe(labels.length);
  });
});

describe('pricing', () => {
  it('shows free only when the price is known and every fee is zero', () => {
    expect(isFree(offer())).toBe(true);
    expect(priceLabel(offer())).toBe('Free');
    expect(isFree(offer({ baseFeeMsat: 1000 }))).toBe(false);
    expect(isFree(offer({ successFeeMsat: 1000 }))).toBe(false);
  });

  it('never presents unknown pricing as free', () => {
    const unknown = offer({ pricingKnown: false });
    expect(isFree(unknown)).toBe(false);
    expect(priceLabel(unknown)).toBe('Price unavailable');
  });

  it('does not call an offer free unless it settles for nothing', () => {
    expect(isFree(offer({ settlementMethod: 'lightning' }))).toBe(false);
    expect(priceLabel(offer({ settlementMethod: 'lightning' }))).toBe('0 + 0 sats');
  });

  it('shows the base and success fees in sats', () => {
    expect(priceLabel(offer({ settlementMethod: 'lightning', baseFeeMsat: 3000, successFeeMsat: 5000 }))).toBe('3 + 5 sats');
    expect(priceLabel(offer({ settlementMethod: 'lightning', baseFeeMsat: 1500, successFeeMsat: 0 }))).toBe('1.5 + 0 sats');
  });

  it('sorts free first, priced by total fee, and unknown last', () => {
    const free = priceSortKey(offer());
    const cheap = priceSortKey(offer({ settlementMethod: 'lightning', baseFeeMsat: 1000 }));
    const dear = priceSortKey(offer({ settlementMethod: 'lightning', baseFeeMsat: 1000, successFeeMsat: 9000 }));
    const unknown = priceSortKey(offer({ pricingKnown: false }));
    expect(free).toBeLessThan(cheap);
    expect(cheap).toBeLessThan(dear);
    expect(dear).toBeLessThan(unknown);
  });
});

describe('serviceAvailability', () => {
  const now = 150_000;
  const check = (over: Record<string, unknown> = {}) => offer({ availability: { status: 'healthy', lastCheckedAt: 100, leaseExpiresAt: 200, ...over } });

  it('is recently checked only with a healthy, unexpired check', () => {
    expect(serviceAvailability(check(), false, now)).toEqual({ label: 'Recently checked', ready: true, state: 'ready' });
  });

  it('expires the check when its lease ends, including at the exact moment', () => {
    expect(serviceAvailability(check(), false, 199_999).ready).toBe(true);
    expect(serviceAvailability(check(), false, 200_000)).toEqual({ label: 'Check expired', ready: false, state: 'warn' });
    expect(serviceAvailability(check(), false, 250_000).label).toBe('Check expired');
  });

  it('does not trust a check that claims to be from the future', () => {
    expect(serviceAvailability(check({ lastCheckedAt: 300, leaseExpiresAt: 400 }), false, now).ready).toBe(false);
    expect(serviceAvailability(check({ lastCheckedAt: 209, leaseExpiresAt: 400 }), false, now).ready).toBe(true);
    expect(serviceAvailability(check({ lastCheckedAt: 0 }), false, now).ready).toBe(false);
  });

  it('reports offline providers and unknown status without calling either ready', () => {
    expect(serviceAvailability(check({ status: 'offline' }), false, now)).toEqual({ label: 'Offline', ready: false, state: 'warn' });
    expect(serviceAvailability(check({ status: 'degraded' }), false, now)).toEqual({ label: 'Availability not confirmed', ready: false, state: 'unknown' });
    expect(serviceAvailability(offer(), false, now)).toEqual({ label: 'Availability not confirmed', ready: false, state: 'unknown' });
  });

  it('never says recently checked for data that needs a refresh', () => {
    expect(serviceAvailability(check(), true, now)).toEqual({ label: 'Status needs refresh', ready: false, state: 'warn' });
  });
});

describe('formatAge', () => {
  it.each([
    [-1000, 'just now'],
    [0, 'just now'],
    [4999, 'just now'],
    [5000, '5s ago'],
    [59_999, '59s ago'],
    [60_000, '1m ago'],
    [3_599_000, '59m ago'],
    [3_600_000, '1h ago'],
    [47 * 3_600_000, '47h ago'],
    [48 * 3_600_000, '2d ago'],
  ])('formats %ims as %s', (ms, expected) => {
    expect(formatAge(ms)).toBe(expected);
  });
});

describe('titles and descriptions', () => {
  it('gives the marketplace’s own offers a readable title and keeps unknown ones humanised', () => {
    expect(offerTitle(offer({ offerId: 'marketplace.search' }))).toBe('Search the marketplace');
    expect(offerTitle(offer({ offerId: 'execute.compute' }))).toBe('Run a WebAssembly job');
    expect(offerTitle(offer({ offerId: 'priced-lookup' }))).toBe('Priced Lookup');
    expect(builtInTitle(offer({ offerId: 'priced-lookup' }))).toBeUndefined();
  });

  it('prefers the published service id over a built-in title', () => {
    expect(offerTitle(offer({ offerId: 'events.query', serviceId: 'hla-catalog' }))).toBe('HLA Catalog');
    expect(builtInTitle(offer({ offerId: 'events.query', serviceId: 'hla-catalog' }))).toBeUndefined();
  });

  it('describes an offer with its own summary, then by kind, then generically', () => {
    expect(describeOffer(offer({ summary: 'Custom words', offerKind: 'marketplace.search' }))).toBe('Custom words');
    expect(describeOffer(offer({ offerKind: 'marketplace.search' }))).toBe('Search the marketplace for providers and offers.');
    expect(describeOffer(offer({ offerKind: 'something.new' }))).toContain('Provider-published service');
  });
});

describe('offerSignature', () => {
  const now = 150_000;
  const priced = offer({ settlementMethod: 'lightning', baseFeeMsat: 1000, summary: 'A', availability: { admission: 'open', status: 'healthy', lastCheckedAt: 100, leaseExpiresAt: 200 } });

  it('is stable for an unchanged listing, even as its check age moves', () => {
    const later = { ...priced, availability: { ...priced.availability!, lastCheckedAt: 120 } };
    expect(offerSignature(later, false, now)).toBe(offerSignature(priced, false, now));
  });

  it.each([
    ['price', { baseFeeMsat: 2000 }],
    ['summary', { summary: 'B' }],
    ['admission', { availability: { admission: 'invitation_required', status: 'healthy', lastCheckedAt: 100, leaseExpiresAt: 200 } }],
    ['availability state', { availability: { admission: 'open', status: 'offline', lastCheckedAt: 100, leaseExpiresAt: 200 } }],
  ])('changes when the %s changes', (_label, change) => {
    expect(offerSignature({ ...priced, ...change }, false, now)).not.toBe(offerSignature(priced, false, now));
  });

  it('changes when the data goes stale', () => {
    expect(offerSignature(priced, true, now)).not.toBe(offerSignature(priced, false, now));
  });
});
