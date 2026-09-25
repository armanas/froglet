import init, { validate_chain_json } from '../generated/verifier/froglet_verify.js';
import sample from '../generated/verifier/free-chain.json';

export interface VerificationReport {
  valid: boolean;
  chain_evaluated: boolean;
  structure_errors?: string[];
  error?: string;
  artifacts: { artifact_type: string; hash: string; signer: string; status: string; envelope_valid: boolean; detail?: string[] }[];
  chain?: unknown;
}

export function reportSummary(report: VerificationReport): string {
  if (report.error) return `Could not verify: ${report.error}`;
  if (!report.artifacts.length) return 'No artifacts supplied.';
  if (report.artifacts.some(a => a.status === 'invalid')) return 'Verification failed. At least one artifact is invalid.';
  if (report.valid) return report.artifacts.some(a => a.artifact_type === 'receipt')
    ? 'Verified artifact chain, including receipt.' : 'Verified agreement chain. No receipt supplied.';
  if (!report.chain_evaluated) return 'Artifact checks complete. A complete, unambiguous chain was not supplied.';
  return 'Chain verification failed. Review the report below.';
}

export function verifyJson(json: string): VerificationReport {
  // Historical evidence is checked without a wall clock; expiration is not an
  // assertion that an old receipt's signature or historical agreement is invalid.
  return JSON.parse(validate_chain_json(json));
}

export function sampleJson(tampered = false): string {
  const example = structuredClone(sample);
  if (tampered) example.artifacts[4].payload.deal_hash = '0'.repeat(64);
  return JSON.stringify(example, null, 2);
}

const ARTIFACT_LABELS: Record<string, string> = {
  descriptor: 'Provider details', offer: 'Service offer', quote: 'Price and limits',
  deal: 'Accepted agreement', receipt: 'Reported result', invoice_bundle: 'Payment invoices',
};

export function reportGuidance(report: VerificationReport): string {
  if (report.error) return 'Paste complete JSON without Markdown fences, or select Verify sample chain to start again.';
  if (!report.artifacts.length) return 'No signed records were found. Paste a Froglet artifact or try the sample above.';
  if (report.artifacts.some(a => a.status === 'invalid')) return 'This evidence cannot be trusted as supplied. Get the original signed records from the sender. For the demo, Verify sample chain restores the unchanged example.';
  if (report.valid) return 'The supplied signatures and links match. This does not establish that the reported work is correct or that money moved. Try Check tampered sample to see a changed record rejected.';
  return 'Include the matching provider descriptor, offer, quote, deal, and receipt. Paid chains may also need an invoice bundle. Remove duplicate records, then verify again.';
}

