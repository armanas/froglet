import type { MarketplaceOfferSummary, MarketplaceProviderSummary, MarketplaceSnapshot } from '../data/live-snapshot';
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
	serviceAvailability,
	type Category,
} from './marketplace-model';

export { serviceAvailability } from './marketplace-model';

const REFRESH_MS = 30_000;
const STALE_MS = 90_000;

// ── DOM helpers ──────────────────────────────────────────────

function h<K extends keyof HTMLElementTagNameMap>(tag: K, className = '', text = ''): HTMLElementTagNameMap[K] {
	const node = document.createElement(tag);
	if (className) node.className = className;
	if (text) node.textContent = text;
	return node;
}

function setText(root: ParentNode, selector: string, value: string | number): void {
	const element = root.querySelector(selector);
	if (element) element.textContent = String(value);
}

function compactId(value: string): string {
	if (value.length <= 18) return value;
	return `${value.slice(0, 10)}...${value.slice(-6)}`;
}

function formatSnapshotTime(value: string): string {
	return new Intl.DateTimeFormat('en', { dateStyle: 'medium', timeStyle: 'medium', timeZone: 'UTC' }).format(new Date(value));
}

/** ISO string for a unix-seconds value, or '' when it is missing or not a valid date. */
function isoFromSeconds(seconds: number | undefined): string {
	if (!seconds || !Number.isFinite(seconds) || seconds <= 0) return '';
	const date = new Date(seconds * 1000);
	return Number.isNaN(date.getTime()) ? '' : date.toISOString();
}

function displayEndpoint(value: string | undefined): string {
	if (!value) return 'No public HTTPS endpoint';
	try {
		return new URL(value).host;
	} catch {
		return value;
	}
}

// ── Service listing ──────────────────────────────────────────

interface RenderContext {
	providers: Map<string, MarketplaceProviderSummary>;
	stale: boolean;
	now: number;
}

let itemCounter = 0;

function buildDetail(offer: MarketplaceOfferSummary, ctx: RenderContext, id: string, title: string, link: string): HTMLTableRowElement {
	const provider = ctx.providers.get(offer.providerId);
	const availability = serviceAvailability(offer, ctx.stale, ctx.now);
	const facts = h('dl', 'mkt-facts');
	const fact = (label: string, value: string | undefined, copy = false): void => {
		if (!value) return;
		const wrap = h('div');
		const dd = h('dd');
		dd.append(h('span', 'mkt-fact-value', value));
		if (copy) {
			const button = h('button', 'mkt-mini', 'Copy');
			button.type = 'button';
			button.dataset.copyValue = value;
			button.setAttribute('aria-label', `Copy ${label.toLowerCase()}`);
			dd.append(button);
		}
		wrap.append(h('dt', '', label), dd);
		facts.append(wrap);
	};
	const admission = offer.availability?.admission && offer.availability.admission !== 'unknown' ? ` · ${offer.availability.admission.replaceAll('_', ' ')}` : '';
	const leaseIso = isoFromSeconds(offer.availability?.leaseExpiresAt);
	const checkedIso = isoFromSeconds(offer.availability?.lastCheckedAt);

	fact('Offer ID', offer.offerId, true);
	fact('Kind', offer.offerKind);
	fact('Runtime', offer.runtime);
	fact('Package', offer.packageKind);
	fact('Settlement', offer.settlementMethod);
	fact('Fees', offer.pricingKnown === false ? 'Not published' : `Base ${offer.baseFeeMsat} msat · success ${offer.successFeeMsat} msat`);
	fact('Provider', offer.providerId, true);
	if (provider?.endpoint) fact('Endpoint', displayEndpoint(provider.endpoint));
	fact('Availability', `${availability.label}${admission}`);
	if (checkedIso) fact('Last provider check', `${formatSnapshotTime(checkedIso)} UTC`);
	if (leaseIso) fact('Check valid until', `${formatSnapshotTime(leaseIso)} UTC`);
	fact('Artifact hash', offer.artifactHash, true);

	const side = h('div', 'mkt-detail__side');
	const links: Array<[string, string]> = [];
	const demoLink = publicDemoLink(offer);
	if (demoLink) links.push(['Try it', demoLink]);
	if (link) links.push([`Open ${title}`, link], ['Share / QR', `${link}#share`]);
	links.push(['How offers work', '/marketplace/overview/'], ['Verify a receipt', '/verify-receipt/']);
	for (const [label, href] of links) {
		const anchor = h('a', '', label);
		anchor.setAttribute('href', href);
		if (href === demoLink) {
			anchor.dataset.marketplacePublicDemo = '';
			anchor.setAttribute('aria-label', `Try ${title} in the public beta`);
		}
		side.append(anchor);
	}

	const row = h('tr', 'mkt-detail');
	row.id = id;
	row.hidden = true;
	const cell = h('td');
	cell.colSpan = 5;
	const grid = h('div', 'mkt-detail__grid');
	grid.append(facts, side);
	cell.append(grid);
	row.append(cell);
	return row;
}

