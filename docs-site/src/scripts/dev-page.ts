import { initCopyButtons } from './clipboard';
import { loadVerifier, reportSummary, sampleJson, verifyJson, type VerificationReport } from './receipt-verifier';

export interface VerifyView {
  status: 'verified' | 'invalid' | 'incomplete';
  summary: string;
  rows: Array<{ type: string; status: string }>;
  change?: string;
}

/** What the developer panel shows for a verifier report. Pure, so it is tested without a browser. */
export function describeReport(report: VerificationReport, tampered: boolean): VerifyView {
  const invalid = Boolean(report.error) || Boolean(report.artifacts?.some((artifact) => artifact.status === 'invalid'));
  return {
    status: report.valid ? 'verified' : invalid ? 'invalid' : 'incomplete',
    summary: reportSummary(report),
    rows: (report.artifacts ?? []).map((artifact) => ({ type: artifact.artifact_type, status: artifact.status.replaceAll('_', ' ') })),
    change: tampered ? 'Changed before verifying: receipt.payload.deal_hash was set to 64 zeros.' : undefined,
  };
}

function render(output: HTMLElement, view: VerifyView, report: VerificationReport): void {
  const summary = document.createElement('p');
  summary.className = 'dev-verify__summary';
  summary.textContent = view.summary;

  const list = document.createElement('ul');
  list.className = 'dev-verify__rows';
  for (const row of view.rows) {
    const item = document.createElement('li');
    item.dataset.status = row.status;
    const type = document.createElement('code');
    type.textContent = row.type;
    item.append(type, ` ${row.status}`);
    list.append(item);
  }

  const parts: Node[] = [summary];
  if (view.change) {
    const change = document.createElement('p');
    change.className = 'dev-verify__change';
    change.textContent = view.change;
    parts.push(change);
  }
  parts.push(list);

  const full = document.createElement('details');
  const label = document.createElement('summary');
  label.textContent = 'Full report (JSON)';
  const pre = document.createElement('pre');
  pre.tabIndex = 0;
  pre.textContent = JSON.stringify(report, null, 2);
  full.append(label, pre);
  parts.push(full);

  output.replaceChildren(...parts);
  output.dataset.status = view.status;
}

/** The panel that verifies the sample chain with the WebAssembly build of froglet-verify. */
export function initDevVerify(root: ParentNode = document): void {
  const panel = root.querySelector<HTMLElement>('[data-dev-verify]');
  const output = panel?.querySelector<HTMLElement>('[data-dev-verify-output]');
  if (!panel || !output) return;
  const badge = panel.querySelector<HTMLElement>('[data-dev-verify-badge]');
  const buttons = Array.from(panel.querySelectorAll<HTMLButtonElement>('[data-dev-verify-run]'));
  let revision = 0;

  const setBadge = (status: string, text: string) => {
    if (!badge) return;
    badge.dataset.status = status;
    badge.textContent = text;
  };

  const run = async (tampered: boolean) => {
    revision += 1;
    const current = revision;
    for (const button of buttons) button.disabled = true;
    output.setAttribute('aria-busy', 'true');
    output.dataset.status = 'loading';
    output.textContent = 'Loading the verifier (WebAssembly)…';
    setBadge('loading', 'loading');
    try {
      await loadVerifier();
      if (current !== revision) return;
      const report = verifyJson(sampleJson(tampered));
      const view = describeReport(report, tampered);
      render(output, view, report);
      setBadge(view.status, view.status === 'verified' ? 'valid' : view.status);
    } catch (error) {
      if (current !== revision) return;
      output.dataset.status = 'unavailable';
      output.textContent = `The verifier could not load (${error instanceof Error ? error.message : String(error)}). Use the command line below instead.`;
      setBadge('unavailable', 'unavailable');
    } finally {
      if (current === revision) {
        for (const button of buttons) button.disabled = false;
        output.removeAttribute('aria-busy');
      }
    }
  };

  for (const button of buttons) {
    button.addEventListener('click', () => void run(button.dataset.devVerifyRun === 'tamper'));
  }
}

export function initDevPage(): void {
  initCopyButtons();
  initDevVerify();
}
