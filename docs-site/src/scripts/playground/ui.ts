import { copyToClipboard } from '../clipboard';
import { CONTRACT, type CompileResult, type Compiler } from './compiler';
import { runExchange, type Step } from './consumer';
import { describeChain, describeCompile, describeEvent, describeOutcome, SIGNER, short, sizeLabel, STEPS, type ProblemsView } from './describe';
import { createEditor } from './editor';
import type { FunctionInfo } from './functions';
import { PLAYGROUND_LIMITS, Refusal, WASM_PAGE_BYTES } from './protocol';
import { createProvider, type Provider, type ProviderEvent, type PublishedService } from './provider';
import type { ChainReport, Identity, Kernel, Limits, ModuleRunner, ServiceRecord, SignedArtifact, Verifier } from './types';
import { createWire, type Wire, type WireEntry } from './wire';

export { describeChain, describeCompile, describeEvent, describeOutcome, short, sizeLabel, STEPS } from './describe';

// The page's two-sided playground: Bob publishes a function, which he writes in an editor or uploads, and Alice calls it. The
// page shows every message, the evidence, and what each side refused. The logic is in provider.ts, consumer.ts and
// compiler.ts; this file draws it. Everything shown from the exchange (results, names, errors, the compiler's messages) is
// put on the page as text, never as markup.

export interface PlaygroundDeps {
  loadKernel(): Promise<Kernel>;
  loadVerifier(): Promise<Verifier>;
  runModule: ModuleRunner;
  /** The functions Bob can start from: source that the compiler turns into a module. */
  functions: FunctionInfo[];
  compiler: Compiler;
  /** Saves a file for the person. */
  download?: (name: string, bytes: Uint8Array) => void;
  /** Bob's limits; the page uses the playground's own. */
  limits?: Limits;
  pollIntervalMs?: number;
  now?: () => number;
  waitForNextSecond?: () => Promise<void>;
}

const megabytes = (bytes: number) => `${bytes / 1_048_576} MiB`;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string> = {}, ...children: Array<Node | string>): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attrs)) node.setAttribute(name, value);
  node.append(...children);
  return node;
}

interface Session {
  kernel: Kernel;
  verifier: Verifier;
  provider: Provider;
  wire: Wire;
  alice: Identity;
}

type Choice = { kind: 'function'; fn: FunctionInfo } | { kind: 'upload' };

