import type { MarketplaceOfferSummary, MarketplaceProviderSummary, MarketplaceSnapshot } from '../data/live-snapshot';

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

function displayEndpoint(value: string | undefined): string {
	if (!value) return 'n/a';
	try {
		return new URL(value).host;
	} catch {
		return value;
	}
}

function runtimeNames(snapshot: MarketplaceSnapshot): string {
	const runtimes = Array.from(new Set(snapshot.offers.map((offer) => offer.runtime).filter(Boolean)));
	return runtimes.length > 0 ? runtimes.map((runtime) => runtime.toUpperCase()).join(' / ') : 'NONE';
}

function avatarInitials(id: string): string {
	return compactId(id).slice(0, 2);
}

function topServiceKind(serviceKinds: string[]): string {
	return serviceKinds[0] || 'n/a';
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

function renderProviderRow(provider: MarketplaceProviderSummary): HTMLTableRowElement {
	const row = document.createElement('tr');
	row.className = 'row';
	row.dataset.marketplaceSearchRow = '';
	row.dataset.marketplaceKind = 'provider';
	row.dataset.searchText = [
		provider.providerId,
		compactId(provider.providerId),
		provider.descriptorHash,
		provider.endpoint,
		...provider.serviceKinds,
	].join(' ');

	const identity = document.createElement('td');
	identity.className = 'name';
	const avatar = document.createElement('span');
	avatar.className = 'av';
	avatar.textContent = avatarInitials(provider.providerId);
	identity.append(avatar, ` ${compactId(provider.providerId)}`);

	const endpoint = document.createElement('td');
	endpoint.className = 'endp';
	endpoint.textContent = displayEndpoint(provider.endpoint);

	const service = document.createElement('td');
	service.className = 'svc';
	service.textContent = topServiceKind(provider.serviceKinds);

	const ok = document.createElement('td');
	ok.className = 'ok';
	ok.textContent = String(provider.successCount);

	const fail = document.createElement('td');
	fail.className = 'fail';
	fail.textContent = String(provider.failureCount);

	const receipts = document.createElement('td');
	receipts.className = 'vol';
	receipts.textContent = String(provider.successCount + provider.failureCount);

	row.append(identity, endpoint, service, ok, fail, receipts);
	return row;
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

function renderProviderTable(root: ParentNode, providers: MarketplaceProviderSummary[]): void {
	const body = root.querySelector('[data-marketplace-provider-table]');
	if (!body) return;
	body.textContent = '';
	const rows = [...providers]
		.sort((a, b) => b.successCount + b.failureCount - (a.successCount + a.failureCount))
		.slice(0, 8);
	if (rows.length === 0) {
		const row = document.createElement('tr');
		const cell = document.createElement('td');
		cell.colSpan = 6;
		cell.className = 'panel-empty';
		cell.textContent = 'No providers indexed yet.';
		row.append(cell);
		body.append(row);
		return;
	}
	body.append(...rows.map(renderProviderRow));
}

function renderServicesBreakdown(root: ParentNode, offers: MarketplaceOfferSummary[]): void {
	const container = root.querySelector<HTMLElement>('[data-marketplace-services-breakdown]');
	if (!container) return;
	const counts = new Map<string, number>();
	for (const offer of offers) {
		const key = offer.runtime || offer.offerKind || 'other';
		counts.set(key, (counts.get(key) ?? 0) + 1);
	}
	const rows = Array.from(counts.entries())
		.map(([name, count]) => ({ name, count }))
		.sort((a, b) => b.count - a.count);
	container.textContent = '';
	if (rows.length === 0) {
		const empty = document.createElement('div');
		empty.className = 'panel-empty';
		empty.textContent = 'No indexed offers yet.';
		container.append(empty);
		return;
	}
	const max = rows.reduce((m, r) => Math.max(m, r.count), 0);
	for (const row of rows) {
		const wrap = document.createElement('div');
		wrap.className = 'svc-row';
		wrap.dataset.marketplaceSearchRow = '';
		wrap.dataset.marketplaceKind = 'service';
		wrap.dataset.searchText = row.name;
		const nm = document.createElement('span');
		nm.className = 'nm';
		nm.textContent = row.name;
		const bar = document.createElement('span');
		bar.className = 'bar';
		const fill = document.createElement('span');
		fill.className = 'fill';
		fill.style.width = `${max > 0 ? Math.round((row.count / max) * 100) : 0}%`;
		bar.append(fill);
		const ct = document.createElement('span');
		ct.className = 'ct';
		ct.textContent = String(row.count);
		wrap.append(nm, bar, ct);
		container.append(wrap);
	}
}

function searchableText(element: HTMLElement): string {
	return `${element.dataset.searchText || ''} ${element.textContent || ''}`.toLowerCase();
}

function initMarketplaceSearch(root: HTMLElement): () => void {
	const input = root.querySelector<HTMLInputElement>('[data-marketplace-search]');
	const count = root.querySelector<HTMLOutputElement>('[data-marketplace-search-count]');
	const form = root.querySelector<HTMLElement>('[data-marketplace-search-form]');

	if (!input) return () => {};

	const apply = () => {
		const terms = input.value
			.trim()
			.toLowerCase()
			.split(/\s+/)
			.filter(Boolean);
		const rows = Array.from(root.querySelectorAll<HTMLElement>('[data-marketplace-search-row]'));
		let shown = 0;

		for (const row of rows) {
			const visible = terms.length === 0 || terms.every((term) => searchableText(row).includes(term));
			row.hidden = !visible;
			if (visible) shown += 1;
		}

		if (count) {
			count.textContent = terms.length === 0
				? 'All indexed rows'
				: `${shown} match${shown === 1 ? '' : 'es'}`;
		}
	};

	input.addEventListener('input', apply);
	form?.addEventListener('click', () => input.focus());
	form?.addEventListener('keydown', (event) => {
		if (event.key === 'Enter' || event.key === ' ') {
			event.preventDefault();
			input.focus();
		}
	});
	form?.addEventListener('submit', (event) => {
		event.preventDefault();
		apply();
	});
	apply();
	return apply;
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
	renderProviderTable(root, snapshot.providers);
	renderOfferBook(root, snapshot.offers);
	renderServicesBreakdown(root, snapshot.offers);
}

export function initMarketplaceLive(): void {
	const root = document.querySelector<HTMLElement>('[data-marketplace-live]');
	if (!root) return;
	const applySearch = initMarketplaceSearch(root);

	const refresh = async () => {
		try {
			const response = await fetch('/api/marketplace-snapshot', {
				cache: 'no-store',
				headers: { accept: 'application/json' },
			});
			if (!response.ok) throw new Error(`HTTP ${response.status}`);
			const snapshot = await response.json() as MarketplaceSnapshot;
			renderSnapshot(root, snapshot);
			applySearch();
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
