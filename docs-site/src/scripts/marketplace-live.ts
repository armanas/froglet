import type { MarketplaceOfferSummary, MarketplaceSnapshot } from '../data/live-snapshot';

function compactId(value: string): string {
	if (value.length <= 18) return value;
	return `${value.slice(0, 10)}...${value.slice(-6)}`;
}

function formatSnapshotTime(value: string): string {
	return new Intl.DateTimeFormat('en', {
		dateStyle: 'medium',
		timeStyle: 'medium',
		timeZone: 'UTC',
	}).format(new Date(value));
}

function setText(root: ParentNode, selector: string, value: string | number): void {
	const element = root.querySelector(selector);
	if (element) element.textContent = String(value);
}

function setBar(root: ParentNode, selector: string, value: number): void {
	const element = root.querySelector<HTMLElement>(selector);
	if (element) element.style.setProperty('--bar', `${value}%`);
}

function compactEndpoint(value: string | undefined): string {
	if (!value) return 'NONE';
	try {
		return new URL(value).host.toUpperCase();
	} catch {
		return value.toUpperCase();
	}
}

function runtimeNames(snapshot: MarketplaceSnapshot): string {
	const runtimes = Array.from(new Set(snapshot.offers.map((offer) => offer.runtime).filter(Boolean)));
	return runtimes.length > 0 ? runtimes.map((runtime) => runtime.toUpperCase()).join(' / ') : 'NONE';
}

function serviceKindSummary(serviceKinds: string[]): string {
	if (serviceKinds.length === 0) return 'NONE';
	const groups = Array.from(new Set(serviceKinds.map((kind) => {
		if (kind.includes('compute')) return 'COMPUTE';
		if (kind.includes('demo')) return 'DEMO';
		if (kind.includes('events')) return 'EVENTS';
		return kind.split('.')[0]?.toUpperCase() || 'OTHER';
	})));
	return `${serviceKinds.length} KINDS / ${groups.join(' / ')}`;
}

function renderOfferRow(offer: MarketplaceOfferSummary): HTMLTableRowElement {
	const row = document.createElement('tr');
	for (const value of [
		offer.offerId,
		offer.runtime || 'n/a',
		offer.settlementMethod || 'n/a',
		String(offer.baseFeeMsat + offer.successFeeMsat),
		compactId(offer.providerId),
	]) {
		const cell = document.createElement('td');
		cell.textContent = value;
		row.append(cell);
	}
	return row;
}

function renderOfferBook(root: ParentNode, offers: MarketplaceOfferSummary[]): void {
	const body = root.querySelector('[data-marketplace-offer-book]');
	if (!body) return;
	body.textContent = '';
	if (offers.length === 0) {
		const row = document.createElement('tr');
		const cell = document.createElement('td');
		cell.colSpan = 5;
		cell.textContent = 'NO OFFERS';
		row.append(cell);
		body.append(row);
		return;
	}
	body.append(...offers.slice(0, 6).map(renderOfferRow));
}

