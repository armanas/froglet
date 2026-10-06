// Pure presentation logic for the marketplace dashboard: classification, labels, prices and
// availability. No DOM access, so every rule here is unit-tested directly.

import type { MarketplaceOfferSummary } from '../data/live-snapshot';
import { PUBLIC_DEMO } from '../data/public-demo-config';
import { serviceName } from '../data/service-presentation';

export type AvailabilityState = 'ready' | 'warn' | 'unknown';
export interface Availability { label: string; ready: boolean; state: AvailabilityState }

/** A listing is "Recently checked" only with a healthy, unexpired provider check that is not from the future. */
export function serviceAvailability(offer: MarketplaceOfferSummary, stale = false, now = Date.now()): Availability {
	if (stale) return { label: 'Status needs refresh', ready: false, state: 'warn' };
	const a = offer.availability;
	if (a?.status === 'healthy' && a.leaseExpiresAt * 1000 > now && a.lastCheckedAt > 0 && a.lastCheckedAt * 1000 <= now + 60_000) return { label: 'Recently checked', ready: true, state: 'ready' };
	if (a?.leaseExpiresAt && a.leaseExpiresAt * 1000 <= now) return { label: 'Check expired', ready: false, state: 'warn' };
	if (a?.status === 'offline') return { label: 'Offline', ready: false, state: 'warn' };
	return { label: 'Availability not confirmed', ready: false, state: 'unknown' };
}

export type Category = 'service' | 'compute' | 'marketplace' | 'events' | 'other';

export const CATEGORY_ORDER: readonly Category[] = ['service', 'compute', 'marketplace', 'events', 'other'];

export const CATEGORY_LABELS: Record<Category, string> = {
	service: 'Shared services',
	compute: 'Compute',
	marketplace: 'Marketplace',
	events: 'Events',
	other: 'Other offers',
};

const SHARE_PATH = /^\/s\/[a-f0-9]{64}\/[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/;

/** Only a well-formed first-party share path is ever linked. */
export function hasShareLink(offer: MarketplaceOfferSummary): boolean {
	return Boolean(offer.sharePath && SHARE_PATH.test(offer.sharePath));
}

/** This site's two anonymous demo operations. Registry-supplied URLs never become actions. */
export function publicDemoLink(offer: MarketplaceOfferSummary): string | undefined {
	if (offer.providerId !== PUBLIC_DEMO.providerId || !isFree(offer) || offer.availability?.admission === 'invitation_required') return undefined;
	const compute = offer.offerId === PUBLIC_DEMO.computeOffer && offer.offerKind === 'compute.wasm.v1' && offer.runtime === 'wasm' && offer.packageKind === 'inline_module';
	const catalog = offer.offerId === PUBLIC_DEMO.catalogService && offer.offerKind === PUBLIC_DEMO.catalogService && offer.runtime === 'builtin' && offer.packageKind === 'builtin';
	return compute || catalog ? '/services/#try-it' : undefined;
}

/** Shared services are the ones with a share link; everything else is grouped by what the offer does. */
export function categoryOf(offer: MarketplaceOfferSummary): Category {
	if (hasShareLink(offer)) return 'service';
	const id = (offer.offerId || '').toLowerCase();
	const kind = (offer.offerKind || '').toLowerCase();
	if (id.startsWith('marketplace.') || kind.startsWith('marketplace.')) return 'marketplace';
	if (id.startsWith('events.') || kind.startsWith('events.')) return 'events';
	if (id.startsWith('execute.') || kind.startsWith('compute.')) return 'compute';
	return 'other';
}

export function isFree(offer: MarketplaceOfferSummary): boolean {
	return offer.pricingKnown !== false && offer.settlementMethod === 'none' && offer.baseFeeMsat === 0 && offer.successFeeMsat === 0;
}

/** Unknown pricing is never shown as free. */
export function priceLabel(offer: MarketplaceOfferSummary): string {
	if (offer.pricingKnown === false) return 'Price unavailable';
	if (isFree(offer)) return 'Free';
	return `${offer.baseFeeMsat / 1000} + ${offer.successFeeMsat / 1000} sats`;
}

/** Sort key for price: free first, then by total fee, unknown last. */
export function priceSortKey(offer: MarketplaceOfferSummary): number {
	if (offer.pricingKnown === false) return Number.MAX_SAFE_INTEGER;
	return isFree(offer) ? 0 : offer.baseFeeMsat + offer.successFeeMsat + 1;
}

// Human titles for the marketplace's own offers. The raw offer id stays visible under the title.
const builtInTitles: Record<string, string> = {
	'events.query': 'Read node events',
	'execute.compute': 'Run a WebAssembly job',
	'execute.compute.generic': 'Run a compute job',
	'marketplace.provider': 'Look up a provider',
	'marketplace.receipts': 'Query execution receipts',
	'marketplace.search': 'Search the marketplace',
};

const offerDescriptions: Record<string, string> = {
	'events.query': 'Read events recorded by this Froglet node.',
	'compute.wasm.v1': 'Run a WebAssembly workload and receive a signed execution receipt.',
	'compute.execution.v1': 'Run a supported compute workload and receive a signed execution receipt.',
	'marketplace.provider': 'Look up a provider and its advertised capabilities.',
	'marketplace.receipts': 'Query execution receipts indexed by the marketplace.',
	'marketplace.search': 'Search the marketplace for providers and offers.',
};

/** The friendly title for a built-in offer, or undefined when the title is just the humanised id. */
export function builtInTitle(offer: MarketplaceOfferSummary): string | undefined {
	return offer.serviceId ? undefined : builtInTitles[offer.offerId];
}

export function offerTitle(offer: MarketplaceOfferSummary): string {
	return builtInTitle(offer) ?? serviceName(offer.serviceId || offer.offerId);
}

export function describeOffer(offer: MarketplaceOfferSummary): string {
	return offer.summary || offerDescriptions[offer.offerKind] || 'Provider-published service. Inspect its input and output contract before calling.';
}

/** "just now", "12s ago", "3m ago", "5h ago", "2d ago". */
export function formatAge(ms: number): string {
	const seconds = Math.max(0, Math.floor(ms / 1000));
	if (seconds < 5) return 'just now';
	if (seconds < 60) return `${seconds}s ago`;
	const minutes = Math.floor(seconds / 60);
	if (minutes < 60) return `${minutes}m ago`;
	const hours = Math.floor(minutes / 60);
	if (hours < 48) return `${hours}h ago`;
	return `${Math.floor(hours / 24)}d ago`;
}

/** What changed between two renders of the same listing decides whether its row flashes. */
export function offerSignature(offer: MarketplaceOfferSummary, stale = false, now = Date.now()): string {
	return [serviceAvailability(offer, stale, now).state, priceLabel(offer), offer.summary ?? '', offer.availability?.admission ?? ''].join('|');
}