function buildItem(offer: MarketplaceOfferSummary, ctx: RenderContext): HTMLTableSectionElement {
	const availability = serviceAvailability(offer, ctx.stale, ctx.now);
	const free = isFree(offer);
	const category = categoryOf(offer);
	const title = offerTitle(offer);
	const description = describeOffer(offer);
	const provider = ctx.providers.get(offer.providerId);
	const providerText = provider?.endpoint ? displayEndpoint(provider.endpoint) : compactId(offer.providerId);
	const link = hasShareLink(offer) ? offer.sharePath! : '';
	const demoLink = publicDemoLink(offer);
	const price = priceLabel(offer);
	const detailId = `mkt-detail-${++itemCounter}`;

	const item = h('tbody', 'mkt-item');
	item.dataset.marketplaceSearchRow = '';
	item.dataset.marketplaceKind = 'offer';
	item.dataset.ready = String(availability.ready);
	item.dataset.free = String(free);
	item.dataset.category = category;
	item.dataset.key = `${offer.providerId}:${offer.offerId}`;
	item.dataset.signature = offerSignature(offer, ctx.stale, ctx.now);
	item.dataset.sortName = title.toLowerCase();
	item.dataset.sortCategory = String(CATEGORY_ORDER.indexOf(category));
	item.dataset.sortPrice = String(priceSortKey(offer));
	item.dataset.sortStatus = String(availability.state === 'ready' ? 0 : availability.state === 'unknown' ? 1 : 2);
	item.dataset.searchText = [offer.offerId, title, offer.providerId, providerText, offer.runtime, offer.offerKind, CATEGORY_LABELS[category], description]
		.filter(Boolean)
		.join(' ');

	// Service: icon, title, one-line description and the notes a reader must not miss.
	const service = h('td', 'mkt-col-service');
	const icon = h('span', 'mkt-icon');
	icon.dataset.kind = category;
	icon.setAttribute('aria-hidden', 'true');
	const namebox = h('span', 'mkt-namebox');
	namebox.append(h('strong', 'mkt-name', title));
	// Like an app's developer line: the exact id and who publishes it.
	const byline = h('span', 'mkt-id', `${offer.serviceId || offer.offerId} · ${providerText}`);
	byline.title = offer.providerId;
	namebox.append(byline);
	namebox.append(h('span', 'mkt-sub', description));
	if (demoLink) namebox.append(h('span', 'mkt-note', 'Public beta. No invitation; free jobs share a finite allowance. The demo checks admission when you run.'));
	if (offer.availability?.admission === 'invitation_required') {
		namebox.append(h('span', 'mkt-note', 'Invitation required. Listing checks metadata only; execution has not been tested by the marketplace.'));
	}
	// The full sentence shows here only where the Price column has collapsed; otherwise a short caption sits under the price.
	const explainsFees = !free && offer.pricingKnown !== false;
	if (explainsFees) namebox.append(h('span', 'mkt-note mkt-note--price', 'Price is base + success fee; inspect payment terms before use.'));
	// Shown only where the Type, Price and Status columns have collapsed; each part appears with its column.
	const inline = h('span', 'mkt-inline');
	inline.append(h('span', 'mkt-inline__type', CATEGORY_LABELS[category]), h('span', 'mkt-inline__price', ` · ${price}`), h('span', 'mkt-inline__status', ` · ${availability.label}`));
	namebox.append(inline);
	const lockup = h('div', 'mkt-service');
	lockup.append(icon, namebox);
	service.append(lockup);

	const type = h('td', 'mkt-col-type');
	const tag = h('span', 'mkt-tag', CATEGORY_LABELS[category]);
	tag.dataset.kind = category;
	type.append(tag);

	const priceCell = h('td', 'mkt-col-price');
	priceCell.append(h('strong', free ? 'service-price is-free' : 'service-price', price));
	if (explainsFees) priceCell.append(h('span', 'mkt-price-note', 'base + success fee'));

	const status = h('td', 'mkt-col-status');
	const badge = h('span', 'service-availability', availability.label);
	badge.dataset.ready = String(availability.ready);
	badge.dataset.state = availability.state;
	status.append(badge);
	const checkedIso = isoFromSeconds(offer.availability?.lastCheckedAt);
	if (checkedIso) {
		const since = Date.parse(checkedIso);
		const age = h('time', 'mkt-age', `checked ${formatAge(ctx.now - since)}`);
		age.dateTime = checkedIso;
		age.dataset.since = String(since);
		status.append(age);
	}

	const action = h('td', 'mkt-col-action');
	const actions = h('div', 'mkt-actions');
	if (demoLink) {
		const tryIt = h('a', 'service-open mkt-pill mkt-pill--primary', 'Try it');
		tryIt.setAttribute('href', demoLink);
		tryIt.setAttribute('aria-label', `Try ${title} in the public beta`);
		tryIt.dataset.marketplacePublicDemo = '';
		actions.append(tryIt);
	}
	if (link) {
		const open = h('a', 'service-open mkt-pill mkt-pill--primary', 'Open');
		open.setAttribute('href', link);
		open.setAttribute('aria-label', `Open ${title}`);
		actions.append(open);
	}
	const toggle = h('button', 'mkt-toggle');
	toggle.type = 'button';
	toggle.dataset.marketplaceToggle = '';
	toggle.setAttribute('aria-expanded', 'false');
	toggle.setAttribute('aria-controls', detailId);
	toggle.setAttribute('aria-label', `Details for ${title}`);
	toggle.append(h('span', 'mkt-toggle__label', 'Details'), h('span', 'mkt-chevron'));
	actions.append(toggle);
	action.append(actions);

	const row = h('tr', 'mkt-row');
	row.append(service, type, priceCell, status, action);
	item.append(row, buildDetail(offer, ctx, detailId, title, link));
	return item;
}

