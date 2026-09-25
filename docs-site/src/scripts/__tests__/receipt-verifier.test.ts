import { readFileSync } from 'node:fs';
import { beforeAll, beforeEach, afterEach, describe, expect, it, vi } from 'vitest';
import { initSync } from '../../generated/verifier/froglet_verify.js';
import sample from '../../generated/verifier/free-chain.json';
import { initReceiptVerifier, reportSummary, sampleJson, verifyJson } from '../receipt-verifier';

beforeAll(() => {
  initSync({ module: readFileSync('src/generated/verifier/froglet_verify_bg.wasm') });
});

describe('guided verification interactions', () => {
  const element = <T extends HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
  const click = (name: string) => element<HTMLButtonElement>(`[data-receipt-${name}]`).click();
  const output = () => element<HTMLElement>('[data-receipt-output]');
  const input = () => element<HTMLTextAreaElement>('[data-receipt-input]');
  const settle = async () => { await vi.waitFor(() => expect(output().hasAttribute('aria-busy')).toBe(false)); };
  beforeEach(() => {
    document.body.innerHTML = `<button data-receipt-sample>Sample</button><button data-receipt-tamper>Tamper</button>
      <textarea data-receipt-input></textarea><button data-receipt-verify>Verify</button>
      <div data-receipt-output tabindex="-1"></div><button data-receipt-copy disabled>Copy</button>
      <button data-receipt-download disabled>Download</button><button data-receipt-clear>Clear</button>
      <p data-receipt-export-status></p><details data-receipt-export hidden><textarea data-receipt-export-text readonly></textarea></details>`;
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: vi.fn().mockResolvedValue(undefined) } });
    initReceiptVerifier();
  });
  afterEach(() => { document.body.innerHTML = ''; vi.restoreAllMocks(); });
  it('checks real unchanged and tampered examples, brings results into focus, and recovers', async () => {
    click('sample'); await settle();
    expect(output().dataset.status).toBe('verified');
    expect(document.activeElement).toBe(output());
    click('tamper'); await settle();
    expect(output().dataset.status).toBe('invalid');
    expect(output().textContent).toContain('Reported result (receipt): invalid');
    expect(output().textContent).toContain('Get the original signed records');
    click('sample'); await settle();
    expect(output().dataset.status).toBe('verified');
    expect(sampleJson()).toBe(JSON.stringify(sample, null, 2));
  });
  it('does not publish or export a stale result when the input changes during initialization', async () => {
    click('sample');
    input().value = '{'; input().dispatchEvent(new Event('input'));
    await Promise.resolve(); await Promise.resolve();
    expect(output().dataset.status).toBe('pending');
    expect(element<HTMLButtonElement>('[data-receipt-download]').disabled).toBe(true);
    expect(element<HTMLButtonElement>('[data-receipt-copy]').disabled).toBe(true);
    expect(output().textContent).not.toContain('Verified');
  });
  it('keeps the latest sample result when verification attempts overlap', async () => {
    click('sample'); click('tamper'); await settle();
    expect(output().dataset.status).toBe('invalid');
    expect(output().textContent).toContain('receipt): invalid');
  });
  it('gives recovery instructions for malformed input and missing chain members', async () => {
    input().value = '{'; click('verify'); await settle();
    expect(output().dataset.status).toBe('invalid');
    expect(input().getAttribute('aria-invalid')).toBe('true');
    expect(output().textContent).toContain('Paste complete JSON');
    input().value = JSON.stringify(sample.artifacts[4]); click('verify'); await settle();
    expect(output().dataset.status).toBe('incomplete');
    expect(output().textContent).toContain('Include the matching provider descriptor');
  });
  it('offers selectable report text if clipboard access is denied', async () => {
    vi.mocked(navigator.clipboard.writeText).mockRejectedValue(new Error('denied'));
    click('sample'); await settle(); click('copy');
    await vi.waitFor(() => expect(element<HTMLElement>('[data-receipt-export]').hidden).toBe(false));
    const text = element<HTMLTextAreaElement>('[data-receipt-export-text]');
    expect(JSON.parse(text.value).report.valid).toBe(true);
    expect(text.selectionEnd - text.selectionStart).toBe(text.value.length);
  });
  it('clears pasted data, report text, and pending verification', async () => {
    click('sample'); click('clear');
    await Promise.resolve(); await Promise.resolve();
    expect(input().value).toBe('');
    expect(output().dataset.status).toBe('empty');
    expect(element<HTMLButtonElement>('[data-receipt-copy]').disabled).toBe(true);
    expect(element<HTMLTextAreaElement>('[data-receipt-export-text]').value).toBe('');
  });
});

describe('browser WASM verifier against the public canonical chain', () => {
  it('verifies the complete free chain and identifies the canonical receipt after the Deal', () => {
    const report = verifyJson(JSON.stringify(sample));
    expect(report.valid).toBe(true);
    expect(report.chain_evaluated).toBe(true);
    expect(report.artifacts.map(a => a.artifact_type)).toEqual(['descriptor', 'offer', 'quote', 'deal', 'receipt']);
    expect(report.artifacts.every(a => a.envelope_valid)).toBe(true);
    expect(reportSummary(report)).toContain('including receipt');
  });
  it('fails cryptographic verification after the receipt payload is tampered with', () => {
    const changed = structuredClone(sample);
    changed.artifacts[4].payload.deal_hash = '0'.repeat(64);
    const report = verifyJson(JSON.stringify(changed));
    expect(report.valid).toBe(false);
    expect(report.artifacts[4].envelope_valid).toBe(false);
    expect(reportSummary(report)).toContain('Verification failed');
  });
  it('reports a valid standalone receipt without claiming a complete chain', () => {
    const report = verifyJson(JSON.stringify(sample.artifacts[4]));
    expect(report.valid).toBe(false);
    expect(report.chain_evaluated).toBe(false);
    expect(report.artifacts[0].status).toBe('verified');
    expect(reportSummary(report)).toContain('complete, unambiguous chain was not supplied');
  });
  it('reports duplicate receipts as ambiguous rather than selecting the first', () => {
    const report = verifyJson(JSON.stringify({ artifacts: [...sample.artifacts, sample.artifacts[4]] }));
    expect(report.valid).toBe(false);
    expect(report.chain_evaluated).toBe(false);
    expect(report.structure_errors).toContain('more than one receipt artifact supplied');
  });
  it('handles empty and malformed input without presenting a success', () => {
    expect(reportSummary(verifyJson('{'))).toContain('Could not verify');
    expect(reportSummary(verifyJson('{"artifacts":[]}'))).toBe('No artifacts supplied.');
  });
});
