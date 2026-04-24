type FetchKind = 'json' | 'text';

export type ProbeState = 'pass' | 'fail';

export interface EndpointProbe {
	label: string;
	url: string;
	expect: string;
	status: ProbeState;
	httpStatus?: number;
	detail: string;
}

export interface MarketplaceProviderSummary {
	providerId: string;
	descriptorHash: string;
	serviceKinds: string[];
	executionRuntimes: string[];
	endpoint: string;
	successCount: number;
	failureCount: number;
	totalSettledMsat: number;
}

export interface MarketplaceOfferSummary {
	offerId: string;
	offerKind: string;
	runtime: string;
	packageKind: string;
	settlementMethod: string;
	baseFeeMsat: number;
	successFeeMsat: number;
	artifactHash: string;
}

export interface MarketplaceSnapshot {
	checkedAt: string;
	status: ProbeState;
	detail: string;
	providerCount: number;
	offerCount: number;
	providers: MarketplaceProviderSummary[];
	offers: MarketplaceOfferSummary[];
}

export interface StatusSnapshot {
	checkedAt: string;
	endpoints: EndpointProbe[];
	marketplace: MarketplaceSnapshot;
}

const MARKETPLACE_URL = 'https://marketplace.froglet.dev';
const PROVIDER_URL = 'https://ai.froglet.dev';
const TRY_URL = 'https://try.froglet.dev';
const SITE_URL = 'https://froglet.dev';

const timeoutMs = 4_000;

function nowIso(): string {
	return new Date().toISOString();
}

function shortHash(value: string, length = 12): string {
	return value.length > length ? `${value.slice(0, length)}...` : value;
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

async function readBody(response: Response, kind: FetchKind): Promise<unknown> {
	if (kind === 'json') return response.json();
	return response.text();
}

async function probe(
	label: string,
	url: string,
	expect: string,
	kind: FetchKind,
	check: (body: unknown) => { ok: boolean; detail: string },
): Promise<EndpointProbe> {
	try {
		const response = await fetchWithTimeout(url);
		const body = await readBody(response, kind);
		const result = check(body);
		if (response.ok && result.ok) {
			return { label, url, expect, status: 'pass', httpStatus: response.status, detail: result.detail };
		}
		return {
			label,
			url,
			expect,
			status: 'fail',
			httpStatus: response.status,
			detail: result.detail || `Unexpected response from ${url}`,
		};
	} catch (error) {
		return {
			label,
			url,
			expect,
			status: 'fail',
			detail: error instanceof Error ? error.message : String(error),
		};
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

export async function getMarketplaceSnapshot(): Promise<MarketplaceSnapshot> {
	const checkedAt = nowIso();
	try {
		const healthResponse = await fetchWithTimeout(`${MARKETPLACE_URL}/healthz`);
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
			};
		}

		const [providersResponse, offersResponse] = await Promise.all([
			fetchWithTimeout(`${MARKETPLACE_URL}/v1/providers?limit=3`),
			fetchWithTimeout(`${MARKETPLACE_URL}/v1/offers?limit=8`),
		]);
		const providersBody = asRecord(await providersResponse.json());
		const offersBody = asRecord(await offersResponse.json());
		const providerItems = asArray(providersBody.items);
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
				descriptorHash: shortHash(String(row.current_descriptor_hash ?? descriptor.artifact_hash ?? '')),
				serviceKinds: asStringArray(descriptor.service_kinds),
				executionRuntimes: asStringArray(descriptor.execution_runtimes),
				endpoint: String(firstEndpoint.uri ?? ''),
				successCount: asNumber(trust.success_count),
				failureCount: asNumber(trust.failure_count),
				totalSettledMsat: asNumber(trust.total_settled_msat),
			};
		});

		const offers = offerItems.map((item): MarketplaceOfferSummary => {
			const row = asRecord(item);
			return {
				offerId: String(row.offer_id ?? ''),
				offerKind: String(row.offer_kind ?? ''),
				runtime: String(row.runtime ?? ''),
				packageKind: String(row.package_kind ?? ''),
				settlementMethod: String(row.settlement_method ?? ''),
				baseFeeMsat: asNumber(row.base_fee_msat),
				successFeeMsat: asNumber(row.success_fee_msat),
				artifactHash: shortHash(String(row.artifact_hash ?? '')),
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
		};
	} catch (error) {
		return {
			checkedAt,
			status: 'fail',
			detail: error instanceof Error ? error.message : String(error),
			providerCount: 0,
			offerCount: 0,
			providers: [],
			offers: [],
		};
	}
}

export async function getStatusSnapshot(): Promise<StatusSnapshot> {
	const checkedAt = nowIso();
	const [site, providerHealth, providerVersion, tryPrompt, marketplace] = await Promise.all([
		probe('Site', `${SITE_URL}/`, 'HTTP 200 HTML containing Froglet', 'text', (body) => {
			const text = String(body);
			return text.includes('Froglet')
				? { ok: true, detail: 'Homepage returned Froglet HTML.' }
				: { ok: false, detail: 'Homepage did not contain the Froglet marker.' };
		}),
		probe('Hosted node health', `${PROVIDER_URL}/health`, 'JSON status ok', 'json', (body) => {
			const json = asRecord(body);
			return json.status === 'ok' && json.service === 'froglet'
				? { ok: true, detail: 'Node health is ok.' }
				: { ok: false, detail: 'Node health shape did not match.' };
		}),
		probe('Hosted node version', `${PROVIDER_URL}/v1/node/capabilities`, 'Version matches public release', 'json', (body) => {
			const json = asRecord(body);
			return json.version === '0.1.0'
				? { ok: true, detail: 'Hosted node reports version 0.1.0.' }
				: { ok: false, detail: `Hosted node reports version ${String(json.version ?? 'unknown')}.` };
		}),
		probe('Hosted trial prompt', `${TRY_URL}/llms.txt`, 'HTTP 200 LLM instructions', 'text', (body) => {
			const text = String(body);
			return text.includes('Froglet') && text.includes('demo.add')
				? { ok: true, detail: 'LLM trial prompt is reachable.' }
				: { ok: false, detail: 'LLM trial prompt markers were missing.' };
		}),
		getMarketplaceSnapshot(),
	]);

	const marketplaceProbe: EndpointProbe = {
		label: 'Marketplace read API',
		url: `${MARKETPLACE_URL}/healthz`,
		expect: 'JSON status ok plus providers/offers read API',
		status: marketplace.status,
		httpStatus: marketplace.status === 'pass' ? 200 : undefined,
		detail: marketplace.detail,
	};

	return {
		checkedAt,
		endpoints: [site, providerHealth, providerVersion, tryPrompt, marketplaceProbe],
		marketplace,
	};
}