function listItems(table: HTMLTableElement): HTMLTableSectionElement[] {
	return Array.from(table.tBodies).filter((body) => body.classList.contains('mkt-item'));
}

function applySort(root: HTMLElement): void {
	const table = root.querySelector<HTMLTableElement>('[data-marketplace-service-list]');
	if (!table) return;
	const key = root.dataset.sortKey || 'status';
	const direction = root.dataset.sortDir === 'desc' ? -1 : 1;
	const number = (element: HTMLElement, name: string) => Number(element.dataset[name] ?? 0);
	const items = listItems(table).sort((a, b) => {
		let delta = 0;
		if (key === 'name') delta = (a.dataset.sortName ?? '').localeCompare(b.dataset.sortName ?? '');
		else if (key === 'category') delta = number(a, 'sortCategory') - number(b, 'sortCategory');
		else if (key === 'price') delta = number(a, 'sortPrice') - number(b, 'sortPrice');
		else delta = number(a, 'sortStatus') - number(b, 'sortStatus');
		return delta * direction
			|| (a.dataset.sortName ?? '').localeCompare(b.dataset.sortName ?? '')
			|| (a.dataset.key ?? '').localeCompare(b.dataset.key ?? '');
	});
	table.append(...items);
	for (const header of table.querySelectorAll<HTMLElement>('thead th')) {
		header.removeAttribute('aria-sort');
		if (header.querySelector<HTMLElement>('[data-sort]')?.dataset.sort === key) header.setAttribute('aria-sort', direction === 1 ? 'ascending' : 'descending');
	}
}

const previousSignatures = new WeakMap<HTMLElement, Map<string, string>>();

