import { serviceQrSvg } from './data/service-qr';
import { getMarketplaceSnapshot } from './data/live-snapshot';
import { serviceReference, sharedService } from './data/shared-service';
import { resolveServiceLink, restoreServiceLinkCache } from './data/service-link';
import { renderServiceLinkHtml, renderServiceLinkMarkdown } from './data/service-link-page';
import { verifyServiceLinkEvidence } from './data/service-link-verifier';
import { publicDemoResponse, type PublicDemoEnv } from './data/public-demo-proxy';

interface WorkerEnv extends PublicDemoEnv {
	ASSETS: {
		fetch(request: Request): Promise<Response>;
	};
	FROGLET_RELAY_SUFFIX?: string;
	FROGLET_MARKETPLACE_URL?: string;
	FROGLET_SITE_ORIGIN?: string;
	SERVICE_LINK_CACHE?: { get(key: string): Promise<string | null>; put(key: string, value: string, options: { expirationTtl: number }): Promise<void> };
}

const jsonHeaders = {
	'content-type': 'application/json; charset=utf-8',
	'cache-control': 'no-store',
};

export default {
	async fetch(request: Request, env: WorkerEnv): Promise<Response> {
		const demo = await publicDemoResponse(request, env);
		if (demo) return demo;
		const url = new URL(request.url);
		if (url.pathname === '/service' || url.pathname === '/service/') {
			const providers = url.searchParams.getAll('provider');
			const services = url.searchParams.getAll('service');
			if (providers.length || services.length) {
				if (request.method !== 'GET' && request.method !== 'HEAD') return new Response('Method not allowed', { status: 405, headers: { allow: 'GET, HEAD' } });
				if (providers.length !== 1 || services.length !== 1) return new Response('Invalid service link', { status: 400, headers: { 'cache-control': 'no-store' } });
				try { serviceReference(providers[0], services[0], env.FROGLET_RELAY_SUFFIX); }
				catch { return new Response('Invalid service link', { status: 400, headers: { 'cache-control': 'no-store' } }); }
				return new Response(null, { status: 302, headers: { location: new URL(`/s/${providers[0]}/${encodeURIComponent(services[0])}`, url.origin).toString(), 'cache-control': 'no-store' } });
			}
		}
		if (url.pathname.startsWith('/s/')) {
			if (request.method !== 'GET' && request.method !== 'HEAD') return new Response('Method not allowed', { status: 405, headers: { allow: 'GET, HEAD' } });
			const route = /^\/s\/([^/]+)\/([^/]+?)(?:\/(manifest\.json|agent\.md|qr\.svg))?\/?$/.exec(url.pathname);
			if (!route || url.searchParams.has('revision')) return new Response('Invalid service link', { status: 400, headers: { 'cache-control': 'no-store' } });
			let provider: string, service: string;
			try {
				provider = decodeURIComponent(route[1]); service = decodeURIComponent(route[2]);
				serviceReference(provider, service, env.FROGLET_RELAY_SUFFIX);
			} catch { return new Response('Invalid service link', { status: 400, headers: { 'cache-control': 'no-store' } }); }
			if (route[3] === 'qr.svg') {
				const origin = env.FROGLET_SITE_ORIGIN ?? 'https://froglet.dev';
				if (!/^https:\/\/(?:candidate\.)?froglet\.dev$/.test(origin)) return new Response('Invalid site origin', {status:500});
				const canonical = `${origin}/s/${provider}/${encodeURIComponent(service)}`;
				return new Response(request.method === 'HEAD' ? null : serviceQrSvg(canonical), {headers:{
					'content-type':'image/svg+xml; charset=utf-8', 'cache-control':'public, max-age=86400',
					'x-content-type-options':'nosniff', 'content-security-policy':"default-src 'none'; style-src 'unsafe-inline'; sandbox",
					'content-disposition':`${url.searchParams.get('download') === '1' ? 'attachment' : 'inline'}; filename="${service}-qr.svg"`,
				}});
			}
			let view;
			try {
				const siteOrigin = env.FROGLET_SITE_ORIGIN ?? 'https://froglet.dev';
				if (!/^https:\/\/(?:candidate\.)?froglet\.dev$/.test(siteOrigin)) throw new Error('Invalid first-party site origin');
				view = await resolveServiceLink(provider, service, siteOrigin, fetch, { relaySuffix: env.FROGLET_RELAY_SUFFIX, marketplaceUrl: env.FROGLET_MARKETPLACE_URL ?? 'https://marketplace.froglet.dev' }, verifyServiceLinkEvidence);
			} catch (error) {
				console.warn('service-link resolution failed', error instanceof Error ? error.message : String(error));
				return new Response('Service description temporarily unavailable', { status: 503, headers: { 'cache-control': 'no-store' } });
			}
			const cacheKey = `service-link:v1:${provider}:${service}`;
			if (view.availability.state === 'published_reachable' && view.evidence.verification_state === 'verified') {
				try {
					if (env.SERVICE_LINK_CACHE) {
						const saved = await env.SERVICE_LINK_CACHE.get(cacheKey);
						let previous: { contract?: { revision_hash?: string }; availability?: { last_verified_at?: string } } | null = null;
						try { previous = saved ? JSON.parse(saved) : null; } catch { /* Replace malformed cache data. */ }
						const age = Date.now() - Date.parse(previous?.availability?.last_verified_at ?? '');
						const recent = previous?.contract?.revision_hash === view.contract?.revision_hash && age >= 0 && age < 5 * 60 * 1000;
						if (!recent) await env.SERVICE_LINK_CACHE.put(cacheKey, JSON.stringify(view), { expirationTtl: 30 * 24 * 60 * 60 });
					}
				}
				catch (error) { console.warn('service-link cache write failed', error instanceof Error ? error.message : String(error)); }
			} else if (env.SERVICE_LINK_CACHE && view.evidence.verification_state !== 'invalid') {
				try {
					const saved = await env.SERVICE_LINK_CACHE.get(cacheKey);
					if (saved) view = await restoreServiceLinkCache(saved, provider, service, env.FROGLET_SITE_ORIGIN ?? 'https://froglet.dev', verifyServiceLinkEvidence) ?? view;
				} catch (error) { console.warn('service-link cache read failed', error instanceof Error ? error.message : String(error)); }
			}
			const accept = request.headers.get('accept')?.split(',')[0]?.split(';')[0]?.trim();
			const mode = route[3] ?? (accept === 'application/json' ? 'manifest.json' : accept === 'text/markdown' || accept === 'text/plain' ? 'agent.md' : 'html');
			const manifest = mode === 'manifest.json';
			const agent = mode === 'agent.md';
			const body = manifest ? JSON.stringify(view) : agent ? renderServiceLinkMarkdown(view) : renderServiceLinkHtml(view);
			const headers = new Headers({
				'content-type': manifest ? 'application/json; charset=utf-8' : agent ? (accept === 'text/plain' && !route[3] ? 'text/plain; charset=utf-8' : 'text/markdown; charset=utf-8') : 'text/html; charset=utf-8',
				'cache-control': 'no-store',
				'vary': 'Accept',
				'access-control-allow-origin': '*',
				'link': `<${view.links.manifest}>; rel="alternate"; type="application/json", <${view.links.agent}>; rel="alternate"; type="text/markdown"`,
				'x-content-type-options': 'nosniff',
			});
			if (!manifest && !agent) headers.set('content-security-policy', "default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; img-src 'self'; base-uri 'none'; form-action 'none'");
			return new Response(request.method === 'HEAD' ? null : body, { status: view.availability.state === 'unknown' ? 503 : 200, headers });
		}
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
