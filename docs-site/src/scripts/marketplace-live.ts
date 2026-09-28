import type { MarketplaceOfferSummary, MarketplaceProviderSummary, MarketplaceSnapshot } from '../data/live-snapshot';
import { serviceName } from '../data/service-presentation';

export function serviceAvailability(offer: MarketplaceOfferSummary, stale = false, now = Date.now()): { label: string; ready: boolean } {
	if (stale) return { label: 'Status needs refresh', ready: false };
	const a = offer.availability;
	if (a?.status === 'healthy' && a.leaseExpiresAt * 1000 > now && a.lastCheckedAt > 0 && a.lastCheckedAt * 1000 <= now + 60_000) return { label: 'Recently checked', ready: true };
	if (a?.leaseExpiresAt && a.leaseExpiresAt * 1000 <= now) return { label: 'Check expired', ready: false };
	return { label: a?.status === 'offline' ? 'Offline' : 'Availability not confirmed', ready: false };
}

const offerDescriptions: Record<string, string> = {
	'events.query': 'Read events recorded by this Froglet node.',
	'compute.wasm.v1': 'Run a WebAssembly workload and receive a signed execution receipt.',
	'compute.execution.v1': 'Run a supported compute workload and receive a signed execution receipt.',
	'marketplace.provider': 'Look up a provider and its advertised capabilities.',
	'marketplace.receipts': 'Query execution receipts indexed by the marketplace.',
	'marketplace.search': 'Search the marketplace for providers and offers.',
};

function renderServiceCards(root: HTMLElement, offers: MarketplaceOfferSummary[], stale = false): void {
	const container = root.querySelector('[data-marketplace-service-cards]');
	if (!container) return;
	container.replaceChildren();
	const sorted = [...offers].sort((a, b) => Number(serviceAvailability(b, stale).ready) - Number(serviceAvailability(a, stale).ready));
	for (const offer of sorted) {
		const availability = serviceAvailability(offer, stale);
		const free = offer.pricingKnown !== false && offer.settlementMethod === 'none' && offer.baseFeeMsat === 0 && offer.successFeeMsat === 0;
		const card = document.createElement('article');
		card.className = 'service-card';
		card.dataset.marketplaceSearchRow = '';
		card.dataset.marketplaceKind = 'offer';
		card.dataset.ready = String(availability.ready);
		card.dataset.free = String(free);
		card.dataset.searchText = `${offer.offerId} ${offer.providerId} ${offer.runtime} ${offer.summary || ''}`;
		const top = document.createElement('div'); top.className = 'service-card-top';
		const badge = document.createElement('span'); badge.className = 'service-availability'; badge.dataset.ready = String(availability.ready); badge.textContent = availability.label;
		const price = document.createElement('strong'); price.textContent = offer.pricingKnown === false ? 'Price unavailable' : free ? 'Free' : `${offer.baseFeeMsat / 1000} + ${offer.successFeeMsat / 1000} sats`;
		top.append(badge, price);
		const heading = document.createElement('h3'); heading.textContent = serviceName(offer.serviceId || offer.offerId);
		const description = document.createElement('p'); description.className = 'service-description';
		description.textContent = offer.summary || offerDescriptions[offer.offerKind] || 'Provider-published service. Inspect its input and output contract before calling.';
		const identity = document.createElement('p'); identity.className = 'service-meta'; identity.textContent = `Provider ${compactId(offer.providerId)}`; identity.title = offer.providerId;
		const observed = document.createElement('p'); observed.className = 'service-meta';
		observed.textContent = offer.availability?.lastCheckedAt ? `Provider check: ${formatSnapshotTime(new Date(offer.availability.lastCheckedAt * 1000).toISOString())} UTC` : 'No recent provider check in the catalog.';
		const actions = document.createElement('div'); actions.className = 'service-actions';
		const validPath = offer.sharePath && /^\/s\/[a-f0-9]{64}\/[a-zA-Z0-9][a-zA-Z0-9._-]{0,127}$/.test(offer.sharePath);
		const link = document.createElement('a'); link.className = 'service-open';
		link.href = validPath ? offer.sharePath! : '/marketplace/overview/';
		link.textContent = validPath ? 'Open service →' : 'How to use this offer →';
		actions.append(link);
		if (validPath) {
			const share = document.createElement('a'); share.href = `${offer.sharePath}#share`; share.textContent = 'Share / QR';
			actions.append(share);
		}
		card.append(top, heading, description, identity, observed);
		if (offer.availability?.admission === 'invitation_required') {
			const access = document.createElement('p'); access.className = 'service-meta';
			access.textContent = 'Invitation required. Listing checks metadata only; execution has not been tested by the marketplace.';
			card.append(access);
		}
		if (!free && offer.pricingKnown !== false) { const terms = document.createElement('p'); terms.className = 'service-meta'; terms.textContent = 'Price is base + success fee; inspect payment terms before use.'; card.append(terms); }
		card.append(actions); container.append(card);
	}
	if (!offers.length) { const empty = document.createElement('p'); empty.textContent = 'No services are listed in this snapshot.'; container.append(empty); }
	setText(root, '[data-marketplace-field="recentServices"]', offers.filter(offer => serviceAvailability(offer, stale).ready).length);
}

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
	if (!value) return 'No public HTTPS endpoint';
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