function renderServiceList(root: HTMLElement, snapshot: MarketplaceSnapshot, stale: boolean, flash: boolean): void {
	const table = root.querySelector<HTMLTableElement>('[data-marketplace-service-list]');
	if (!table) return;
	const now = Date.now();
	const ctx: RenderContext = { providers: new Map(snapshot.providers.map((provider) => [provider.providerId, provider])), stale, now };
	for (const body of Array.from(table.tBodies)) {
		if (body.classList.contains('mkt-item') || body.classList.contains('mkt-skeleton-rows')) body.remove();
	}

	const previous = flash ? previousSignatures.get(root) : undefined;
	const next = new Map<string, string>();
	for (const offer of snapshot.offers) {
		const item = buildItem(offer, ctx);
		const key = item.dataset.key ?? '';
		next.set(key, item.dataset.signature ?? '');
		if (previous && previous.size > 0) {
			if (!previous.has(key)) item.classList.add('is-new');
			else if (previous.get(key) !== item.dataset.signature) item.classList.add('is-updated');
		}
		item.addEventListener('animationend', () => item.classList.remove('is-new', 'is-updated'));
		table.append(item);
	}
	previousSignatures.set(root, next);
	table.setAttribute('aria-busy', 'false');
	applySort(root);

	const shared = snapshot.offers.filter(hasShareLink).length;
	root.dataset.sharedCount = String(shared);
	setText(root, '[data-marketplace-field="sharedServices"]', shared);
	setText(root, '[data-marketplace-field="recentServices"]', snapshot.offers.filter((offer) => serviceAvailability(offer, stale, now).ready).length);
	setText(root, '[data-marketplace-field="scope"]',
		snapshot.offerCount > snapshot.offers.length
			? `Showing ${snapshot.offers.length} of ${snapshot.offerCount} indexed offers. Search and filters cover only the offers loaded here.`
			: `Showing all ${snapshot.offers.length} indexed offer${snapshot.offers.length === 1 ? '' : 's'}.`);
}

function renderCategories(root: HTMLElement, offers: MarketplaceOfferSummary[]): void {
	const group = root.querySelector<HTMLElement>('[data-marketplace-categories]');
	if (!group) return;
	const counts = new Map<Category, number>();
	for (const offer of offers) counts.set(categoryOf(offer), (counts.get(categoryOf(offer)) ?? 0) + 1);
	const selected = group.querySelector<HTMLInputElement>('input:checked')?.value ?? 'all';
	const entries: Array<[string, string, number]> = [['all', 'All services', offers.length]];
	for (const category of CATEGORY_ORDER) {
		// "Shared services" is always listed, even at zero, so its empty state is discoverable.
		if (category === 'service' || (counts.get(category) ?? 0) > 0) entries.push([category, CATEGORY_LABELS[category], counts.get(category) ?? 0]);
	}
	group.querySelectorAll('.mkt-cat').forEach((node) => node.remove());
	const keep = entries.some(([value]) => value === selected) ? selected : 'all';
	for (const [value, label, count] of entries) {
		const wrap = h('label', 'mkt-cat');
		const input = h('input');
		input.type = 'radio';
		input.name = 'marketplace-category';
		input.value = value;
		input.checked = value === keep;
		const countNode = h('span', 'mkt-cat__count', String(count));
		countNode.dataset.marketplaceCategoryCount = value;
		wrap.append(input, h('span', 'mkt-cat__name', label), countNode);
		group.append(wrap);
	}
}

// ── Provider and runtime panels ──────────────────────────────

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