export function initReceiptVerifier(): void {
  const input = document.querySelector<HTMLTextAreaElement>('[data-receipt-input]');
  const output = document.querySelector<HTMLElement>('[data-receipt-output]');
  const button = document.querySelector<HTMLButtonElement>('[data-receipt-verify]');
  const download = document.querySelector<HTMLButtonElement>('[data-receipt-download]');
  if (!input || !output || !button || !download) return;
  const copy = document.querySelector<HTMLButtonElement>('[data-receipt-copy]');
  const exportStatus = document.querySelector<HTMLElement>('[data-receipt-export-status]');
  const exportDetails = document.querySelector<HTMLDetailsElement>('[data-receipt-export]');
  const exportText = document.querySelector<HTMLTextAreaElement>('[data-receipt-export-text]');
  let lastReport: VerificationReport | undefined;
  let initializing: Promise<unknown> | undefined;
  let revision = 0;
  const invalidate = () => {
    revision += 1; lastReport = undefined; download.disabled = true;
    if (copy) copy.disabled = true;
    if (exportStatus) exportStatus.textContent = '';
    if (exportDetails) { exportDetails.hidden = true; exportDetails.open = false; }
    if (exportText) exportText.value = '';
    input.removeAttribute('aria-invalid');
    output.removeAttribute('aria-busy');
    button.disabled = false;
  };
  const verify = async () => {
    invalidate();
    const current = revision;
    const json = input.value;
    if (!json.trim()) {
      output.textContent = 'Paste your signed JSON first, or select Verify sample chain above.';
      output.dataset.status = 'empty'; input.setAttribute('aria-invalid', 'true');
      input.focus(); return;
    }
    button.disabled = true;
    output.setAttribute('aria-busy', 'true');
    output.dataset.status = 'loading';
    output.textContent = 'Loading local verifier…';
    try {
      await (initializing ??= init().catch(error => { initializing = undefined; throw error; }));
      // Editing or replacing the input cancels this attempt, including while WASM loads.
      if (current !== revision) return;
      const report = verifyJson(json);
      const summary = document.createElement('p'); summary.textContent = reportSummary(report);
      summary.className = 'verify-summary';
      const guidance = document.createElement('p'); guidance.textContent = reportGuidance(report);
      const artifacts = document.createElement('ul');
      for (const artifact of report.artifacts || []) {
        const row = document.createElement('li');
        row.textContent = `${ARTIFACT_LABELS[artifact.artifact_type] || artifact.artifact_type} (${artifact.artifact_type}): ${artifact.status.replaceAll('_', ' ')}${artifact.detail?.length ? ` — ${artifact.detail.join('; ')}` : ''}`;
        artifacts.append(row);
      }
      const issues = document.createElement('p'); issues.textContent = (report.structure_errors || []).join(' ');
      const full = document.createElement('details');
      const label = document.createElement('summary'); label.textContent = 'Full verification report';
      const detail = document.createElement('pre'); detail.textContent = JSON.stringify(report, null, 2);
      full.append(label, detail);
      output.replaceChildren(summary, guidance, artifacts, issues, full);
      output.dataset.status = report.valid ? 'verified' : report.error || report.artifacts?.some(a => a.status === 'invalid') ? 'invalid' : 'incomplete';
      lastReport = report; download.disabled = false;
      if (copy) copy.disabled = false;
      input.setAttribute('aria-invalid', String(Boolean(report.error)));
      // On mobile the result must be brought into view after verifying custom input.
      output.focus();
    } catch (error) {
      if (current !== revision) return;
      output.textContent = `The local verifier could not load. Check your connection and try again. Your input is still here. Details: ${error instanceof Error ? error.message : String(error)}`;
      output.dataset.status = 'unavailable';
      output.focus();
    } finally {
      if (current === revision) { button.disabled = false; output.removeAttribute('aria-busy'); }
    }
  };
  document.querySelector('[data-receipt-sample]')?.addEventListener('click', () => {
    input.value = sampleJson(); void verify();
  });
  document.querySelector('[data-receipt-tamper]')?.addEventListener('click', () => {
    input.value = sampleJson(true); void verify();
  });
  button.addEventListener('click', () => { void verify(); });
  input.addEventListener('input', () => {
    invalidate();
    output.textContent = 'Input changed. Run verification again.'; output.dataset.status = 'pending';
  });
  document.querySelector('[data-receipt-clear]')?.addEventListener('click', () => {
    invalidate(); input.value = '';
    output.textContent = 'Cleared. Try the sample above or paste your own signed JSON.';
    output.dataset.status = 'empty'; output.focus();
  });
  const reportText = () => JSON.stringify({
    verification_scope: 'Local signatures, canonical hashes, artifact semantics, and supplied chain links. No live settlement or expiry check. Signatures identify keys, not people or truthful execution.',
    report: lastReport,
  }, null, 2);
  const showReportText = (text: string) => {
    if (exportDetails && exportText) {
      exportDetails.hidden = false; exportDetails.open = true;
      exportText.value = text; exportText.focus(); exportText.select();
    }
  };
  copy?.addEventListener('click', async () => {
    if (!lastReport) return;
    const current = revision; const text = reportText();
    try {
      await navigator.clipboard.writeText(text);
      if (current === revision && exportStatus) exportStatus.textContent = 'Report copied.';
    } catch {
      if (current !== revision) return;
      if (exportStatus) exportStatus.textContent = 'Clipboard access is unavailable. Copy the selected report text below.';
      showReportText(text);
    }
  });
  download.addEventListener('click', () => {
    if (!lastReport) return;
    const text = reportText();
    if (exportStatus) exportStatus.textContent = 'If your browser does not save the file, use Copy report or the report text below.';
    showReportText(text);
    const url = URL.createObjectURL(new Blob([text], { type: 'application/json' }));
    const link = document.createElement('a'); link.href = url; link.download = 'froglet-verification-report.json';
    link.hidden = true; document.body.append(link); link.click();
    setTimeout(() => { link.remove(); URL.revokeObjectURL(url); }, 1000);
  });
}