function providerEvidence(provider: MarketplaceProviderSummary): string {
	return [
		`provider_id: ${provider.providerId}`,
		`descriptor_hash: ${provider.descriptorHash}`,
		`endpoint: ${provider.endpoint || 'n/a'}`,
		`services: ${provider.serviceKinds.join(', ') || 'n/a'}`,
		`ok: ${provider.successCount}`,
		`fail: ${provider.failureCount}`,
		`receipts: ${provider.successCount + provider.failureCount}`,
		`settled_msat: ${provider.totalSettledMsat}`,
	].join('\n');
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
	row.dataset.providerSummary = providerEvidence(provider);
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
	row.dataset.marketplaceSearchRow = '';
	row.dataset.searchText = `${offer.offerId} ${offer.providerId} ${offer.artifactHash} ${offer.runtime}`;
	row.title = `artifact_hash: ${offer.artifactHash}`;
	for (const value of [
		offer.offerId,
		offer.runtime || 'n/a',
		offer.settlementMethod || 'n/a',
		offer.pricingKnown === false ? 'Price unavailable' : String(offer.baseFeeMsat + offer.successFeeMsat),
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
		const filter = root.querySelector<HTMLSelectElement>('[data-marketplace-filter]')?.value || 'all';
		let shown = 0;

		for (const row of rows) {
			const matches = terms.length === 0 || terms.every((term) => searchableText(row).includes(term));
			const visible = matches && (row.dataset.marketplaceKind !== 'offer' || filter === 'all' || (filter === 'ready' && row.dataset.ready === 'true') || (filter === 'free' && row.dataset.free === 'true'));
			row.hidden = !visible;
			if (visible && row.dataset.marketplaceKind === 'offer') shown += 1;
		}

		const empty = root.querySelector<HTMLElement>('[data-marketplace-no-results]');
		if (empty) empty.hidden = shown !== 0 || root.dataset.catalogLoaded !== 'true';
		if (count) {
			count.textContent = root.dataset.catalogLoaded === 'true' ? `${shown} service${shown === 1 ? '' : 's'}` : 'Loading';
		}
	};

	input.addEventListener('input', apply);
	root.querySelector('[data-marketplace-filter]')?.addEventListener('change', apply);
	form?.addEventListener('click', () => input.focus());
	form?.addEventListener('submit', (event) => {
		event.preventDefault();
		apply();
	});
	apply();
	return apply;
}

function fieldText(root: ParentNode, field: string): string {
	const element = root.querySelector(`[data-marketplace-field="${field}"]`);
	return element?.textContent?.replace(/\s+/g, ' ').trim() || 'unavailable';
}

function marketplaceEvidence(root: HTMLElement): string {
	return [
		'Froglet marketplace evidence',
		`status: ${root.dataset.status}`,
		`scope: sampled provider receipts and offer pricing; provider/offer counts are catalog totals`,
		`checked_at: ${fieldText(root, 'checkedAt')}`,
		`providers: ${fieldText(root, 'froglets')}`,
		`offers: ${fieldText(root, 'offers')}`,
		`free_offers: ${fieldText(root, 'freeOffers')}`,
		`priced_offers: ${fieldText(root, 'paidOffers')}`,
		`receipts: ${fieldText(root, 'receipts')}`,
		`success_rate: ${fieldText(root, 'successRate')}`,
		`indexed_receipt_value: ${fieldText(root, 'settledSats')}`,
		`detail: ${fieldText(root, 'detail')}`,
		'sources:',
		'- https://marketplace.froglet.dev/v1/providers?limit=12',
		'- https://marketplace.froglet.dev/v1/offers?limit=24',
		'not_proved: hosted paid rails, mainnet money movement, or production marketplace depth beyond observed public data',
	].join('\n');
}

function fallbackCopy(text: string): boolean {
	const textarea = document.createElement('textarea');
	textarea.value = text;
	textarea.setAttribute('readonly', '');
	textarea.style.position = 'fixed';
	textarea.style.left = '-9999px';
	textarea.style.top = '0';
	document.body.appendChild(textarea);
	textarea.select();
	const copied = document.execCommand('copy');
	textarea.remove();
	return copied;
}

async function copyText(text: string): Promise<void> {
	try {
		await navigator.clipboard.writeText(text);
		return;
	} catch {}

	if (!fallbackCopy(text)) throw new Error('copy failed');
}

function markCopyButton(button: HTMLButtonElement, label: string): void {
	const original = button.dataset.copyLabel || button.textContent || 'Copy';
	button.dataset.copyLabel = original;
	button.textContent = label;
	window.setTimeout(() => {
		button.textContent = original;
	}, 1600);
}

function initMarketplaceEvidenceActions(root: HTMLElement): void {
	root.querySelector<HTMLButtonElement>('[data-marketplace-copy-summary]')?.addEventListener('click', async (event) => {
		const button = event.currentTarget;
		if (!(button instanceof HTMLButtonElement)) return;
		try {
			await copyText(marketplaceEvidence(root));
			markCopyButton(button, 'Copied');
		} catch {
			markCopyButton(button, 'Copy failed');
		}
	});

	root.querySelector<HTMLButtonElement>('[data-marketplace-copy-provider]')?.addEventListener('click', async (event) => {
		const button = event.currentTarget;
		if (!(button instanceof HTMLButtonElement)) return;
		const row = root.querySelector<HTMLElement>('[data-provider-summary]:not([hidden])')
			?? root.querySelector<HTMLElement>('[data-provider-summary]');
		const summary = row?.dataset.providerSummary || 'No provider row available in the current marketplace snapshot.';
		try {
			await copyText(`Froglet top provider evidence\n${summary}`);
			markCopyButton(button, 'Copied');
		} catch {
			markCopyButton(button, 'Copy failed');
		}
	});
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
	body.append(...offers.map(renderOfferRow));
}

function renderSnapshot(root: HTMLElement, snapshot: MarketplaceSnapshot): void {
	root.dataset.catalogLoaded = 'true';
	const successCount = snapshot.providers.reduce((sum, provider) => sum + provider.successCount, 0);
	const failureCount = snapshot.providers.reduce((sum, provider) => sum + provider.failureCount, 0);
	const totalReceipts = successCount + failureCount;
	const freeOffers = snapshot.offers.filter(
		(offer) => offer.pricingKnown !== false && offer.settlementMethod === 'none' && offer.baseFeeMsat === 0 && offer.successFeeMsat === 0,
	).length;
	const paidOffers = snapshot.offers.filter(offer => offer.pricingKnown !== false).length - freeOffers;
	const freeShare = snapshot.offers.length === 0 ? 0 : Math.round((freeOffers / snapshot.offers.length) * 100);
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
	setText(root, '[data-marketplace-field="detail"]', `${snapshot.detail} Receipt totals cover ${snapshot.providers.length} sampled providers; pricing and runtime breakdown cover ${snapshot.offers.length} sampled offers.`);
	setText(root, '[data-marketplace-field="freeOffers"]', freeOffers);
	setText(root, '[data-marketplace-field="paidOffers"]', paidOffers);
	setText(root, '[data-marketplace-field="freeShare"]', `${freeShare}%`);
	setText(root, '[data-marketplace-field="successRate"]', totalReceipts === 0 ? 'N/A' : `${Math.round((successCount / totalReceipts) * 100)}%`);
	setText(root, '[data-marketplace-field="receipts"]', totalReceipts);
	setText(root, '[data-marketplace-field="receiptsLabel"]', `${totalReceipts} RECEIPTS`);
	setText(root, '[data-marketplace-field="successCount"]', successCount);
	setText(root, '[data-marketplace-field="failureCount"]', failureCount);
	setText(root, '[data-marketplace-field="settledMsat"]', `${totalSettledMsat} MSAT`);
	setText(root, '[data-marketplace-field="settledSats"]', `${Math.round(totalSettledMsat / 1000)}sats`);
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
	renderServiceCards(root, snapshot.offers, Date.now() - Date.parse(snapshot.checkedAt) > 90_000);
	renderProviderTable(root, snapshot.providers);
	renderOfferBook(root, snapshot.offers);
	renderServicesBreakdown(root, snapshot.offers);
}

export function initMarketplaceLive(): void {
	const root = document.querySelector<HTMLElement>('[data-marketplace-live]');
	if (!root) return;
	const applySearch = initMarketplaceSearch(root);
	initMarketplaceEvidenceActions(root);
	let lastSnapshot: MarketplaceSnapshot | undefined;
	let refreshing = false;

	function state(status: 'live' | 'stale' | 'unavailable', detail?: string) {
		root!.dataset.status = status;
		setText(root!, '[data-marketplace-field="message"]', status === 'live'
			? 'Catalog updated. Open a service to see what it does.'
			: status === 'stale'
				? 'Showing an earlier catalog. Availability may have changed; we are checking again.'
				: 'We could not load the catalog. We will try again automatically.');
		const badge = document.querySelector<HTMLElement>('[data-marketplace-field="refresh"]');
		if (badge) { badge.textContent = status === 'live' ? 'CATALOG UPDATED' : status.toUpperCase(); badge.dataset.status = status; }
		if (lastSnapshot) { renderServiceCards(root!, lastSnapshot.offers, status !== 'live'); applySearch(); }
		if (detail) setText(root!, '[data-marketplace-field="detail"]', detail);
		if (!lastSnapshot) {
			const cards = root!.querySelector('[data-marketplace-service-cards]');
			if (cards) cards.textContent = 'The catalog is unavailable. Retrying automatically.';
			setText(root!, '[data-marketplace-search-count]', 'Unavailable');
			for (const field of ['froglets', 'offers', 'freeOffers', 'paidOffers', 'receipts', 'successRate', 'settledSats']) {
				setText(root!, `[data-marketplace-field="${field}"]`, '—');
			}
			for (const selector of ['[data-marketplace-provider-table]', '[data-marketplace-offer-book]']) {
				const body = root!.querySelector(selector);
				if (!body) continue;
				const row = document.createElement('tr');
				const cell = document.createElement('td');
				cell.colSpan = 6; cell.className = 'panel-empty'; cell.textContent = 'Catalog unavailable. Retrying automatically.';
				row.append(cell); body.replaceChildren(row);
			}
		}
	}
	const refresh = async () => {
		if (refreshing) return;
		refreshing = true;
		try {
			const response = await fetch('/api/marketplace-snapshot', {
				cache: 'no-store', signal: AbortSignal.timeout(12_000), headers: { accept: 'application/json' },
			});
			const snapshot = await response.json() as MarketplaceSnapshot;
			if (!response.ok || snapshot.status !== 'pass') throw new Error(snapshot.detail || `HTTP ${response.status}`);
			const age = Date.now() - Date.parse(snapshot.checkedAt);
			if (!Number.isFinite(age) || age < -60_000 || !Array.isArray(snapshot.providers) || !Array.isArray(snapshot.offers)) {
				throw new Error('Invalid marketplace snapshot');
			}
			renderSnapshot(root, snapshot);
			lastSnapshot = snapshot;
			state(age > 90_000 ? 'stale' : 'live');
			applySearch();
		} catch (error) {
			state(lastSnapshot ? 'stale' : 'unavailable',
				`${lastSnapshot ? 'Showing the last successful snapshot. ' : ''}${error instanceof Error ? error.message : String(error)}`);
		} finally { refreshing = false; }
	};
	void refresh();
	window.setInterval(() => {
		if (lastSnapshot && Date.now() - Date.parse(lastSnapshot.checkedAt) > 90_000) state('stale');
		void refresh();
	}, 30_000);
}