export function initPlayground(scope: ParentNode, deps: PlaygroundDeps): void {
  const root = scope.querySelector<HTMLElement>('[data-playground]');
  if (!root) return;
  const q = <T extends HTMLElement>(selector: string): T => {
    const node = root.querySelector<T>(selector);
    if (!node) throw new Error(`The playground markup has no ${selector}`);
    return node;
  };

  const bob = q('[data-play-bob]');
  const modeBox = q('[data-play-mode]');
  const functionsBox = q('[data-play-functions]');
  const uploadFields = q('[data-play-upload]');
  const fileInput = q<HTMLInputElement>('[data-play-file]');
  const serviceIdInput = q<HTMLInputElement>('[data-play-service-id]');
  const blurb = q('[data-play-blurb]');
  const cheatBob = q<HTMLInputElement>('[data-play-cheat-bob]');
  const publishButton = q<HTMLButtonElement>('[data-play-publish]');
  const bobStatus = q('[data-play-bob-status]');
  const bobFacts = q('[data-play-facts]');
  const bobLog = q('[data-play-bob-log]');
  const editorPanel = q('[data-play-editor]');
  const sourceBox = q<HTMLTextAreaElement>('[data-play-source]');
  const gutter = q('[data-play-gutter]');
  const problems = q('[data-play-problems]');
  const resetSourceButton = q<HTMLButtonElement>('[data-play-reset-source]');
  const downloadButton = q<HTMLButtonElement>('[data-play-download]');
  const aliceStatus = q('[data-play-alice-status]');
  const aliceKey = q('[data-play-alice-key]');
  const serviceSelect = q<HTMLSelectElement>('[data-play-service]');
  const inputBox = q<HTMLTextAreaElement>('[data-play-input]');
  const cheatAlice = q<HTMLInputElement>('[data-play-cheat-alice]');
  const callButton = q<HTMLButtonElement>('[data-play-call]');
  const resetButton = q<HTMLButtonElement>('[data-play-reset]');
  const steps = q('[data-play-steps]');
  const outcomeBox = q('[data-play-outcome]');
  const wireList = q('[data-play-wire]');
  const wireCount = q('[data-play-wire-count]');
  const evidence = q('[data-play-evidence]');
  const chainBadge = q('[data-play-chain-badge]');
  const evidenceRows = q('[data-play-evidence-rows]');
  const evidenceOut = q('[data-play-evidence-out]');
  const evidenceJson = q('[data-play-evidence-json]');
  const tamperButton = q<HTMLButtonElement>('[data-play-tamper]');
  const copyButton = q<HTMLButtonElement>('[data-play-copy]');

  const editor = createEditor({ textarea: sourceBox, gutter });
  const inputs = new Map<string, string>();
  /** What the person has made of each function, so going to another function and back keeps their changes. */
  const sources = new Map<string, string>();
  const idle = { bob: 'not published', alice: 'waiting for Bob', outcome: 'Nothing called yet.', wire: 'No messages yet.', log: '' };

  let session: Session | undefined;
  let starting: Promise<Session> | undefined;
  let chain: SignedArtifact[] | undefined;
  /** The last module the compiler made, and the source it came from, so a download is always of what was compiled. */
  let compiled: { source: string; module: Uint8Array } | undefined;
  let busy = false;
  let published = false;
  let preloaded = false;

  // ── small pieces of state shown on the page ──
  const setBadge = (badge: HTMLElement, status: string, text: string) => {
    badge.dataset.status = status;
    badge.textContent = text;
  };
  const refresh = () => {
    publishButton.disabled = busy;
    callButton.disabled = busy || !published;
    resetButton.disabled = busy;
    tamperButton.disabled = busy || !chain;
    copyButton.disabled = busy || !chain;
    downloadButton.disabled = busy || !compiled || compiled.source !== editor.value;
  };
  const setOutcome = (status: string, ...parts: Array<Node | string>) => {
    outcomeBox.dataset.status = status;
    outcomeBox.replaceChildren(...parts);
  };
  const markSteps = (current: Step | null, failed?: Step | 'input', allDone = false) => {
    const at = current ? STEPS.indexOf(current) : -1;
    const failedAt = failed && failed !== 'input' ? STEPS.indexOf(failed) : -1;
    STEPS.forEach((step, index) => {
      const item = steps.querySelector<HTMLElement>(`[data-step="${step}"]`);
      if (!item) return;
      item.dataset.state = allDone ? 'done' : failedAt >= 0 ? (index < failedAt ? 'done' : index === failedAt ? 'failed' : 'pending') : index < at ? 'done' : index === at ? 'active' : 'pending';
    });
  };
  const restoreBobBadge = () => setBadge(bobStatus, published ? 'ready' : 'idle', published ? 'published' : idle.bob);

  // ── the wire: every message of the latest call, with repeated polls folded into one line ──
  let lastKey = '';
  let lastRow: { times: HTMLElement; ms: HTMLElement; count: number } | undefined;
  const clearWire = () => {
    session?.wire.clear();
    lastKey = '';
    lastRow = undefined;
    wireList.replaceChildren(el('li', { class: 'play__empty', 'data-play-empty': '' }, idle.wire));
    wireCount.textContent = '0 messages';
  };
  const showWire = (entry: WireEntry) => {
    wireList.querySelector('[data-play-empty]')?.remove();
    const status = (entry.response as { status?: unknown } | null)?.status;
    const key = `${entry.method} ${entry.path} ${entry.status} ${typeof status === 'string' ? status : ''}`;
    if (key === lastKey && lastRow) {
      lastRow.count += 1;
      lastRow.times.textContent = `× ${lastRow.count}`;
      lastRow.times.hidden = false;
      lastRow.ms.textContent = `${Math.round(entry.ms)} ms`;
    } else {
      const times = el('span', { class: 'play__times' });
      times.hidden = true;
      const ms = el('span', { class: 'play__ms' }, `${Math.round(entry.ms)} ms`);
      const bodies = el('div', { class: 'play__bodies' });
      if (entry.request !== undefined) bodies.append(el('h5', {}, 'Request'), el('pre', { tabindex: '0' }, JSON.stringify(entry.request, null, 2)));
      bodies.append(el('h5', {}, 'Response'), el('pre', { tabindex: '0' }, JSON.stringify(entry.response, null, 2)));
      const item = el(
        'li',
        { class: 'play__entry', 'data-play-entry': '' },
        el(
          'details',
          {},
          el('summary', {}, el('span', { class: 'play__n' }, String(entry.n)), el('code', { class: 'play__route' }, `${entry.method} ${entry.path}`), el('span', { class: 'play__code', 'data-ok': String(entry.status < 400) }, String(entry.status)), times, ms),
          bodies,
        ),
      );
      wireList.append(item);
      lastKey = key;
      lastRow = { times, ms, count: 1 };
    }
    const messages = wireList.querySelectorAll('[data-play-entry]').length;
    wireCount.textContent = `${entry.n} message${entry.n === 1 ? '' : 's'}${messages < entry.n ? `, ${messages} lines` : ''}`;
  };

  // ── Bob's source, and what the compiler says about it ──
  const choice = (): Choice => {
    const picked = root.querySelector<HTMLInputElement>('input[name="play-function"]:checked')?.value;
    const fn = deps.functions.find((candidate) => candidate.id === picked);
    return fn ? { kind: 'function', fn } : { kind: 'upload' };
  };

  const clearProblems = () => {
    problems.replaceChildren();
    problems.removeAttribute('data-status');
    editor.mark([]);
  };

  function showProblems(result: CompileResult) {
    const view: ProblemsView = describeCompile(result);
    const list = el('ul', { class: 'play__problem-list' });
    for (const problem of view.items) {
      const row = el('li', { 'data-severity': problem.severity });
      const { line, column } = problem;
      if (line !== null && column !== null) {
        const jump = el('button', { type: 'button', class: 'play__jump' }, `Line ${line}, column ${column}`);
        jump.addEventListener('click', () => editor.reveal({ line, column, ...(problem.endLine !== null && problem.endColumn !== null ? { endLine: problem.endLine, endColumn: problem.endColumn } : {}) }));
        row.append(jump);
      }
      row.append(el('span', {}, problem.message));
      if (problem.note) row.append(el('small', { class: 'play__problem-note' }, problem.note));
      list.append(row);
    }
    problems.replaceChildren(el('p', { class: 'play__problem-summary' }, view.summary), ...(view.items.length ? [list] : []));
    problems.dataset.status = view.status;
    editor.mark(view.items.flatMap((problem) => (problem.severity === 'error' && problem.line !== null ? [problem.line] : [])));
    const first = view.items.find((problem) => problem.severity === 'error' && problem.line !== null && problem.column !== null);
    if (first) editor.reveal({ line: first.line!, column: first.column!, ...(first.endLine !== null && first.endColumn !== null ? { endLine: first.endLine, endColumn: first.endColumn } : {}) });
  }

  /** Puts the chosen function's source in the editor, or the upload fields in its place, and says what each one is. */
  const showChoice = () => {
    const picked = choice();
    const isFunction = picked.kind === 'function';
    modeBox.dataset.playMode = picked.kind;
    editorPanel.hidden = !isFunction;
    uploadFields.hidden = isFunction;
    blurb.replaceChildren();
    if (picked.kind === 'function') {
      blurb.append(picked.fn.blurb);
      if (picked.fn.rust) blurb.append(' ', el('a', { href: picked.fn.rust, target: '_blank', rel: 'noreferrer' }, 'Compare with the Rust version'), '.');
      serviceIdInput.value = picked.fn.serviceId;
      editor.set(sources.get(picked.fn.id) ?? picked.fn.code);
      resetSourceButton.hidden = editor.value === picked.fn.code;
    } else {
      blurb.append('Any WebAssembly module that follows ', el('code', {}, 'froglet.wasm.run_json.v1'), ': it exports memory, alloc, and run, and imports nothing. It reads and returns JSON. Bob checks this before he publishes it. Compile one from Rust, Zig, C, or another language, or use the editor.');
      serviceIdInput.value = 'demo.mine';
    }
    clearProblems();
    refresh();
  };

  editor.onChange((value) => {
    const picked = choice();
    if (picked.kind === 'function') {
      sources.set(picked.fn.id, value);
      resetSourceButton.hidden = value === picked.fn.code;
    }
    // What the compiler said was about the text before this change.
    clearProblems();
    refresh();
  });

  resetSourceButton.addEventListener('click', () => {
    const picked = choice();
    if (picked.kind !== 'function') return;
    sources.delete(picked.fn.id);
    editor.set(picked.fn.code);
    resetSourceButton.hidden = true;
    clearProblems();
    refresh();
  });

  downloadButton.addEventListener('click', () => {
    if (!compiled) return;
    const name = serviceIdInput.value.trim();
    deps.download?.(`${/^[a-z0-9][a-z0-9._-]{0,63}$/.test(name) ? name : 'function'}.wasm`, compiled.module);
  });

  /** The compiler is large, so it is fetched when someone shows they mean to use it, not when the page loads. */
  const preload = () => {
    if (preloaded || choice().kind !== 'function') return;
    preloaded = true;
    deps.compiler.preload();
  };

  // ── Bob ──
  const showBobEvent = (event: ProviderEvent) => {
    if (event.type === 'executed') bobLog.textContent = describeEvent(event);
  };

  const showPublished = ({ record, sizeBytes, memory, descriptor, offer }: PublishedService, key: string, builtFrom: string) => {
    const fact = (label: string, value: string) => [el('dt', {}, label), el('dd', {}, value)];
    bobFacts.replaceChildren(
      ...fact('Service', record.service_id),
      ...fact('Built from', builtFrom),
      ...fact('Module', `${sizeLabel(sizeBytes)}, hash ${short(record.module_hash)}`),
      ...fact('Memory', `${memory.min} page${memory.min === 1 ? '' : 's'} at the start, at most ${memory.cappedTo}${memory.declaredMax === null ? ' (Bob added this maximum)' : ''}`),
      ...fact('Signed', `descriptor ${short(descriptor.hash, 10)}, offer ${short(offer.hash, 10)}`),
      ...fact('Bob’s key', short(key, 16)),
    );
    bobFacts.hidden = false;
    setBadge(bobStatus, 'ready', `published · ${record.service_id}`);
  };

  const startSession = () =>
    (starting ??= (async () => {
      const [kernel, verifier] = await Promise.all([deps.loadKernel(), deps.loadVerifier()]);
      const provider = createProvider({ kernel, verifier, runModule: deps.runModule, ...(deps.limits ? { limits: deps.limits } : {}), ...(deps.now ? { now: deps.now } : {}), onEvent: showBobEvent });
      const wire = createWire((request) => provider.handle(request), showWire);
      const alice = kernel.newIdentity();
      aliceKey.textContent = short(alice.public_key, 16);
      session = { kernel, verifier, provider, wire, alice };
      return session;
    })().catch((error) => {
      starting = undefined;
      throw error;
    }));

  /** Alice lists what Bob offers, over the wire like everything else she knows. */
  async function listServices(current: Session) {
    const response = await current.wire.transport({ method: 'GET', path: '/v1/provider/services' });
    const services = ((response.body as { services?: ServiceRecord[] }).services ?? []).filter((service) => service.publication_state === 'active');
    serviceSelect.replaceChildren(...services.map((service) => el('option', { value: service.service_id }, `${service.service_id} · ${service.summary}`)));
    return services;
  }

  /** Compiles what is in the editor. On a problem it shows where, and returns nothing: nothing is published. */
  async function compileSource(): Promise<{ module: Uint8Array; ms: number } | null> {
    const source = editor.value;
    setBadge(bobStatus, 'working', 'compiling');
    const result = await deps.compiler.compile(source, (phase) => setBadge(bobStatus, 'working', phase === 'loading' ? 'loading the compiler' : 'compiling'));
    showProblems(result);
    if (!result.ok) {
      restoreBobBadge();
      return null;
    }
    compiled = { source, module: result.module };
    setBadge(bobStatus, 'working', 'publishing');
    return { module: result.module, ms: result.ms };
  }

  async function publish() {
    if (busy) return;
    busy = true;
    refresh();
    setBadge(bobStatus, 'working', 'publishing');
    bobLog.textContent = '';
    try {
      const current = await startSession();
      const picked = choice();
      const serviceId = serviceIdInput.value.trim();
      let input: { serviceId: string; summary: string; module: Uint8Array };
      let builtFrom: string;
      if (picked.kind === 'function') {
        const made = await compileSource();
        if (!made) return;
        const changed = editor.value !== picked.fn.code;
        input = { serviceId, summary: changed ? `${picked.fn.summary} (edited)` : picked.fn.summary, module: made.module };
        builtFrom = `${changed ? 'your edited' : 'the'} source, compiled in this page in ${made.ms} ms`;
        inputs.set(serviceId, picked.fn.input);
      } else {
        const file = fileInput.files?.[0];
        if (!file) throw new Refusal(400, 'choose a .wasm file first');
        input = { serviceId, summary: `Uploaded: ${file.name}`, module: new Uint8Array(await file.arrayBuffer()) };
        builtFrom = 'a module you uploaded';
        inputs.set(serviceId, '{}');
      }
      const result = await current.provider.publish(input);
      showPublished(result, current.provider.publicKey, builtFrom);
      const services = await listServices(current);
      published = services.length > 0;
      serviceSelect.disabled = !published;
      serviceSelect.value = result.record.service_id;
      inputBox.value = inputs.get(result.record.service_id) ?? '{}';
      setBadge(aliceStatus, 'ready', 'ready');
    } catch (error) {
      setBadge(bobStatus, published ? 'ready' : 'refused', published ? 'published' : 'not published');
      bobLog.textContent = error instanceof Refusal ? `Bob refused to publish it: ${error.message}.` : `Could not publish it: ${error instanceof Error ? error.message : String(error)}`;
    } finally {
      busy = false;
      refresh();
    }
  }

  // ── Alice ──
  function showChain(artifacts: SignedArtifact[], report: ChainReport) {
    chain = artifacts;
    const { valid, text } = describeChain(report);
    setBadge(chainBadge, valid ? 'verified' : 'invalid', valid ? 'chain valid' : 'chain invalid');
    evidenceRows.replaceChildren(
      ...artifacts.map((artifact, index) => {
        const status = report.artifacts[index]?.status ?? 'invalid';
        return el('li', { 'data-status': status.replaceAll('_', ' ') }, el('code', {}, artifact.artifact_type), el('span', { class: 'play__signer' }, `signed by ${SIGNER[artifact.artifact_type]}`), el('span', { class: 'play__hash' }, short(artifact.hash)), el('span', { class: 'play__verdict' }, status.replaceAll('_', ' ')));
      }),
    );
    evidenceOut.textContent = text;
    evidenceOut.dataset.status = valid ? 'verified' : 'invalid';
    evidenceJson.textContent = JSON.stringify({ artifacts }, null, 2);
    evidence.hidden = false;
  }

  async function call() {
    if (busy || !session) return;
    const current = session;
    busy = true;
    chain = undefined;
    evidence.hidden = true;
    clearWire();
    bobLog.textContent = '';
    refresh();
    markSteps(null);
    setBadge(aliceStatus, 'working', 'calling');
    outcomeBox.setAttribute('aria-busy', 'true');
    setOutcome('working', 'Calling…');
    current.provider.setCheat(cheatBob.checked ? 'wrong-result' : 'none');
    try {
      const outcome = await runExchange({
        kernel: current.kernel,
        verifier: current.verifier,
        transport: current.wire.transport,
        requester: current.alice,
        serviceId: serviceSelect.value,
        inputText: inputBox.value,
        alterDealAfterSigning: cheatAlice.checked,
        onStep: (step) => markSteps(step),
        pollIntervalMs: deps.pollIntervalMs,
        now: deps.now,
        waitForNextSecond: deps.waitForNextSecond,
      });
      const view = describeOutcome(outcome);
      const parts: Array<Node | string> = [el('p', { class: 'play__title' }, view.title), el('p', {}, view.detail)];
      if (view.result) parts.push(el('pre', { tabindex: '0' }, view.result));
      if (view.hint) parts.push(el('p', { class: 'play__hint' }, view.hint));
      setOutcome(view.status, ...parts);
      setBadge(aliceStatus, view.status, view.status === 'verified' ? 'verified' : view.status === 'failed' ? 'function failed' : 'refused');
      markSteps(null, outcome.ok ? undefined : outcome.stage, outcome.ok);

      const artifacts = outcome.artifacts as Partial<Record<string, SignedArtifact>>;
      const all = ['descriptor', 'offer', 'quote', 'deal', 'receipt'].map((type) => artifacts[type]);
      if (all.every(Boolean)) showChain(all as SignedArtifact[], outcome.ok ? outcome.chain : current.verifier.validateChain(all));
    } catch (error) {
      setOutcome('refused', el('p', { class: 'play__title' }, 'Something went wrong'), el('p', {}, error instanceof Error ? error.message : String(error)));
      setBadge(aliceStatus, 'refused', 'error');
    } finally {
      outcomeBox.removeAttribute('aria-busy');
      busy = false;
      refresh();
    }
  }

  function tamper() {
    if (!session || !chain) return;
    const changed = structuredClone(chain);
    changed[4].payload.deal_hash = '0'.repeat(64);
    const report = session.verifier.validateChain(changed);
    const { valid, text } = describeChain(report);
    evidenceOut.textContent = `Changed before verifying: receipt.payload.deal_hash was set to 64 zeros. ${text}`;
    evidenceOut.dataset.status = valid ? 'verified' : 'invalid';
  }

  async function copy() {
    if (!chain) return;
    const ok = await copyToClipboard(JSON.stringify({ artifacts: chain }, null, 2), copyButton, { successText: 'Copied', failureText: 'Copy failed' });
    if (ok) evidenceOut.dataset.status = 'verified';
    else delete evidenceOut.dataset.status;
    evidenceOut.textContent = ok ? 'Copied. Paste it into the receipt verifier, or save it and give it to froglet-verify.' : 'The browser blocked copying. Open “The evidence as JSON” and select it there.';
  }

  function reset() {
    if (busy) return;
    session = undefined;
    starting = undefined;
    chain = undefined;
    compiled = undefined;
    published = false;
    sources.clear();
    clearWire();
    bobFacts.replaceChildren();
    bobFacts.hidden = true;
    bobLog.textContent = idle.log;
    setBadge(bobStatus, 'idle', idle.bob);
    setBadge(aliceStatus, 'idle', idle.alice);
    aliceKey.textContent = 'a new key when Bob publishes';
    serviceSelect.replaceChildren(el('option', { value: '' }, 'Publish a function first'));
    serviceSelect.disabled = true;
    inputBox.value = '';
    evidence.hidden = true;
    evidenceRows.replaceChildren();
    evidenceOut.textContent = '';
    evidenceJson.textContent = '';
    setOutcome('idle', idle.outcome);
    markSteps(null);
    cheatBob.checked = false;
    cheatAlice.checked = false;
    showChoice();
  }

  // ── set up ──
  for (const [index, fn] of deps.functions.entries()) {
    functionsBox.append(
      el(
        'label',
        { class: 'play__option' },
        el('input', { type: 'radio', name: 'play-function', value: fn.id, ...(index === 0 ? { checked: '' } : {}), 'data-play-choice': fn.id }),
        el('span', {}, el('strong', {}, fn.label), ` ${fn.summary}`),
      ),
    );
  }
  for (const radio of root.querySelectorAll<HTMLInputElement>('input[name="play-function"]')) radio.addEventListener('change', showChoice);
  serviceSelect.addEventListener('change', () => {
    inputBox.value = inputs.get(serviceSelect.value) ?? '{}';
  });
  publishButton.addEventListener('click', () => void publish());
  callButton.addEventListener('click', () => void call());
  resetButton.addEventListener('click', reset);
  tamperButton.addEventListener('click', tamper);
  copyButton.addEventListener('click', () => void copy());
  // Someone who focuses anything in Bob's panel, or points at the editor or the button, is about to compile.
  bob.addEventListener('focusin', preload);
  editorPanel.addEventListener('pointerenter', preload);
  publishButton.addEventListener('pointerenter', preload);

  const limits = deps.limits ?? PLAYGROUND_LIMITS;
  q('[data-play-limits]').textContent = `Bob’s limits: ${limits.max_runtime_ms / 1000} s per call, ${megabytes(limits.max_memory_bytes)} of memory (${limits.max_memory_bytes / WASM_PAGE_BYTES} pages), ${limits.max_input_bytes / 1024} KiB in and ${limits.max_output_bytes / 1024} KiB out. Browsers cannot count fuel, so the fuel limit a node enforces is not enforced here.`;
  q('[data-play-contract]').textContent = CONTRACT;
  reset();
  root.hidden = false;
}
