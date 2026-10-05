import type { APIRoute } from 'astro';
import { readAdder } from '../../data/adder-download';

export const prerender = true;

export const GET: APIRoute = () => new Response(readAdder().buffer as ArrayBuffer, {
  headers: { 'content-type': 'application/wasm' },
});
