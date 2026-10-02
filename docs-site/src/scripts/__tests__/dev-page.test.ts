import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { initSync } from '../../generated/verifier/froglet_verify.js';
import { describeReport, initDevPage, initDevVerify } from '../dev-page';
import { sampleJson, verifyJson } from '../receipt-verifier';
import { repoRoot, src } from './route-helpers';

const page = readFileSync(resolve(src, 'pages/open-source.astro'), 'utf8');
const fixture = JSON.parse(readFileSync(resolve(repoRoot, 'conformance/kernel_v1.json'), 'utf8'));
const widget = page.match(/<div class="dev-verify" data-dev-verify>[\s\S]*?<\/noscript>\s*<\/div>/)?.[0] ?? '';

beforeAll(() => {
  initSync({ module: readFileSync('src/generated/verifier/froglet_verify_bg.wasm') });
});
afterEach(() => { document.body.innerHTML = ''; vi.restoreAllMocks(); vi.resetModules(); vi.doUnmock('../receipt-verifier'); });

describe('what the developer panel says about a report', () => {
  it('calls the unchanged sample chain verified, with every artifact verified', () => {
    const view = describeReport(verifyJson(sampleJson()), false);
    expect(view.status).toBe('verified');
    expect(view.summary).toBe('Verified artifact chain, including receipt.');
    expect(view.rows.map((row) => row.type)).toEqual(['descriptor', 'offer', 'quote', 'deal', 'receipt']);
    expect(view.rows.every((row) => row.status === 'verified')).toBe(true);
    expect(view.change).toBeUndefined();
  });

  it('calls the changed receipt invalid and names exactly what was changed', () => {
    const changed = JSON.parse(sampleJson(true));
    expect(changed.artifacts[4].payload.deal_hash).toBe('0'.repeat(64));
    const view = describeReport(verifyJson(sampleJson(true)), true);
    expect(view.status).toBe('invalid');
    expect(view.rows.find((row) => row.type === 'receipt')?.status).toBe('invalid');
    expect(view.rows.filter((row) => row.status !== 'verified').map((row) => row.type)).toEqual(['receipt']);
    expect(view.change).toBe('Changed before verifying: receipt.payload.deal_hash was set to 64 zeros.');
  });

  it('calls a lone receipt incomplete rather than valid or invalid', () => {
    const sample = JSON.parse(sampleJson());
    const view = describeReport(verifyJson(JSON.stringify(sample.artifacts[4])), false);
    expect(view.status).toBe('incomplete');
  });
});

describe('the verifier panel on the page', () => {
  const output = () => document.querySelector<HTMLElement>('[data-dev-verify-output]')!;
  const badge = () => document.querySelector<HTMLElement>('[data-dev-verify-badge]')!;
  const buttons = () => Array.from(document.querySelectorAll<HTMLButtonElement>('[data-dev-verify-run]'));
  const click = (mode: string) => document.querySelector<HTMLButtonElement>(`[data-dev-verify-run="${mode}"]`)!.click();
  const settle = async () => { await vi.waitFor(() => expect(output().hasAttribute('aria-busy')).toBe(false)); };
  beforeEach(() => { document.body.innerHTML = widget; initDevVerify(); });

  it('is taken from the page markup and starts idle', () => {
    expect(widget.length).toBeGreaterThan(300);
    expect(badge().dataset.status).toBe('idle');
    expect(output().textContent).toBe('Nothing verified yet.');
    expect(buttons()).toHaveLength(2);
    expect(output().getAttribute('role')).toBe('status');
    expect(output().getAttribute('aria-live')).toBe('polite');
  });

  it('verifies the sample chain, then shows a changed receipt failing', async () => {
    click('sample'); await settle();
    expect(output().dataset.status).toBe('verified');
    expect(badge().textContent).toBe('valid');
    expect(output().textContent).toContain('Verified artifact chain, including receipt.');
    expect(output().querySelectorAll('.dev-verify__rows li')).toHaveLength(5);
    click('tamper'); await settle();
    expect(output().dataset.status).toBe('invalid');
    expect(badge().textContent).toBe('invalid');
    expect(output().textContent).toContain('receipt.payload.deal_hash was set to 64 zeros');
    expect(output().querySelector('.dev-verify__rows li[data-status="invalid"] code')?.textContent).toBe('receipt');
    click('sample'); await settle();
    expect(output().dataset.status).toBe('verified');
    expect(output().textContent).not.toContain('64 zeros');
  });

  it('disables both buttons while a check runs, so two checks cannot overlap, then re-enables them', async () => {
    click('sample');
    expect(buttons().every((button) => button.disabled)).toBe(true);
    expect(output().getAttribute('aria-busy')).toBe('true');
    click('tamper');
    await settle();
    expect(output().dataset.status).toBe('verified');
    expect(buttons().every((button) => !button.disabled)).toBe(true);
  });

  it('offers the full report as text, never as markup', async () => {
    click('sample'); await settle();
    const report = output().querySelector('details pre')!;
    expect(JSON.parse(report.textContent!).valid).toBe(true);
    expect(output().querySelector('script, img')).toBeNull();
  });

  it('tells the reader to use the command line when the verifier cannot load', async () => {
    document.body.innerHTML = widget;
    vi.resetModules();
    vi.doMock('../receipt-verifier', async (importOriginal) => ({ ...(await importOriginal<typeof import('../receipt-verifier')>()), loadVerifier: () => Promise.reject(new Error('network down')) }));
    const { initDevVerify: init } = await import('../dev-page');
    init();
    click('sample'); await settle();
    expect(output().dataset.status).toBe('unavailable');
    expect(output().textContent).toContain('network down');
    expect(output().textContent).toContain('command line');
    expect(badge().dataset.status).toBe('unavailable');
    expect(buttons().every((button) => !button.disabled)).toBe(true);
  });

  it('does nothing on a page without the panel', () => {
    document.body.innerHTML = '<p>No panel</p>';
    expect(() => initDevVerify()).not.toThrow();
  });
});