function renderSnapshot(root: HTMLElement, snapshot: MarketplaceSnapshot): void {
	const successCount = snapshot.providers.reduce((sum, provider) => sum + provider.successCount, 0);
	const failureCount = snapshot.providers.reduce((sum, provider) => sum + provider.failureCount, 0);
	const totalReceipts = successCount + failureCount;
	const freeOffers = snapshot.offers.filter(
		(offer) => offer.settlementMethod === 'none' && offer.baseFeeMsat === 0 && offer.successFeeMsat === 0,
	).length;
	const paidOffers = Math.max(0, snapshot.offerCount - freeOffers);
	const freeShare = snapshot.offerCount === 0 ? 0 : Math.round((freeOffers / snapshot.offerCount) * 100);
	const primaryProvider = snapshot.providers[0];
	const endpointCount = snapshot.providers.filter((provider) => provider.endpoint.length > 0).length;
	const totalSettledMsat = snapshot.providers.reduce((sum, provider) => sum + provider.totalSettledMsat, 0);
	const offerNames = snapshot.offers.slice(0, 5).map((offer) => offer.offerId.toUpperCase()).join('   ') || 'NO OFFERS';
	const runtimeCount = Array.from(new Set(snapshot.offers.map((offer) => offer.runtime).filter(Boolean))).length;
	const serviceKinds = primaryProvider ? serviceKindSummary(primaryProvider.serviceKinds) : 'NONE';

	root.dataset.status = snapshot.status;
	setText(root, '[data-marketplace-field="status"]', snapshot.status === 'pass' ? 'READ API ONLINE' : 'READ API DOWN');
	setText(root, '[data-marketplace-field="froglets"]', snapshot.providerCount);
	setText(root, '[data-marketplace-field="offers"]', snapshot.offerCount);
	setText(root, '[data-marketplace-field="checkedAt"]', `${formatSnapshotTime(snapshot.checkedAt)} UTC`);
	setText(root, '[data-marketplace-field="detail"]', snapshot.detail);
	setText(root, '[data-marketplace-field="freeOffers"]', freeOffers);
	setText(root, '[data-marketplace-field="paidOffers"]', paidOffers);
	setText(root, '[data-marketplace-field="freeShare"]', `${freeShare}%`);
	setText(root, '[data-marketplace-field="successRate"]', totalReceipts === 0 ? 'N/A' : `${Math.round((successCount / totalReceipts) * 100)}%`);
	setText(root, '[data-marketplace-field="receipts"]', totalReceipts);
	setText(root, '[data-marketplace-field="receiptsLabel"]', `${totalReceipts} RECEIPTS`);
	setText(root, '[data-marketplace-field="successCount"]', successCount);
	setText(root, '[data-marketplace-field="failureCount"]', failureCount);
	setText(root, '[data-marketplace-field="settledMsat"]', `${totalSettledMsat} MSAT`);
	setText(root, '[data-marketplace-field="dealFeedStatus"]', snapshot.dealFeed.status.toUpperCase());
	setText(root, '[data-marketplace-field="dealFeedDetail"]', snapshot.dealFeed.detail);
	setText(root, '[data-marketplace-field="dealFeedCount"]', snapshot.dealFeed.deals.length);
	setText(root, '[data-marketplace-field="runtimeCount"]', runtimeCount);
	setText(root, '[data-marketplace-field="primaryProvider"]', primaryProvider ? compactId(primaryProvider.providerId).toUpperCase() : 'NONE');
	setText(root, '[data-marketplace-field="primaryEndpoint"]', compactEndpoint(primaryProvider?.endpoint));
	setText(root, '[data-marketplace-field="descriptorHash"]', primaryProvider?.descriptorHash.toUpperCase() ?? 'NONE');
	setText(root, '[data-marketplace-field="endpointCount"]', endpointCount);
	setText(root, '[data-marketplace-field="runtimeNames"]', runtimeNames(snapshot));
	setText(root, '[data-marketplace-field="serviceKinds"]', serviceKinds);
	setText(root, '[data-marketplace-field="offerNames"]', offerNames);
	setText(root, '[data-marketplace-field="ticker"]', offerNames);
	setBar(root, '.terminal-meter', freeShare);
	renderOfferBook(root, snapshot.offers);
}

export function initMarketplaceLive(): void {
	const root = document.querySelector<HTMLElement>('[data-marketplace-live]');
	if (!root) return;

	const refresh = async () => {
		try {
			const response = await fetch('/api/marketplace-snapshot', {
				cache: 'no-store',
				headers: { accept: 'application/json' },
			});
			if (!response.ok) throw new Error(`HTTP ${response.status}`);
			const snapshot = await response.json() as MarketplaceSnapshot;
			renderSnapshot(root, snapshot);
			setText(root, '[data-marketplace-field="refresh"]', 'LIVE');
		} catch (error) {
			setText(
				root,
				'[data-marketplace-field="refresh"]',
				`STATIC ${error instanceof Error ? error.message : String(error)}`,
			);
		}
	};

	void refresh();
	window.setInterval(refresh, 30_000);
}
