import compilerUrl from '../generated/assemblyscript/asc.js?url';
import { PUBLIC_DEMO, PUBLIC_DEMO_PREFIX } from '../data/public-demo-config';
import { browserCompilerWorker, createCompiler } from './playground/compiler';
import { FUNCTIONS } from './playground/functions';
import { loadKernel, loadPlaygroundVerifier } from './playground/kernel';
import { prepareLiveRun, preparePublishedServiceRun, publishedServiceProfiles, resumeLiveRun, exportLiveEvidence, type LiveDeps, type LiveRun } from './live-service-client';
import { verify_service_link_evidence_json } from '../generated/verifier/froglet_verify.js';

const STORAGE = 'froglet-public-demo.pending.v1';
const label: Record<string, string> = { discover: 'Checking Bob’s signed offer…', quote: 'Agreeing on the exact work…', deal: 'Sending your signed request…', result: 'Bob is running the job…', verify: 'Checking the signed result…' };

export function startLiveServices(scope: ParentNode = document): void {
  const root = scope.querySelector<HTMLElement>('[data-live-demo]');
  if (!root || root.dataset.started) return;
  root.dataset.started = 'true';
  const one = <T extends HTMLElement>(selector: string) => root.querySelector<T>(selector)!;
  const status = one<HTMLElement>('[data-job-status]');
  const output = one<HTMLElement>('[data-job-result]');
  const receipt = one<HTMLButtonElement>('[data-download-receipt]');
  const retry = one<HTMLButtonElement>('[data-retry-job]');
  const runButton = one<HTMLButtonElement>('[data-run-program]');
  const queryButton = one<HTMLButtonElement>('[data-run-query]');
  const failureButton = one<HTMLButtonElement>('[data-run-failure]');
  const code = one<HTMLTextAreaElement>('[data-program-source]');
  const input = one<HTMLTextAreaElement>('[data-program-input]');
  const capacity = one<HTMLElement>('[data-demo-capacity]');
  const publishedBox = root.querySelector<HTMLElement>('[data-published-tools]');
  const publishedButton = root.querySelector<HTMLButtonElement>('[data-run-published]');
  const publishedSelect = root.querySelector<HTMLSelectElement>('[data-published-service]');
  const publishedInput = root.querySelector<HTMLTextAreaElement>('[data-published-input]');
  const compiler = createCompiler({ spawn: browserCompilerWorker, compilerUrl: new URL(compilerUrl, location.href).href });
  const adder = FUNCTIONS.find(f => f.id === 'adder')!;
  code.value = adder.code;
  input.value = adder.input;
  let pending: LiveRun | undefined;
  let deps: LiveDeps | undefined;
  let busy = false;
  let evidence: unknown;
  let disposed = false;
  const control = new AbortController();

  const save = (run: LiveRun) => {
    pending = run;
    try { sessionStorage.setItem(STORAGE, JSON.stringify(run)); }
    catch { one<HTMLElement>('[data-recovery-note]').textContent = 'Keep this tab open to recover the same job. This browser could not save its reference.'; }
  };
  const buttons = () => {
    runButton.disabled = queryButton.disabled = failureButton.disabled = busy || !deps;
    retry.disabled = busy || !deps || !pending;
    if (publishedButton) publishedButton.disabled = busy || !deps || !deps.publishedServices?.length;
  };
  const transport: LiveDeps['transport'] = async ({ method, path, body }) => {
    const response = await fetch(PUBLIC_DEMO_PREFIX + path, { method, headers: body === undefined ? {} : { 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body), cache: 'no-store', signal: control.signal });
    const text = await response.text();
    let answer: unknown;
    try { answer = JSON.parse(text); } catch { throw new Error('The provider did not return a readable response.'); }
    return { status: response.status, body: answer };
  };
  const updateCapacity = async () => {
    try {
      const response = await fetch(PUBLIC_DEMO_PREFIX + '/demo/status', { cache: 'no-store', signal: control.signal });
      const s = await response.json();
      if (!disposed && response.ok && s.remaining && Number.isSafeInteger(s.remaining.deals)) capacity.textContent = `Native allowance: ${s.remaining.deals} job admissions remaining. Shared network limits may stop access sooner. ${s.paused ? 'New jobs are paused.' : 'A restart does not refill allowances.'}`;
    } catch { /* A counter failure must not invent capacity or erase an existing result. */ }
  };

  async function finish(run: LiveRun) {
    const outcome = await resumeLiveRun(deps!, run);
    if (disposed) return;
    if (!outcome.terminal) {
      status.textContent = outcome.reason;
      one<HTMLElement>('[data-recovery-note]').textContent = 'Use “Retry this job” to keep the same signed request and avoid requesting new work.';
      return;
    }
    output.textContent = JSON.stringify(outcome.status === 'succeeded' ? outcome.result : { status: 'failed', failure: outcome.failure }, null, 2);
    status.textContent = outcome.status === 'succeeded' ? 'Result received. Signed receipt verified.'
      : outcome.failure === 'execution_limit_exceeded' || outcome.failure === 'execution_timed_out'
        ? 'The execution limit stopped this job. Signed failure receipt verified.'
        : 'The job failed. Signed failure receipt verified.';
    evidence = exportLiveEvidence(outcome);
    one<HTMLTextAreaElement>('[data-exchange-json]').value = JSON.stringify(evidence, null, 2);
    one<HTMLDetailsElement>('[data-exchange-export]').hidden = false;
    receipt.disabled = false;
    one<HTMLElement>('[data-program-commitment]').textContent = run.publishedService ? `Published program SHA-256: ${run.publishedService.profile.moduleHash}` : run.request.kind === 'wasm' ? `Program SHA-256: ${run.request.submission.workload.module_hash}` : 'The lookup used Bob’s published synthetic snapshot.';
    pending = undefined;
    try { sessionStorage.removeItem(STORAGE); } catch { /* No saved credential is involved. */ }
    one<HTMLElement>('[data-recovery-note]').textContent = outcome.status === 'succeeded'
      ? run.publishedService ? 'The browser checked the published contract, signatures and result commitment. This verifies Bob’s report, not the truth of the catalog data.' : 'The browser checked the signatures and the result commitment. This is evidence of Bob’s report, not scientific validation.'
      : 'The browser checked the signatures and the signed failure receipt. This verifies Bob’s failure report.';
    await updateCapacity();
  }

  async function run(kind: 'program' | 'catalog' | 'failure' | 'published') {
    if (busy || !deps) return;
    if (pending) {
      status.textContent = 'Recover the previous job first, or explicitly discard its local reference.';
      return;
    }
    busy = true; buttons(); receipt.disabled = true; evidence = undefined; output.textContent = '';
    one<HTMLDetailsElement>('[data-exchange-export]').hidden = true;
    one<HTMLTextAreaElement>('[data-exchange-json]').value = '';
    one<HTMLElement>('[data-program-commitment]').textContent = '';
    one<HTMLElement>('[data-recovery-note]').textContent = '';
    try {
      let module: Uint8Array | undefined;
      let text: string;
      if (kind === 'published') {
        const prepared = await preparePublishedServiceRun(deps, publishedSelect!.value, publishedInput!.value);
        if (!disposed) await finish(prepared);
        return;
      } else if (kind === 'catalog') {
        const term = one<HTMLSelectElement>('[data-query-term]').value;
        text = JSON.stringify({ op: 'select', collection: 'terminology', columns: ['source', 'target'], limit: 100, ...(term ? { equals: { source: term } } : {}) });
      } else {
        const source = kind === 'failure' ? FUNCTIONS.find(f => f.id === 'never-ends')!.code : code.value;
        status.textContent = 'Compiling your program in this browser…';
        const compiled = await compiler.compile(source);
        if (!compiled.ok) throw new Error(compiled.kind === 'errors' ? compiled.diagnostics.map(d => `${d.line ? `Line ${d.line}: ` : ''}${d.message}`).join('\n') : compiled.kind === 'imports' ? compiled.message : compiled.message);
        module = compiled.module;
        text = kind === 'failure' ? '{}' : input.value;
      }
      if (disposed) return;
      const prepared = await prepareLiveRun(deps, text, module);
      await finish(prepared);
    } catch (error) { if (!disposed) status.textContent = error instanceof Error ? error.message : 'The job could not be prepared.'; }
    finally { busy = false; buttons(); }
  }

  runButton.addEventListener('click', () => void run('program'), { signal: control.signal });
  queryButton.addEventListener('click', () => void run('catalog'), { signal: control.signal });
  failureButton.addEventListener('click', () => void run('failure'), { signal: control.signal });
  publishedButton?.addEventListener('click', () => void run('published'), { signal: control.signal });
  const publishedExample = () => {
    if (publishedInput && publishedSelect) publishedInput.value = JSON.stringify(publishedSelect.value === 'marketplace-provider' ? { provider_id: PUBLIC_DEMO.providerId } : publishedSelect.value === 'marketplace-search' ? { provider_id: PUBLIC_DEMO.providerId, availability: 'healthy', limit: 10 } : { provider_id: PUBLIC_DEMO.providerId, limit: 10 }, null, 2);
  };
  publishedSelect?.addEventListener('change', () => { if (!busy) publishedExample(); }, { signal: control.signal });
  retry.addEventListener('click', async () => {
    if (busy || !deps || !pending) return;
    busy = true; buttons();
    try { await finish(pending); } catch (error) { status.textContent = error instanceof Error ? error.message : 'Recovery is not available yet.'; }
    finally { busy = false; buttons(); }
  }, { signal: control.signal });
  one<HTMLButtonElement>('[data-discard-job]').addEventListener('click', () => {
    if (busy) return;
    pending = undefined;
    try { sessionStorage.removeItem(STORAGE); } catch { /* Best effort for a browser-only reference. */ }
    status.textContent = 'Local reference discarded. A new run is new work; an already submitted job may still have completed.';
    buttons();
  }, { signal: control.signal });
  one<HTMLButtonElement>('[data-reset-program]').addEventListener('click', () => { if (!busy) { code.value = adder.code; input.value = adder.input; } }, { signal: control.signal });
  receipt.addEventListener('click', () => {
    if (!evidence) return;
    const url = URL.createObjectURL(new Blob([JSON.stringify(evidence, null, 2)], { type: 'application/json' }));
    const link = document.createElement('a'); link.href = url; link.download = 'froglet-signed-exchange.json'; document.body.append(link); link.click(); link.remove();
    one<HTMLDetailsElement>('[data-exchange-export]').open = true;
    setTimeout(() => URL.revokeObjectURL(url), 10000);
  }, { signal: control.signal });

  buttons();
  void (async () => {
    try {
      const configResponse = await fetch(PUBLIC_DEMO_PREFIX + '/config', { cache: 'no-store', signal: control.signal });
      const config = await configResponse.json();
      if (!configResponse.ok) throw new Error(config.error ?? 'The hosted demo is not active yet.');
      const [kernel, verifier] = await Promise.all([loadKernel(), loadPlaygroundVerifier()]);
      if (disposed) return;
      const publishedServices = publishedServiceProfiles(config);
      deps = { kernel, verifier, transport, providerId: config.providerId, save, publishedServices, verifyPublication: (revision, offer, descriptor) => JSON.parse(verify_service_link_evidence_json(JSON.stringify({ publication_revision: revision, offer, descriptor }))), onStep: step => { if (!disposed) status.textContent = label[step]; } };
      if (publishedBox && publishedSelect && publishedInput && publishedServices.length) {
        publishedBox.hidden = false;
        const selected = new URL(location.href).searchParams.getAll('service');
        if (selected.length === 1 && publishedServices.some(p => p.serviceId === selected[0])) publishedSelect.value = selected[0];
        publishedExample();
        one<HTMLElement>('[data-published-status]').textContent = 'The published program and operation are pinned. Your browser verifies the current contract before requesting work.';
      }
      try {
        const saved = sessionStorage.getItem(STORAGE);
        if (saved) { const run = JSON.parse(saved); if (run.providerId === config.providerId) pending = run; }
      } catch { /* The requester validates a saved job before it is sent. */ }
      status.textContent = pending ? 'A previous job can be recovered. Use “Retry this job”.' : 'Ready. Choose a job and run it on Bob’s provider.';
      buttons();
      await updateCapacity();
    } catch (error) { if (!disposed) status.textContent = error instanceof Error ? error.message : 'The hosted demo is unavailable.'; }
  })();
  window.addEventListener('pagehide', event => {
    // A cached document resumes with the same controls, edits and pending job.
    // Abort only when it is actually leaving; Back must retain working handlers.
    if (event.persisted) return;
    disposed = true; control.abort(); compiler.dispose();
  }, { signal: control.signal });
}
