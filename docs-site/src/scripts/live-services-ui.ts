import compilerUrl from '../generated/assemblyscript/asc.js?url';
import { PUBLIC_DEMO_PREFIX } from '../data/public-demo-config';
import { browserCompilerWorker, createCompiler } from './playground/compiler';
import { FUNCTIONS } from './playground/functions';
import { loadKernel, loadPlaygroundVerifier } from './playground/kernel';
import { prepareLiveRun, resumeLiveRun, exportLiveEvidence, type LiveDeps, type LiveRun } from './live-service-client';

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
      if (!disposed && response.ok && s.remaining && Number.isSafeInteger(s.remaining.deals)) capacity.textContent = `${s.remaining.deals} jobs left in this shared beta allowance. ${s.paused ? 'New jobs are paused.' : 'A restart does not refill it.'}`;
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
    one<HTMLElement>('[data-program-commitment]').textContent = run.request.kind === 'wasm' ? `Program SHA-256: ${run.request.submission.workload.module_hash}` : 'The lookup used Bob’s published synthetic snapshot.';
    pending = undefined;
    try { sessionStorage.removeItem(STORAGE); } catch { /* No saved credential is involved. */ }
    one<HTMLElement>('[data-recovery-note]').textContent = 'The browser checked the signatures and the result commitment. This is evidence of Bob’s report, not scientific validation.';
    await updateCapacity();
  }

  async function run(kind: 'program' | 'catalog' | 'failure') {
    if (busy || !deps) return;
    if (pending) {
      status.textContent = 'Recover the previous job first, or explicitly discard its local reference.';
      return;
    }
    busy = true; buttons(); receipt.disabled = true; evidence = undefined; output.textContent = '';
    one<HTMLDetailsElement>('[data-exchange-export]').hidden = true;
    one<HTMLTextAreaElement>('[data-exchange-json]').value = '';
    one<HTMLElement>('[data-program-commitment]').textContent = '';
    try {
      let module: Uint8Array | undefined;
      let text: string;
      if (kind === 'catalog') {
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
      deps = { kernel, verifier, transport, providerId: config.providerId, save, onStep: step => { if (!disposed) status.textContent = label[step]; } };
      try {
        const saved = sessionStorage.getItem(STORAGE);
        if (saved) { const run = JSON.parse(saved); if (run.providerId === config.providerId) pending = run; }
      } catch { /* The requester validates a saved job before it is sent. */ }
      status.textContent = pending ? 'A previous job can be recovered. Use “Retry this job”.' : 'Ready. Choose a job and run it on Bob’s provider.';
      buttons();
      await updateCapacity();
    } catch (error) { if (!disposed) status.textContent = error instanceof Error ? error.message : 'The hosted demo is unavailable.'; }
  })();
  window.addEventListener('pagehide', () => { disposed = true; control.abort(); compiler.dispose(); }, { once: true });
}
