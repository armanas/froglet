import { getMarketplaceSnapshot } from './data/live-snapshot';

interface WorkerEnv {
	ASSETS: {
		fetch(request: Request): Promise<Response>;
	};
}

const jsonHeaders = {
	'content-type': 'application/json; charset=utf-8',
	'cache-control': 'public, max-age=20',
};

export default {
	async fetch(request: Request, env: WorkerEnv): Promise<Response> {
		const url = new URL(request.url);
		if (url.pathname === '/api/marketplace-snapshot') {
			if (request.method !== 'GET' && request.method !== 'HEAD') {
				return new Response(JSON.stringify({ error: 'method not allowed' }), {
					status: 405,
					headers: jsonHeaders,
				});
			}

			const snapshot = await getMarketplaceSnapshot();
			return new Response(request.method === 'HEAD' ? null : JSON.stringify(snapshot), {
				status: snapshot.status === 'pass' ? 200 : 502,
				headers: jsonHeaders,
			});
		}

		return env.ASSETS.fetch(request);
	},
};
