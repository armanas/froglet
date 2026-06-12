import { getMarketplaceSnapshot } from '../../data/live-snapshot';

const jsonHeaders = {
	'content-type': 'application/json; charset=utf-8',
	'cache-control': 'public, max-age=20',
};

export async function GET() {
	const snapshot = await getMarketplaceSnapshot();
	return new Response(JSON.stringify(snapshot), {
		status: snapshot.status === 'pass' ? 200 : 502,
		headers: jsonHeaders,
	});
}

export async function HEAD() {
	const snapshot = await getMarketplaceSnapshot();
	return new Response(null, {
		status: snapshot.status === 'pass' ? 200 : 502,
		headers: jsonHeaders,
	});
}
