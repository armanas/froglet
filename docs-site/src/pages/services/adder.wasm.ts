import type { APIRoute } from 'astro';
import { compileAdder } from '../../data/adder-download';

export const prerender = true;

export const GET: APIRoute = async () => new Response((await compileAdder()).slice().buffer as ArrayBuffer, {
  headers: { 'content-type': 'application/wasm' },
});
