import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { startLiveServices } from '../live-services-ui';
import { src } from './route-helpers';

const mocks = vi.hoisted(() => ({
  compile: vi.fn(), dispose: vi.fn(), prepare: vi.fn(), resume: vi.fn(),
  preparePublished: vi.fn(),
}));
vi.mock('../playground/compiler', () => ({
  browserCompilerWorker: vi.fn(),
  createCompiler: () => ({ compile: mocks.compile, dispose: mocks.dispose }),
}));
vi.mock('../playground/kernel', () => ({
  loadKernel: async () => ({}), loadPlaygroundVerifier: async () => ({}),
}));
vi.mock('../live-service-client', async importOriginal => ({
  ...await importOriginal<typeof import('../live-service-client')>(),
  prepareLiveRun: mocks.prepare, preparePublishedServiceRun: mocks.preparePublished, resumeLiveRun: mocks.resume,
  exportLiveEvidence: (outcome: unknown) => ({ outcome }),
}));

const page = readFileSync(resolve(src, 'pages/services.astro'), 'utf8');
const markup = page.slice(page.indexOf('<section class="story-section live-demo"'), page.indexOf('<section class="story-section"><div'));
const $ = <T extends HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
const hide = (persisted: boolean) => window.dispatchEvent(new PageTransitionEvent('pagehide', { persisted }));
const ready = async () => vi.waitFor(() => expect($<HTMLButtonElement>('[data-run-program]').disabled).toBe(false));
const result = async () => vi.waitFor(() => expect($('[data-job-status]').textContent).toBe('Result received. Signed receipt verified.'));

beforeEach(async () => {
  vi.clearAllMocks();
  sessionStorage.clear();
  document.body.innerHTML = markup;
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ providerId: 'a'.repeat(64), remaining: { deals: 1000 }, paused: false }))));
  mocks.compile.mockResolvedValue({ ok: true, module: new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]) });
  const run = { providerId: 'a'.repeat(64), request: { kind: 'wasm', submission: { workload: { module_hash: 'b'.repeat(64) } } } };
  mocks.prepare.mockImplementation(async (deps: any) => { deps.save(run); return run; });
  mocks.resume.mockResolvedValue({ terminal: true, status: 'succeeded', result: { sum: 13, product: 42 }, run });
  startLiveServices();
  await ready();
});

afterEach(() => {
  hide(false);
  document.body.innerHTML = '';
  vi.unstubAllGlobals();
});

test('browser Back restores working handlers and preserves the visitor’s edited program and input', async () => {
  $<HTMLButtonElement>('[data-run-program]').click();
  await result();
  expect($('[data-recovery-note]').textContent).toContain('the result commitment');
  $<HTMLTextAreaElement>('[data-program-source]').value = 'visitor-edited-program';
  $<HTMLTextAreaElement>('[data-program-input]').value = '{"a":8,"b":9}';
  hide(true);
  window.dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true }));
  expect(mocks.dispose).not.toHaveBeenCalled();
  expect($<HTMLTextAreaElement>('[data-program-source]').value).toBe('visitor-edited-program');
  $<HTMLButtonElement>('[data-run-program]').click();
  expect($('[data-recovery-note]').textContent).toBe('');
  await vi.waitFor(() => expect(mocks.prepare).toHaveBeenCalledTimes(2));
  expect(mocks.compile).toHaveBeenLastCalledWith('visitor-edited-program');
  expect(mocks.prepare.mock.calls[1][1]).toBe('{"a":8,"b":9}');
  await result();
});

test.each(['execution_limit_exceeded', 'execution_timed_out', 'execution_failed'])('a verified %s failure describes the receipt without claiming a result commitment', async (failure) => {
  const run = { request: { kind: 'wasm', submission: { workload: { module_hash: 'b'.repeat(64) } } } };
  mocks.resume.mockResolvedValueOnce({ terminal: true, status: 'failed', failure, run });
  $<HTMLButtonElement>('[data-run-failure]').click();
  await vi.waitFor(() => expect($('[data-job-status]').textContent).toContain('Signed failure receipt verified.'));
  expect($('[data-recovery-note]').textContent).toBe('The browser checked the signatures and the signed failure receipt. This verifies Bob’s failure report.');
  expect(JSON.parse($('[data-job-result]').textContent!)).toEqual({ status: 'failed', failure });
  expect($<HTMLButtonElement>('[data-download-receipt]').disabled).toBe(false);
  expect($<HTMLDetailsElement>('[data-exchange-export]').hidden).toBe(false);
});

test.each([false, true])('the native allowance does not promise jobs beyond the shared network limits (paused=%s)', async (paused) => {
  vi.mocked(fetch).mockResolvedValueOnce(new Response(JSON.stringify({ remaining: { deals: 955 }, paused })));
  $<HTMLButtonElement>('[data-run-program]').click();
  await result();
  await vi.waitFor(() => expect($('[data-demo-capacity]').textContent).toBe(`Native allowance: 955 job admissions remaining. Shared network limits may stop access sooner. ${paused ? 'New jobs are paused.' : 'A restart does not refill allowances.'}`));
});

