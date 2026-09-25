export type ProbeState = 'pass' | 'fail';

export interface MarketplaceProviderSummary {
	providerId: string;
	descriptorHash: string;
	serviceKinds: string[];
	executionRuntimes: string[];
	endpoint: string;
	successCount: number;
	failureCount: number;
	totalSettledMsat: number;
	lastReceiptFinishedAt: number;
}

export interface MarketplaceOfferSummary {
	providerId: string;
	offerId: string;
	offerKind: string;
	runtime: string;
	packageKind: string;
	settlementMethod: string;
	baseFeeMsat: number;
	successFeeMsat: number;
	artifactHash: string;
}

export type DealFeedState = 'pass' | 'pending' | 'fail';

export interface MarketplaceDealSummary {
	dealId: string;
	providerId: string;
	offerId: string;
	status: string;
	settlementMethod: string;
	baseFeeMsat: number;
	successFeeMsat: number;
	hasReceipt: boolean;
	updatedAt: number;
}

export interface MarketplaceDealFeedSnapshot {
	status: DealFeedState;
	detail: string;
	deals: MarketplaceDealSummary[];
}

export interface MarketplaceSnapshot {
	checkedAt: string;
	status: ProbeState;
	detail: string;
	providerCount: number;
	offerCount: number;
	providers: MarketplaceProviderSummary[];
	offers: MarketplaceOfferSummary[];
	dealFeed: MarketplaceDealFeedSnapshot;
}

const MARKETPLACE_URL = 'https://marketplace.froglet.dev';

const timeoutMs = 4_000;

function nowIso(): string {
	return new Date().toISOString();
}

export function formatSnapshotTime(value: string): string {
	return new Intl.DateTimeFormat('en', {
		dateStyle: 'medium',
		timeStyle: 'medium',
		timeZone: 'UTC',
	}).format(new Date(value));
}

export function compactId(value: string): string {
	if (value.length <= 18) return value;
	return `${value.slice(0, 10)}...${value.slice(-6)}`;
}

async function fetchWithTimeout(url: string): Promise<Response> {
	const controller = new AbortController();
	const timer = setTimeout(() => controller.abort(), timeoutMs);
	try {
		return await fetch(url, {
			signal: controller.signal,
			headers: {
				accept: 'application/json,text/plain,text/html;q=0.9,*/*;q=0.8',
			},
		});
	} finally {
		clearTimeout(timer);
	}
}

function asRecord(value: unknown): Record<string, unknown> {
	return value && typeof value === 'object' ? value as Record<string, unknown> : {};
}

function asArray(value: unknown): unknown[] {
	return Array.isArray(value) ? value : [];
}

function asStringArray(value: unknown): string[] {
	return asArray(value).filter((item): item is string => typeof item === 'string');
}

function asNumber(value: unknown): number {
	return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}

function emptyDealFeed(detail = 'A public deal feed is not available. Catalog counts do not measure customer activity.'): MarketplaceDealFeedSnapshot {
	return {
		status: 'pending',
		detail,
		deals: [],
	};
}

async function getMarketplaceDealFeed(marketplaceUrl: string): Promise<MarketplaceDealFeedSnapshot> {
	try {
		const response = await fetchWithTimeout(`${marketplaceUrl}/v1/deals?limit=10`);
		if (response.status === 404 || response.status === 405) {
			return emptyDealFeed();
		}
		if (!response.ok) {
			return {
				status: 'fail',
				detail: `Public deal feed returned HTTP ${response.status}.`,
				deals: [],
			};
		}
		const body = asRecord(await response.json());

		const deals = asArray(body.items).map((item): MarketplaceDealSummary => {
			const row = asRecord(item);
			return {
				dealId: String(row.deal_id ?? row.deal_hash ?? ''),
				providerId: String(row.provider_id ?? ''),
				offerId: String(row.offer_id ?? ''),
				status: String(row.status ?? ''),
				settlementMethod: String(row.settlement_method ?? ''),
				baseFeeMsat: asNumber(row.base_fee_msat),
				successFeeMsat: asNumber(row.success_fee_msat),
				hasReceipt: row.has_receipt === true,
				updatedAt: asNumber(row.updated_at),
			};
		});

		return {
			status: 'pass',
			detail: `Public deal feed returned ${deals.length} redacted deal(s).`,
			deals,
		};
	} catch (error) {
		return {
			status: 'fail',
			detail: error instanceof Error ? error.message : String(error),
			deals: [],
		};
	}
}

