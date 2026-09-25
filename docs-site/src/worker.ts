import { getMarketplaceSnapshot } from './data/live-snapshot';
import { serviceReference, sharedService } from './data/shared-service';

interface WorkerEnv {
	ASSETS: {
		fetch(request: Request): Promise<Response>;
	};
	FROGLET_RELAY_SUFFIX?: string;
	FROGLET_MARKETPLACE_URL?: string;
}

const jsonHeaders = {
	'content-type': 'application/json; charset=utf-8',
	'cache-control': 'no-store',
};

export default {
	async fetch(request: Request, env: WorkerEnv): Promise<Response> {
		const url = new URL(request.url);
		if (url.pathname === '/api/shared-service') {
			if (request.method !== 'GET') return new Response(JSON.stringify({ error: 'method not allowed' }), { status: 405, headers: { ...jsonHeaders, allow: 'GET' } });
			const provider = url.searchParams.get('provider') ?? '';
			const service = url.searchParams.get('service') ?? '';
			try { serviceReference(provider, service); }
			catch (error) { return new Response(JSON.stringify({ error: String(error) }), { status: 400, headers: jsonHeaders }); }
			try { return new Response(JSON.stringify(await sharedService(provider, service, fetch, { relaySuffix: env.FROGLET_RELAY_SUFFIX, marketplaceUrl: env.FROGLET_MARKETPLACE_URL })), { headers: jsonHeaders }); }
			catch (error) {
				console.warn('shared-service check failed', error instanceof Error ? error.message : String(error));
				return new Response(JSON.stringify({ status: 'unavailable', checkedAt: new Date().toISOString(), error: 'The service could not be checked. Its computer may be asleep, offline, or no longer publishing. Ask the sender to check Froglet status.' }), { status: 503, headers: jsonHeaders });
			}
		}
		if (url.pathname === '/api/marketplace-snapshot') {
			if (request.method !== 'GET' && request.method !== 'HEAD') {
				return new Response(JSON.stringify({ error: 'method not allowed' }), {
					status: 405,
					headers: { ...jsonHeaders, allow: 'GET, HEAD' },
				});
			}

			const snapshot = await getMarketplaceSnapshot(env.FROGLET_MARKETPLACE_URL);
			return new Response(request.method === 'HEAD' ? null : JSON.stringify(snapshot), {
				status: snapshot.status === 'pass' ? 200 : 502,
				headers: jsonHeaders,
			});
		}

		return env.ASSETS.fetch(request);
	},
};