test('a pending signed job remains recoverable after a cached-page round trip', async () => {
  mocks.resume.mockResolvedValueOnce({ terminal: false, reason: 'Retry the same job.' });
  $<HTMLButtonElement>('[data-run-program]').click();
  await vi.waitFor(() => expect($<HTMLButtonElement>('[data-retry-job]').disabled).toBe(false));
  const saved = sessionStorage.getItem('froglet-public-demo.pending.v1');
  hide(true);
  window.dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true }));
  $<HTMLButtonElement>('[data-retry-job]').click();
  await vi.waitFor(() => expect(mocks.resume).toHaveBeenCalledTimes(2));
  expect(mocks.prepare).toHaveBeenCalledTimes(1);
  expect(JSON.stringify(mocks.resume.mock.calls[1][1])).toBe(saved);
  await result();
  expect(sessionStorage.getItem('froglet-public-demo.pending.v1')).toBeNull();
});

test('a final navigation still disposes the compiler after earlier cached navigations', () => {
  hide(true);
  window.dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true }));
  hide(false);
  expect(mocks.dispose).toHaveBeenCalledTimes(1);
  $<HTMLButtonElement>('[data-run-program]').click();
  expect(mocks.compile).not.toHaveBeenCalled();
});

test('the public beta’s local playground link names a real page and section', () => {
  const dom = new DOMParser().parseFromString(page, 'text/html');
  const link = Array.from(dom.querySelectorAll('a')).find(a => a.textContent === 'Explore the local playground');
  expect(link).toBeDefined();
  const url = new URL(link!.getAttribute('href')!, 'https://froglet.dev');
  const target = readFileSync(resolve(src, 'pages', url.pathname.split('/').filter(Boolean).join('/') + '.astro'), 'utf8');
  const targetDom = new DOMParser().parseFromString(target, 'text/html');
  expect(targetDom.getElementById(url.hash.slice(1))).not.toBeNull();
});

const publicProvider = 'c7a15140cc28833978197bdef7daf30e419502f186f59ff88584f453531ea894';
const publishedProfiles = ['marketplace-provider', 'marketplace-search', 'marketplace-receipts'].map((serviceId, i) => ({ serviceId, offerId: serviceId, offerHash: String(i + 1).repeat(64), bindingHash: String(i + 4).repeat(64), moduleHash: String(i + 4).repeat(64), descriptorHash: 'a'.repeat(64), revisionHash: String(i + 7).repeat(64), operationHash: ['b', 'c', 'd'][i].repeat(64), entrypoint: 'run' }));

test('additional query controls remain hidden when the classic config does not explicitly enable pinned publications', () => {
  expect($<HTMLElement>('[data-published-tools]').hidden).toBe(true);
  expect($<HTMLButtonElement>('[data-run-published]').disabled).toBe(true);
  expect(mocks.preparePublished).not.toHaveBeenCalled();
});

test('explicit pinned config enables the selected published query with a bounded example and no compilation', async () => {
  hide(false); sessionStorage.clear(); document.body.innerHTML = markup;
  vi.mocked(fetch).mockImplementation(async (url: any) => new Response(JSON.stringify(String(url).endsWith('/config') ? { providerId: publicProvider, publishedServices: { enabled: true, profiles: publishedProfiles } } : { remaining: { deals: 1000 } })));
  const profile = publishedProfiles[0];
  const run = { providerId: publicProvider, input: { provider_id: publicProvider }, request: { kind: 'execution' }, publishedService: { profile, revision: { revision_hash: profile.revisionHash } } };
  mocks.preparePublished.mockImplementation(async (deps: any) => { deps.save(run); return run; });
  mocks.resume.mockResolvedValue({ terminal: true, status: 'succeeded', result: { provider: null }, run });
  startLiveServices();
  await vi.waitFor(() => expect($<HTMLButtonElement>('[data-run-published]').disabled).toBe(false));
  expect($<HTMLElement>('[data-published-tools]').hidden).toBe(false);
  expect(JSON.parse($<HTMLTextAreaElement>('[data-published-input]').value)).toEqual({ provider_id: publicProvider });
  $<HTMLButtonElement>('[data-run-published]').click(); await result();
  expect(mocks.preparePublished).toHaveBeenCalledWith(expect.anything(), 'marketplace-provider', expect.any(String));
  expect(mocks.compile).not.toHaveBeenCalled();
  expect($('[data-program-commitment]').textContent).toContain(profile.moduleHash);
  expect($('[data-recovery-note]').textContent).toContain('published contract');
});

test('malformed explicit config cannot enable a named tool or replace an edited classic program', async () => {
  hide(false); document.body.innerHTML = markup;
  const malformed = structuredClone(publishedProfiles); malformed[0].operationHash = 'wrong';
  vi.mocked(fetch).mockResolvedValue(new Response(JSON.stringify({ providerId: publicProvider, publishedServices: { enabled: true, profiles: malformed } })));
  startLiveServices(); await ready();
  expect($<HTMLElement>('[data-published-tools]').hidden).toBe(true);
  expect($<HTMLButtonElement>('[data-run-published]').disabled).toBe(true);
});