export async function getMarketplaceSnapshot(marketplaceUrl = MARKETPLACE_URL): Promise<MarketplaceSnapshot> {
	const checkedAt = nowIso();
	try {
		if (!/^https:\/\/marketplace(?:-[a-z0-9]+)*\.froglet\.dev$/.test(marketplaceUrl)) throw new Error('Invalid first-party marketplace origin.');
		const healthResponse = await fetchWithTimeout(`${marketplaceUrl}/healthz`);
		if (!healthResponse.ok) throw new Error(`Marketplace is unavailable (HTTP ${healthResponse.status}). Retrying automatically.`);
		const health = asRecord(await healthResponse.json());
		if (!healthResponse.ok || health.status !== 'ok') {
			return {
				checkedAt,
				status: 'fail',
				detail: `Marketplace health returned HTTP ${healthResponse.status}`,
				providerCount: 0,
				offerCount: 0,
				providers: [],
				offers: [],
				dealFeed: emptyDealFeed(),
			};
		}

		const [providersResponse, offersResponse, dealFeed] = await Promise.all([
			fetchWithTimeout(`${marketplaceUrl}/v1/providers?limit=12`),
			fetchWithTimeout(`${marketplaceUrl}/v1/offers?limit=24`),
			getMarketplaceDealFeed(marketplaceUrl),
		]);
		if (!providersResponse.ok || !offersResponse.ok) throw new Error('Marketplace catalog is unavailable. Retrying automatically.');
		const providersBody = asRecord(await providersResponse.json());
		const offersBody = asRecord(await offersResponse.json());
		if (!providersResponse.ok || !offersResponse.ok || !Array.isArray(providersBody.items) || !Array.isArray(offersBody.items)) {
			throw new Error('Marketplace returned an unavailable or malformed catalog.');
		}
		const providerItems = providersBody.items;
		const offerItems = asArray(offersBody.items);
		const providerCount = asNumber(asRecord(providersBody.pagination).total) || providerItems.length;
		const offerCount = asNumber(asRecord(offersBody.pagination).total) || offerItems.length;

		const providers = providerItems.map((item): MarketplaceProviderSummary => {
			const row = asRecord(item);
			const descriptor = asRecord(row.descriptor);
			const trust = asRecord(row.trust);
			const endpoints = asArray(descriptor.transport_endpoints);
			const firstEndpoint = asRecord(endpoints[0]);
			return {
				providerId: String(row.provider_id ?? ''),
				descriptorHash: String(row.current_descriptor_hash ?? descriptor.artifact_hash ?? ''),
				serviceKinds: asStringArray(descriptor.service_kinds),
				executionRuntimes: asStringArray(descriptor.execution_runtimes),
				endpoint: String(firstEndpoint.uri ?? ''),
				successCount: asNumber(trust.success_count),
				failureCount: asNumber(trust.failure_count),
				totalSettledMsat: asNumber(trust.total_settled_msat),
				lastReceiptFinishedAt: asNumber(trust.last_receipt_finished_at),
			};
		});

		const offers = offerItems.map((item): MarketplaceOfferSummary => {
			const row = asRecord(item);
			return {
				providerId: String(row.provider_id ?? ''),
				offerId: String(row.offer_id ?? ''),
				offerKind: String(row.offer_kind ?? ''),
				runtime: String(row.runtime ?? ''),
				packageKind: String(row.package_kind ?? ''),
				settlementMethod: String(row.settlement_method ?? ''),
				baseFeeMsat: asNumber(row.base_fee_msat),
				successFeeMsat: asNumber(row.success_fee_msat),
				artifactHash: String(row.artifact_hash ?? ''),
			};
		});

		return {
			checkedAt,
			status: providersResponse.ok && offersResponse.ok ? 'pass' : 'fail',
			detail: `Read API returned ${providerCount} provider(s) and ${offerCount} offer(s).`,
			providerCount,
			offerCount,
			providers,
			offers,
			dealFeed,
		};
	} catch (error) {
		const detail = error instanceof SyntaxError ? 'Marketplace returned an unreadable response. Retrying automatically.'
			: error instanceof Error ? error.message : 'Marketplace is unavailable. Retrying automatically.';
		return {
			checkedAt,
			status: 'fail',
			detail,
			providerCount: 0,
			offerCount: 0,
			providers: [],
			offers: [],
			dealFeed: emptyDealFeed(),
		};
	}
}