describe('copy buttons', () => {
  it('copy the exact text of their terminal, and say so', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    const command = 'cargo run -p froglet-verify -- conformance/kernel_v1.json';
    document.body.innerHTML = `<figure class="dev-term"><figcaption><button type="button" data-copy="${command}">Copy</button></figcaption><pre><code>${command}</code></pre></figure>`;
    initDevPage();
    const button = document.querySelector<HTMLButtonElement>('[data-copy]')!;
    button.click();
    await vi.waitFor(() => expect(button.textContent).toBe('Copied'));
    expect(writeText).toHaveBeenCalledWith(command);
  });
});

describe('the feed example on the page', () => {
  it('shows what the verifier reports for the captured page, artifact by artifact', () => {
    const served = readFileSync(resolve(repoRoot, 'froglet-verify/tests/fixtures/node_feed_page.json'), 'utf8');
    const line = (type: string, hash: string) => `[ok ] ${type.padEnd(14)} ${hash.slice(0, 12)}  envelope + semantics verified`;
    // What the page prints is built from the captured page in its frontmatter, from each entry's document.
    const printed = JSON.parse(served).artifacts.map((entry: { document: { artifact_type: string; hash: string } }) => line(String(entry.document.artifact_type), String(entry.document.hash)));
    expect(page).toContain('${String(entry.document.artifact_type).padEnd(14)} ${String(entry.document.hash).slice(0, 12)}  envelope + semantics verified');
    // It must be what the verifier itself reports for that page: every artifact verified, same type and hash.
    const report = verifyJson(served);
    expect(report.error).toBeUndefined();
    expect(report.artifacts.every((artifact) => artifact.status === 'verified')).toBe(true);
    expect(report.artifacts.map((artifact) => line(artifact.artifact_type, artifact.hash))).toEqual(printed);
    // A feed is not a whole deal, so no chain is claimed, which is why the output ends at the result line.
    expect(report.chain_evaluated).toBe(false);
  });
});

describe('the Wasm host on the page', () => {
  it('runs the conformance module exactly as the page shows it, and returns its result', async () => {
    const snippet = page.match(/const wasmHost = `([^`]*)`;/)?.[1];
    expect(snippet).toBeTruthy();
    const AsyncFunction = Object.getPrototypeOf(async function () { /* only for its constructor */ }).constructor;
    const run = new AsyncFunction('moduleBytes', 'request', `${snippet}\nreturn result;`);
    const bytes = Buffer.from(fixture.workload_spec.submission.module_bytes_hex, 'hex');
    expect(await run(bytes, fixture.workload_spec.submission.input)).toBe(42);
  });

  it('reads the result from the pointer in the high 32 bits, not from zero', async () => {
    // alloc returns 64, so the input lands at 64. run returns (32 << 32) | 2 and the result "42" sits at 32.
    // A host that shifts by the wrong amount would read the input instead, or nothing.
    const snippet = page.match(/const wasmHost = `([^`]*)`;/)?.[1];
    const AsyncFunction = Object.getPrototypeOf(async function () { /* only for its constructor */ }).constructor;
    const run = new AsyncFunction('moduleBytes', 'request', `${snippet}\nreturn result;`);
    const moduleHex = '0061736d01000000010c0260017f017f60027f7f017e03030200010503010001071803066d656d6f7279020005616c6c6f6300000372756e00010a1102050041c0000b0900428280808080040b0b08010041200b023432';
    expect(await run(Buffer.from(moduleHex, 'hex'), { anything: true })).toBe(42);
  });

  it('declares no imports, which is what the ABI requires', async () => {
    const bytes = Buffer.from(fixture.workload_spec.submission.module_bytes_hex, 'hex');
    const module = await WebAssembly.compile(bytes);
    expect(WebAssembly.Module.imports(module)).toEqual([]);
    expect(WebAssembly.Module.exports(module).map((entry) => entry.name).sort()).toEqual(['alloc', 'memory', 'run']);
  });
});