function renderProviderRow(provider: MarketplaceProviderSummary): HTMLTableRowElement {
	const row = document.createElement('tr');
	row.className = 'row';
	row.dataset.marketplaceSearchRow = '';
	row.dataset.marketplaceKind = 'provider';
	row.dataset.providerSummary = providerEvidence(provider);
	row.dataset.searchText = [provider.providerId, compactId(provider.providerId), provider.descriptorHash, provider.endpoint, ...provider.serviceKinds].join(' ');

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
	const rows = [...providers].sort((a, b) => b.successCount + b.failureCount - (a.successCount + a.failureCount)).slice(0, 8);
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

function renderServicesBreakdown(root: ParentNode, offers: MarketplaceOfferSummary[]): void {
	const container = root.querySelector<HTMLElement>('[data-marketplace-services-breakdown]');
	if (!container) return;
	const counts = new Map<string, number>();
	for (const offer of offers) {
		const key = offer.runtime || offer.offerKind || 'other';
		counts.set(key, (counts.get(key) ?? 0) + 1);
	}
	const rows = Array.from(counts.entries()).map(([name, count]) => ({ name, count })).sort((a, b) => b.count - a.count);
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

// ── Search, filters and banners ──────────────────────────────

function searchableText(element: HTMLElement): string {
	return `${element.dataset.searchText || ''} ${element.textContent || ''}`.toLowerCase();
}

function readFilters(root: HTMLElement): { category: string; ready: boolean; free: boolean } {
	return {
		category: root.querySelector<HTMLInputElement>('[data-marketplace-categories] input:checked')?.value ?? 'all',
		ready: root.querySelector<HTMLInputElement>('[data-marketplace-filter="ready"]')?.checked ?? false,
		free: root.querySelector<HTMLInputElement>('[data-marketplace-filter="free"]')?.checked ?? false,
	};
}

type BannerMode = 'none' | 'empty' | 'error';

function setBanner(root: HTMLElement, mode: BannerMode): void {
	const banner = root.querySelector<HTMLElement>('[data-marketplace-banner]');
	if (!banner) return;
	if (banner.dataset.mode === mode) {
		banner.hidden = mode === 'none';
		return;
	}
	banner.dataset.mode = mode;
	banner.dataset.tone = mode;
	banner.replaceChildren();
	banner.hidden = mode === 'none';
	if (mode === 'none') return;
	banner.setAttribute('role', mode === 'error' ? 'alert' : 'status');
	const actions = h('div', 'mkt-banner__actions');
	if (mode === 'error') {
		banner.append(h('strong', 'mkt-banner__title', 'The catalog is unavailable'), h('p', 'mkt-banner__text', 'We could not load it. We will try again automatically every 30 seconds.'));
		const retry = h('button', 'mkt-btn mkt-btn--primary', 'Try again now');
		retry.type = 'button';
		retry.dataset.marketplaceRetry = '';
		actions.append(retry);
	} else {
		banner.append(
			h('strong', 'mkt-banner__title', 'No shared services are listed right now'),
			h('p', 'mkt-banner__text', 'Services that people publish with a share link appear here, with what they do and what they cost. The list below shows the marketplace’s own offers.'),
		);
		const share = h('a', 'mkt-btn mkt-btn--primary', 'Share something useful');
		share.setAttribute('href', '/publish/');
		const learn = h('a', 'mkt-btn', 'How sharing works');
		learn.setAttribute('href', '/learn/share-services/');
		actions.append(share, learn);
	}
	banner.append(actions);
}

function initMarketplaceSearch(root: HTMLElement): () => void {
	const input = root.querySelector<HTMLInputElement>('[data-marketplace-search]');
	const count = root.querySelector<HTMLOutputElement>('[data-marketplace-search-count]');
	const form = root.querySelector<HTMLElement>('[data-marketplace-search-form]');
	if (!input) return () => {};

	const apply = () => {
		const terms = input.value.trim().toLowerCase().split(/\s+/).filter(Boolean);
		const { category, ready, free } = readFilters(root);
		const rows = Array.from(root.querySelectorAll<HTMLElement>('[data-marketplace-search-row]'));
		let shown = 0;
		for (const row of rows) {
			const matches = terms.length === 0 || terms.every((term) => searchableText(row).includes(term));
			const isOffer = row.dataset.marketplaceKind === 'offer';
			const visible = matches && (!isOffer
				|| ((category === 'all' || row.dataset.category === category)
					&& (!ready || row.dataset.ready === 'true')
					&& (!free || row.dataset.free === 'true')));
			row.hidden = !visible;
			if (visible && isOffer) shown += 1;
		}

		const loaded = root.dataset.catalogLoaded === 'true';
		const noShared = loaded && Number(root.dataset.sharedCount ?? 0) === 0 && (category === 'all' || category === 'service');
		setBanner(root, !loaded && root.dataset.status === 'unavailable' ? 'error' : noShared ? 'empty' : 'none');
		const empty = root.querySelector<HTMLElement>('[data-marketplace-no-results]');
		const hasOffers = rows.some((row) => row.dataset.marketplaceKind === 'offer');
		if (empty) empty.hidden = !loaded || shown !== 0 || !hasOffers || (category === 'service' && noShared);
		if (count) count.textContent = loaded ? `${shown} service${shown === 1 ? '' : 's'}` : (root.dataset.status === 'unavailable' ? 'Unavailable' : 'Loading');
	};

	input.addEventListener('input', apply);
	root.addEventListener('change', (event) => {
		if ((event.target as HTMLElement | null)?.matches('[data-marketplace-filter], [data-marketplace-categories] input')) apply();
	});
	form?.addEventListener('click', () => input.focus());
	root.querySelector('[data-marketplace-reset]')?.addEventListener('click', () => {
		input.value = '';
		for (const toggle of root.querySelectorAll<HTMLInputElement>('[data-marketplace-filter]')) toggle.checked = false;
		const all = root.querySelector<HTMLInputElement>('[data-marketplace-categories] input[value="all"]');
		if (all) all.checked = true;
		apply();
		input.focus();
	});
	document.addEventListener('keydown', (event) => {
		const target = event.target as HTMLElement | null;
		const typing = Boolean(target && (target.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName)));
		if (event.key === '/' && !typing && !event.metaKey && !event.ctrlKey && !event.altKey) {
			event.preventDefault();
			input.focus();
		} else if (event.key === 'Escape' && target === input && input.value) {
			input.value = '';
			apply();
		}
	});
	apply();
	return apply;
}

// ── Evidence and copy actions ────────────────────────────────

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

// ── Snapshot rendering and the live loop ─────────────────────

function renderSnapshot(root: HTMLElement, snapshot: MarketplaceSnapshot, flash: boolean): void {
	root.dataset.catalogLoaded = 'true';
	const successCount = snapshot.providers.reduce((sum, provider) => sum + provider.successCount, 0);
	const failureCount = snapshot.providers.reduce((sum, provider) => sum + provider.failureCount, 0);
	const totalReceipts = successCount + failureCount;
	const freeOffers = snapshot.offers.filter(isFree).length;
	const paidOffers = snapshot.offers.filter((offer) => offer.pricingKnown !== false).length - freeOffers;
	const totalSettledMsat = snapshot.providers.reduce((sum, provider) => sum + provider.totalSettledMsat, 0);

	root.dataset.status = snapshot.status;
	setText(root, '[data-marketplace-field="froglets"]', snapshot.providerCount);
	setText(root, '[data-marketplace-field="offers"]', snapshot.offerCount);
	setText(root, '[data-marketplace-field="checkedAt"]', `${formatSnapshotTime(snapshot.checkedAt)} UTC`);
	setText(root, '[data-marketplace-field="detail"]', `${snapshot.detail} Receipt totals cover ${snapshot.providers.length} sampled providers; pricing and runtime breakdown cover ${snapshot.offers.length} sampled offers.`);
	setText(root, '[data-marketplace-field="freeOffers"]', freeOffers);
	setText(root, '[data-marketplace-field="paidOffers"]', paidOffers);
	setText(root, '[data-marketplace-field="successRate"]', totalReceipts === 0 ? 'N/A' : `${Math.round((successCount / totalReceipts) * 100)}%`);
	setText(root, '[data-marketplace-field="receipts"]', totalReceipts);
	setText(root, '[data-marketplace-field="settledSats"]', `${Math.round(totalSettledMsat / 1000)}sats`);
	setText(root, '[data-marketplace-field="dealFeedDetail"]', snapshot.dealFeed.detail);
	renderServiceList(root, snapshot, Date.now() - Date.parse(snapshot.checkedAt) > STALE_MS, flash);
	renderCategories(root, snapshot.offers);
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
	let nextRefreshAt = Date.now() + REFRESH_MS;
	// Whether the rows on screen were drawn as stale; state() only redraws when that changes,
	// so the change-flash from a fresh snapshot is not wiped by a second render.
	let renderedStale: boolean | undefined;

	function state(status: 'live' | 'stale' | 'unavailable', detail?: string) {
		root!.dataset.status = status;
		setText(root!, '[data-marketplace-field="message"]', status === 'live'
			? 'Catalog updated. This confirms the index loaded, not that every service is available.'
			: status === 'stale'
				? 'Showing an earlier catalog. Availability may have changed; we are checking again.'
				: 'We could not load the catalog. We will try again automatically.');
		const badge = document.querySelector<HTMLElement>('[data-marketplace-field="refresh"]');
		if (badge) { badge.textContent = status === 'live' ? 'Catalog updated' : status.toUpperCase(); badge.dataset.status = status; }
		if (lastSnapshot) {
			if (renderedStale !== (status !== 'live')) { renderedStale = status !== 'live'; renderServiceList(root!, lastSnapshot, renderedStale, false); }
			applySearch();
		}
		if (detail) setText(root!, '[data-marketplace-field="detail"]', detail);
		if (!lastSnapshot) {
			root!.querySelector('.mkt-skeleton-rows')?.remove();
			setText(root!, '[data-marketplace-field="scope"]', 'The catalog could not be loaded.');
			for (const field of ['froglets', 'offers', 'sharedServices', 'recentServices', 'freeOffers', 'paidOffers', 'receipts', 'successRate', 'settledSats']) {
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
			applySearch();
		}
	}

	const refreshButton = root.querySelector<HTMLButtonElement>('[data-marketplace-refresh]');
	const refresh = async () => {
		if (refreshing) return;
		refreshing = true;
		if (refreshButton) { refreshButton.disabled = true; refreshButton.textContent = 'Checking…'; }
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
			renderSnapshot(root, snapshot, true);
			renderedStale = age > STALE_MS;
			lastSnapshot = snapshot;
			state(age > STALE_MS ? 'stale' : 'live');
			applySearch();
		} catch (error) {
			state(lastSnapshot ? 'stale' : 'unavailable',
				`${lastSnapshot ? 'Showing the last successful snapshot. ' : ''}${error instanceof Error ? error.message : String(error)}`);
		} finally {
			refreshing = false;
			nextRefreshAt = Date.now() + REFRESH_MS;
			if (refreshButton) { refreshButton.disabled = false; refreshButton.textContent = 'Refresh now'; }
		}
	};

	// Disclosure toggles, sortable headers, copy buttons and retry, wired once for all rows.
	root.addEventListener('click', async (event) => {
		const target = event.target as HTMLElement | null;
		const toggle = target?.closest<HTMLButtonElement>('[data-marketplace-toggle]');
		if (toggle) {
			const open = toggle.getAttribute('aria-expanded') !== 'true';
			toggle.setAttribute('aria-expanded', String(open));
			const detail = root.querySelector<HTMLElement>(`#${toggle.getAttribute('aria-controls')}`);
			if (detail) detail.hidden = !open;
			toggle.closest('.mkt-item')?.classList.toggle('is-open', open);
			return;
		}
		const copy = target?.closest<HTMLButtonElement>('[data-copy-value]');
		if (copy) {
			try { await copyText(copy.dataset.copyValue ?? ''); markCopyButton(copy, 'Copied'); } catch { markCopyButton(copy, 'Copy failed'); }
			return;
		}
		const sort = target?.closest<HTMLElement>('[data-sort]');
		if (sort) {
			const key = sort.dataset.sort ?? 'status';
			// The list starts sorted by Status ascending, so a first click on that header reverses it.
			const ascending = (root.dataset.sortKey || 'status') === key && (root.dataset.sortDir || 'asc') === 'asc';
			root.dataset.sortDir = ascending ? 'desc' : 'asc';
			root.dataset.sortKey = key;
			applySort(root);
			return;
		}
		if (target?.closest('[data-marketplace-retry]')) void refresh();
	});
	refreshButton?.addEventListener('click', () => void refresh());

	// One-second tick: "Updated Ns ago", the next-check countdown and every row's check age.
	const tick = () => {
		const now = Date.now();
		if (lastSnapshot) setText(root, '[data-marketplace-age]', `Updated ${formatAge(now - Date.parse(lastSnapshot.checkedAt))}`);
		const remaining = Math.max(0, Math.ceil((nextRefreshAt - now) / 1000));
		setText(root, '[data-marketplace-next]', refreshing ? 'Checking…' : `Next check in ${remaining}s`);
		for (const time of root.querySelectorAll<HTMLElement>('time[data-since]')) {
			time.textContent = `checked ${formatAge(now - Number(time.dataset.since))}`;
		}
	};
	window.setInterval(tick, 1000);

	void refresh();
	window.setInterval(() => {
		if (lastSnapshot && Date.now() - Date.parse(lastSnapshot.checkedAt) > STALE_MS) state('stale');
		void refresh();
	}, REFRESH_MS);
}
