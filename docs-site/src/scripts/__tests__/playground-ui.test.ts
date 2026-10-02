import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { CONTRACT, type Compiler } from '../playground/compiler';
import { FUNCTIONS } from '../playground/functions';
import { PLAYGROUND_LIMITS } from '../playground/protocol';
import { describeChain, describeCompile, describeEvent, describeOutcome, initPlayground, short, sizeLabel, STEPS, type PlaygroundDeps } from '../playground/ui';
import { fakeClock, kernel, newCompiler, nodeRunner, samples, verifier } from './playground-helpers';
import { src } from './route-helpers';

// The page's real markup, taken from the Astro file, driven with the real kernel, verifier, compiler and Workers. One compiler
// serves the file: loading the real one takes a second or two.
vi.setConfig({ testTimeout: 60_000 });
const compiler = newCompiler();
afterAll(() => compiler.dispose());
const page = readFileSync(resolve(src, 'pages/open-source.astro'), 'utf8');
const markup = page.slice(page.indexOf('<div class="play" data-playground hidden>'), page.indexOf('<!-- /playground -->'));

const $ = <T extends HTMLElement>(selector: string) => document.querySelector<T>(selector)!;
const text = (selector: string) => $(selector).textContent ?? '';
const click = (selector: string) => $<HTMLButtonElement>(selector).click();
const choose = (id: string) => {
  const radio = $<HTMLInputElement>(`[data-play-choice="${id}"]`);
  radio.checked = true;
  radio.dispatchEvent(new Event('change', { bubbles: true }));
};
const rows = () => Array.from(document.querySelectorAll<HTMLElement>('[data-play-evidence-rows] li'));
const entries = () => Array.from(document.querySelectorAll<HTMLElement>('[data-play-entry]'));
const routes = () => entries().map((entry) => entry.querySelector('.play__route')!.textContent);
const stepStates = () => STEPS.map((step) => $(`[data-step="${step}"]`).dataset.state);

let clock = fakeClock();
let copied: string[] = [];
let downloads: Array<{ name: string; bytes: Uint8Array }> = [];

function start(overrides: Partial<PlaygroundDeps> = {}) {
  document.body.innerHTML = markup;
  clock = fakeClock();
  initPlayground(document, {
    loadKernel: async () => kernel,
    loadVerifier: async () => verifier,
    runModule: nodeRunner(),
    functions: FUNCTIONS,
    compiler,
    download: (name, bytes) => void downloads.push({ name, bytes }),
    limits: { ...PLAYGROUND_LIMITS, max_runtime_ms: 10_000 },
    pollIntervalMs: 2,
    now: clock.now,
    waitForNextSecond: async () => clock.advance(1),
    ...overrides,
  });
}

const settled = async (selector: string, status: string) => vi.waitFor(() => expect($(selector).dataset.status).toBe(status), { timeout: 8000 });
const published = async () => vi.waitFor(() => expect($<HTMLButtonElement>('[data-play-call]').disabled).toBe(false), { timeout: 30_000 });
const called = async () => vi.waitFor(() => expect($('[data-play-outcome]').dataset.status).not.toBe('working'), { timeout: 8000 });

async function publishAndCall(sample = 'adder', input?: string) {
  choose(sample);
  click('[data-play-publish]');
  await published();
  if (input !== undefined) $<HTMLTextAreaElement>('[data-play-input]').value = input;
  click('[data-play-call]');
  await called();
}

beforeEach(() => {
  copied = [];
  downloads = [];
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: vi.fn(async (value: string) => void copied.push(value)) } });
  start();
});
afterEach(() => {
  document.body.innerHTML = '';
  vi.restoreAllMocks();
});

describe('the playground before anything happens', () => {
  it('appears once it can work, and offers the functions and an upload', () => {
    expect($('[data-playground]').hidden).toBe(false);
    const radios = Array.from(document.querySelectorAll<HTMLInputElement>('input[name="play-function"]'));
    expect(radios.map((radio) => radio.value)).toEqual(['adder', 'fibonacci', 'echo', 'never-ends', 'upload']);
    expect(radios.filter((radio) => radio.checked).map((radio) => radio.value)).toEqual(['adder']);
    expect(text('[data-play-blurb]')).toContain('two integers');
    expect(document.querySelector('[data-play-blurb] a')?.getAttribute('href')).toContain('examples/wasm-services/adder/src/lib.rs');
    expect(document.querySelector('[data-play-blurb] a')?.textContent).toBe('Compare with the Rust version');
  });

  it('cannot call anything until Bob has published, and says so', () => {
    expect($<HTMLButtonElement>('[data-play-call]').disabled).toBe(true);
    expect($<HTMLSelectElement>('[data-play-service]').disabled).toBe(true);
    expect($('[data-play-bob-status]').dataset.status).toBe('idle');
    expect(text('[data-play-alice-status]')).toBe('waiting for Bob');
    expect(text('[data-play-outcome]')).toBe('Nothing called yet.');
    expect(text('[data-play-wire]')).toBe('No messages yet.');
    expect($('[data-play-evidence]').hidden).toBe(true);
    expect(stepStates()).toEqual(['pending', 'pending', 'pending', 'pending', 'pending']);
  });

  it('states Bob\'s limits, and that fuel is not enforced', () => {
    expect(text('[data-play-limits]')).toBe('Bob’s limits: 10 s per call, 8 MiB of memory (128 pages), 128 KiB in and 128 KiB out. Browsers cannot count fuel, so the fuel limit a node enforces is not enforced here.');
  });

  it('shows the upload fields where the editor would be, only when the upload is chosen', () => {
    expect($('[data-play-upload]').hidden).toBe(true);
    expect($('[data-play-editor]').hidden).toBe(false);
    expect($('[data-play-mode]').dataset.playMode).toBe('function');
    choose('upload');
    expect($('[data-play-upload]').hidden).toBe(false);
    expect($('[data-play-editor]').hidden).toBe(true);
    expect($('[data-play-mode]').dataset.playMode).toBe('upload');
    expect($<HTMLInputElement>('[data-play-service-id]').value).toBe('demo.mine');
    expect(text('[data-play-blurb]')).toContain('froglet.wasm.run_json.v1');
    choose('fibonacci');
    expect($('[data-play-upload]').hidden).toBe(true);
    expect($('[data-play-editor]').hidden).toBe(false);
    expect($<HTMLInputElement>('[data-play-service-id]').value).toBe('demo.fibonacci');
    expect(text('[data-play-blurb]')).toContain('Fibonacci number');
  });
});

describe('Bob publishes', () => {
  it('signs a catalog for the function and hands Alice what she needs to call it', async () => {
    click('[data-play-publish]');
    await published();
    expect($('[data-play-bob-status]').dataset.status).toBe('ready');
    expect(text('[data-play-bob-status]')).toBe('published · demo.adder');
    const facts = text('[data-play-facts]');
    expect(facts).toContain('demo.adder');
    const compiledAdder = await compiler.compile(FUNCTIONS[0].code);
    if (!compiledAdder.ok) throw new Error('the adder does not compile');
    expect(facts).toContain(short(kernel.hashBytes(compiledAdder.module)));
    expect(facts).toMatch(/Built fromthe source, compiled in this page in \d+ ms/);
    expect(facts).toMatch(/at most 128/);
    expect($('[data-play-facts]').hidden).toBe(false);
    expect(Array.from($<HTMLSelectElement>('[data-play-service]').options).map((option) => option.value)).toEqual(['demo.adder']);
    expect($<HTMLTextAreaElement>('[data-play-input]').value).toBe('{"a": 6, "b": 7}');
    expect(text('[data-play-alice-key]')).toMatch(/^[0-9a-f]{16}…$/);
    // Alice learns what Bob offers over the wire, like everything else she knows.
    expect(routes()).toEqual(['GET /v1/provider/services']);
  });

  it('can publish a second function, which Alice can then pick', async () => {
    click('[data-play-publish]');
    await published();
    choose('fibonacci');
    click('[data-play-publish]');
    expect($<HTMLButtonElement>('[data-play-call]').disabled).toBe(true);
    // A real compiler result may arrive after waitFor's 1 s default. Use the
    // same bounded publish-completion check as the first function.
    await published();
    expect(text('[data-play-bob-status]')).toBe('published · demo.fibonacci');
    expect(Array.from($<HTMLSelectElement>('[data-play-service]').options).map((option) => option.value)).toEqual(['demo.adder', 'demo.fibonacci']);
    expect($<HTMLSelectElement>('[data-play-service]').value).toBe('demo.fibonacci');
    expect($<HTMLTextAreaElement>('[data-play-input]').value).toBe('{"n": 10}');
    $<HTMLSelectElement>('[data-play-service]').value = 'demo.adder';
    $<HTMLSelectElement>('[data-play-service]').dispatchEvent(new Event('change'));
    expect($<HTMLTextAreaElement>('[data-play-input]').value).toBe('{"a": 6, "b": 7}');
  });

  it('publishes a module you upload, and refuses one that is not a module, with the reason', async () => {
    choose('upload');
    const file = (bytes: BlobPart, name: string) => Object.defineProperty($<HTMLInputElement>('[data-play-file]'), 'files', { configurable: true, value: [new File([bytes], name)] });

    click('[data-play-publish]');
    await vi.waitFor(() => expect(text('[data-play-bob-log]')).toContain('choose a .wasm file first'));
    expect($('[data-play-bob-status]').dataset.status).toBe('refused');

    file('this is text, not a module', 'notes.wasm');
    click('[data-play-publish]');
    await vi.waitFor(() => expect(text('[data-play-bob-log]')).toContain('Bob refused to publish it: not a valid WebAssembly module'));

    $<HTMLInputElement>('[data-play-service-id]').value = 'Not Valid!';
    file((await samples.adder.bytes()) as BlobPart, 'adder.wasm');
    click('[data-play-publish]');
    await vi.waitFor(() => expect(text('[data-play-bob-log]')).toContain('lowercase letters'));

    $<HTMLInputElement>('[data-play-service-id]').value = 'demo.mine';
    click('[data-play-publish]');
    await vi.waitFor(() => expect(text('[data-play-bob-status]')).toBe('published · demo.mine'));
    expect($<HTMLTextAreaElement>('[data-play-input]').value).toBe('{}');
    expect(text('[data-play-facts]')).toContain('a module you uploaded');
    click('[data-play-call]');
    await vi.waitFor(() => expect($('[data-play-outcome]').dataset.status).toBe('verified'), { timeout: 8000 });
    // The function is the adder, which needs a and b, so with `{}` it answers with its own error, and that is still signed.
    expect(text('[data-play-outcome]')).toContain('send integers');
  });

  it('puts what came from a file name on the page as text, never as markup', async () => {
    choose('upload');
    const hostile = '<img src=x onerror=alert(1)>.wasm';
    Object.defineProperty($<HTMLInputElement>('[data-play-file]'), 'files', { configurable: true, value: [new File([(await samples.adder.bytes()) as BlobPart], hostile)] });
    click('[data-play-publish]');
    await published();
    expect(document.querySelector('[data-playground] img')).toBeNull();
    expect(text('[data-play-service]')).toContain(hostile);
  });

  it('recovers when the WebAssembly kernel cannot load, and works when tried again', async () => {
    let attempts = 0;
    start({
      loadKernel: async () => {
        attempts += 1;
        if (attempts === 1) throw new Error('the kernel could not be fetched');
        return kernel;
      },
    });
    click('[data-play-publish]');
    await vi.waitFor(() => expect(text('[data-play-bob-log]')).toContain('Could not publish it: the kernel could not be fetched'));
    expect($<HTMLButtonElement>('[data-play-publish]').disabled).toBe(false);
    click('[data-play-publish]');
    await published();
    expect(attempts).toBe(2);
  });
});

describe('Alice calls', () => {
  it('gets the answer, verifies the chain, and shows every step and message', async () => {
    await publishAndCall();
    expect($('[data-play-outcome]').dataset.status).toBe('verified');
    expect(text('[data-play-outcome]')).toContain('Verified');
    expect(text('[data-play-outcome] pre')).toBe(JSON.stringify({ sum: 13, product: 42 }, null, 2));
    expect(text('[data-play-alice-status]')).toBe('verified');
    expect(stepStates()).toEqual(['done', 'done', 'done', 'done', 'done']);

    expect(routes().slice(0, 5)).toEqual([
      'GET /v1/provider/descriptor',
      'GET /v1/provider/offers',
      'GET /v1/provider/services/demo.adder',
      'POST /v1/provider/quotes',
      'POST /v1/provider/deals',
    ]);
    expect(text('[data-play-wire-count]')).toMatch(/^\d+ messages?/);
    expect(document.querySelector('[data-play-entry] details pre')!.textContent).toContain('"artifact_type": "descriptor"');

    expect($('[data-play-evidence]').hidden).toBe(false);
    expect(rows().map((row) => row.querySelector('code')!.textContent)).toEqual(['descriptor', 'offer', 'quote', 'deal', 'receipt']);
    expect(rows().map((row) => row.dataset.status)).toEqual(['verified', 'verified', 'verified', 'verified', 'verified']);
    expect(rows().map((row) => row.querySelector('.play__signer')!.textContent)).toEqual(['signed by Bob', 'signed by Bob', 'signed by Bob', 'signed by Alice', 'signed by Bob']);
    expect($('[data-play-chain-badge]').dataset.status).toBe('verified');
    expect(text('[data-play-bob-log]')).toMatch(/^Bob ran demo\.adder in \d+ ms\.$/);
  });

  it('shows only the latest call on the wire, numbered from one, and only the latest thing Bob did', async () => {
    await publishAndCall();
    expect(text('[data-play-bob-log]')).toMatch(/^Bob ran demo\.adder/);
    $<HTMLInputElement>('[data-play-cheat-alice]').checked = true;
    click('[data-play-call]');
    await called();
    // Bob ran nothing this time, so the line about the last call is gone, and so are the last call's messages.
    expect(text('[data-play-bob-log]')).toBe('');
    expect(entries()[0].querySelector('.play__n')!.textContent).toBe('1');
    expect(routes()[0]).toBe('GET /v1/provider/descriptor');
    expect(routes().at(-1)).toBe('POST /v1/provider/deals');
    expect(text('[data-play-wire-count]')).toBe('5 messages');
  });

  it('calls the other function with its own input', async () => {
    await publishAndCall('fibonacci', '{"n": 50}');
    expect(text('[data-play-outcome] pre')).toContain('"fibonacci": 12586269025');
  });

  it('gives back the evidence as JSON that verifies on its own, and copies it', async () => {
    await publishAndCall();
    click('[data-play-copy]');
    await vi.waitFor(() => expect(copied).toHaveLength(1));
    const evidence = JSON.parse(copied[0]);
    expect(Object.keys(evidence)).toEqual(['artifacts']);
    expect(evidence.artifacts.map((artifact: { artifact_type: string }) => artifact.artifact_type)).toEqual(['descriptor', 'offer', 'quote', 'deal', 'receipt']);
    expect(verifier.validateChain(evidence.artifacts).valid).toBe(true);
    expect(text('[data-play-evidence-json]')).toBe(copied[0]);
    await vi.waitFor(() => expect(text('[data-play-evidence-out]')).toContain('Copied'));
  });

  it('says so, without a success colour, when the browser blocks copying, and the JSON is still there to select', async () => {
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: vi.fn().mockRejectedValue(new Error('blocked')) } });
    await publishAndCall();
    click('[data-play-copy]');
    await vi.waitFor(() => expect(text('[data-play-evidence-out]')).toContain('The browser blocked copying'));
    expect($('[data-play-evidence-out]').dataset.status).toBeUndefined();
    expect(JSON.parse(text('[data-play-evidence-json]')).artifacts).toHaveLength(5);
  });

  it('breaks the evidence when the receipt is changed, and says which record no longer matches', async () => {
    await publishAndCall();
    click('[data-play-tamper]');
    const out = text('[data-play-evidence-out]');
    expect(out).toContain('receipt.payload.deal_hash was set to 64 zeros');
    expect(out).toContain('the receipt no longer matches what was signed');
    expect($('[data-play-evidence-out]').dataset.status).toBe('invalid');
    // What is shown and copied is still the genuine evidence.
    click('[data-play-copy]');
    await vi.waitFor(() => expect(copied).toHaveLength(1));
    expect(verifier.validateChain(JSON.parse(copied[0]).artifacts).valid).toBe(true);
  });

  it('can call again and again, and folds repeated polls of a slow function into one line', async () => {
    start({ limits: { ...PLAYGROUND_LIMITS, max_runtime_ms: 400 }, pollIntervalMs: 2 });
    await publishAndCall('never-ends', '{}');
    expect($('[data-play-outcome]').dataset.status).toBe('failed');
    const lines = entries().length;
    const messages = Number(text('[data-play-wire-count]').match(/^(\d+) messages/)![1]);
    expect(messages).toBeGreaterThan(lines);
    expect(text('[data-play-wire-count]')).toContain(`${lines} lines`);
    expect(document.querySelector('.play__times:not([hidden])')?.textContent).toMatch(/^× \d+$/);
  });
});

describe('when Bob cannot answer', () => {
  it('stops a function that never ends, and shows the signed receipt that says so', async () => {
    start({ limits: { ...PLAYGROUND_LIMITS, max_runtime_ms: 300 } });
    await publishAndCall('never-ends', '{}');
    expect($('[data-play-outcome]').dataset.status).toBe('failed');
    expect(text('[data-play-outcome]')).toContain('Verified, but the function failed');
    expect(text('[data-play-outcome]')).toContain('execution_timed_out');
    expect(text('[data-play-alice-status]')).toBe('function failed');
    expect(text('[data-play-bob-log]')).toMatch(/^Bob stopped demo\.never-ends at his time limit \(\d+ ms\)\.$/);
    expect($('[data-play-evidence]').hidden).toBe(false);
    expect($('[data-play-chain-badge]').dataset.status).toBe('verified');
    expect(stepStates()).toEqual(['done', 'done', 'done', 'done', 'done']);
  });

  it('is ready to be called again straight after', async () => {
    start({ limits: { ...PLAYGROUND_LIMITS, max_runtime_ms: 300 } });
    await publishAndCall('never-ends', '{}');
    expect($<HTMLButtonElement>('[data-play-call]').disabled).toBe(false);
    expect($<HTMLButtonElement>('[data-play-publish]').disabled).toBe(false);
  });
});

describe('the two ways to cheat', () => {
  it('has Alice catch a receipt over the wrong result, though every signature is genuine', async () => {
    choose('adder');
    click('[data-play-publish]');
    await published();
    $<HTMLInputElement>('[data-play-cheat-bob]').checked = true;
    click('[data-play-call]');
    await called();
    expect($('[data-play-outcome]').dataset.status).toBe('refused');
    expect(text('[data-play-outcome]')).toContain('Alice refused');
    expect(text('[data-play-outcome]')).toContain('result_hash');
    expect(text('[data-play-outcome]')).toContain('the records still verify');
    expect(document.querySelector('[data-play-outcome] pre')).toBeNull();
    expect(stepStates()).toEqual(['done', 'done', 'done', 'failed', 'pending']);
    // The five records are all there and verify, which is the lesson: the check that caught it is Alice's own.
    expect($('[data-play-evidence]').hidden).toBe(false);
    expect($('[data-play-chain-badge]').dataset.status).toBe('verified');
    // And Bob is honest again when the box is cleared.
    $<HTMLInputElement>('[data-play-cheat-bob]').checked = false;
    click('[data-play-call]');
    await vi.waitFor(() => expect($('[data-play-outcome]').dataset.status).toBe('verified'), { timeout: 8000 });
  });

  it('has Bob refuse a deal Alice changed after signing, and run nothing', async () => {
    choose('adder');
    click('[data-play-publish]');
    await published();
    $<HTMLInputElement>('[data-play-cheat-alice]').checked = true;
    click('[data-play-call]');
    await called();
    expect($('[data-play-outcome]').dataset.status).toBe('refused');
    expect(text('[data-play-outcome]')).toContain('Bob refused');
    expect(text('[data-play-outcome]')).toContain('the deal does not verify');
    expect(text('[data-play-outcome]')).toContain('he ran nothing');
    expect(stepStates()).toEqual(['done', 'done', 'failed', 'pending', 'pending']);
    expect($('[data-play-evidence]').hidden).toBe(true);
    expect(text('[data-play-bob-log]')).toBe('');
    expect(text('[data-play-wire]')).toContain('POST /v1/provider/deals');
    expect(document.querySelector('.play__code[data-ok="false"]')!.textContent).toBe('400');
  });

  it('does not send input a browser would round', async () => {
    choose('adder');
    click('[data-play-publish]');
    await published();
    $<HTMLTextAreaElement>('[data-play-input]').value = '{"a": 9007199254740993, "b": 1}';
    click('[data-play-call]');
    await called();
    expect(text('[data-play-outcome]')).toContain('Not sent');
    expect(text('[data-play-outcome]')).toContain('9007199254740993');
    expect(text('[data-play-outcome]')).toContain('Nothing was sent.');
    // The wire shows the latest call, and this call sent nothing.
    expect(entries()).toHaveLength(0);
    expect(text('[data-play-wire]')).toBe('No messages yet.');
  });
});

describe('starting over', () => {
  it('forgets both sides: a new key, no function, no wire, no evidence', async () => {
    await publishAndCall();
    const firstKey = text('[data-play-alice-key]');
    $<HTMLInputElement>('[data-play-cheat-bob]').checked = true;
    click('[data-play-reset]');
    expect($('[data-play-bob-status]').dataset.status).toBe('idle');
    expect($('[data-play-facts]').hidden).toBe(true);
    expect($<HTMLButtonElement>('[data-play-call]').disabled).toBe(true);
    expect($('[data-play-evidence]').hidden).toBe(true);
    expect(text('[data-play-wire]')).toBe('No messages yet.');
    expect(text('[data-play-wire-count]')).toBe('0 messages');
    expect($<HTMLTextAreaElement>('[data-play-input]').value).toBe('');
    expect($<HTMLInputElement>('[data-play-cheat-bob]').checked).toBe(false);
    expect(stepStates()).toEqual(['pending', 'pending', 'pending', 'pending', 'pending']);
    click('[data-play-publish]');
    await published();
    expect(text('[data-play-alice-key]')).not.toBe(firstKey);
    click('[data-play-call]');
    await vi.waitFor(() => expect($('[data-play-outcome]').dataset.status).toBe('verified'), { timeout: 8000 });
  });

  it('cannot be interrupted, or interrupt, while a call is running', async () => {
    start({ limits: { ...PLAYGROUND_LIMITS, max_runtime_ms: 300 } });
    choose('never-ends');
    click('[data-play-publish]');
    await published();
    click('[data-play-call]');
    expect($<HTMLButtonElement>('[data-play-call]').disabled).toBe(true);
    expect($<HTMLButtonElement>('[data-play-publish]').disabled).toBe(true);
    expect($<HTMLButtonElement>('[data-play-reset]').disabled).toBe(true);
    expect($('[data-play-outcome]').getAttribute('aria-busy')).toBe('true');
    await called();
    expect($('[data-play-outcome]').hasAttribute('aria-busy')).toBe(false);
    expect($<HTMLButtonElement>('[data-play-reset]').disabled).toBe(false);
  });
});


// ── Bob writes the function ──
const source = () => $<HTMLTextAreaElement>('[data-play-source]');
const typeSource = (value: string) => {
  source().value = value;
  source().dispatchEvent(new Event('input', { bubbles: true }));
};
const gutterNumbers = () => Array.from(document.querySelectorAll('[data-play-gutter] span')).map((item) => item.textContent);
const markedLines = () => Array.from(document.querySelectorAll('[data-play-gutter] span')).flatMap((item, index) => (item.classList.contains('is-marked') ? [index + 1] : []));
const problemsShown = () => Array.from(document.querySelectorAll('[data-play-problems] li')).map((row) => row.textContent);
const compiled = async (code: string) => {
  const result = await compiler.compile(code);
  if (!result.ok) throw new Error(`does not compile: ${JSON.stringify(result).slice(0, 200)}`);
  return result.module;
};
const facts = () => text('[data-play-facts]');
const adder = FUNCTIONS.find((fn) => fn.id === 'adder')!;
const withProduct = (expression: string) => adder.code.replace('`{"sum":${sum},"product":${a * b}}`', expression);

describe('Bob writes the function', () => {
  it('shows the source of the chosen function in a numbered editor, and the contract that will follow it', () => {
    expect(source().value).toBe(adder.code);
    expect(gutterNumbers()).toHaveLength(adder.code.split('\n').length);
    expect(gutterNumbers().slice(0, 3)).toEqual(['1', '2', '3']);
    expect($('[data-play-reset-source]').hidden).toBe(true);
    expect(text('[data-play-contract]')).toBe(CONTRACT);
    choose('fibonacci');
    expect(source().value).toBe(FUNCTIONS.find((fn) => fn.id === 'fibonacci')!.code);
    choose('never-ends');
    expect(source().value).toContain('while (true)');
    expect(text('[data-play-editor]')).not.toContain('undefined');
  });

  it('keeps what the person has written when they go to another function and come back, and can put the original back', () => {
    typeSource(`${adder.code}\n// my change`);
    expect($('[data-play-reset-source]').hidden).toBe(false);
    choose('fibonacci');
    expect($('[data-play-reset-source]').hidden).toBe(true);
    expect(source().value).not.toContain('my change');
    choose('adder');
    expect(source().value.endsWith('// my change')).toBe(true);
    expect($('[data-play-reset-source]').hidden).toBe(false);
    click('[data-play-reset-source]');
    expect(source().value).toBe(adder.code);
    expect($('[data-play-reset-source]').hidden).toBe(true);
    expect(gutterNumbers()).toHaveLength(adder.code.split('\n').length);
  });

  it('compiles what was written, publishes it, and the change shows in the answer and in the hash', async () => {
    click('[data-play-publish]');
    await published();
    const original = facts();
    click('[data-play-call]');
    await called();
    expect(text('[data-play-outcome] pre')).toContain('"sum": 13');

    typeSource(withProduct('`{"sum":${a * a + b},"product":${a * b}}`'));
    click('[data-play-publish]');
    await vi.waitFor(() => expect(facts()).not.toBe(original), { timeout: 30_000 });
    await published();
    expect(facts()).toMatch(/Built fromyour edited source, compiled in this page in \d+ ms/);
    expect(facts()).toContain(short(kernel.hashBytes(await compiled(source().value))));
    expect(text('[data-play-service]')).toContain('Adds and multiplies two integers (edited)');
    click('[data-play-call]');
    await called();
    expect($('[data-play-outcome]').dataset.status).toBe('verified');
    expect(text('[data-play-outcome] pre')).toContain('"sum": 43');
    // The receipt names the module it ran, and it is the one that was compiled.
    const receipt = JSON.parse(text('[data-play-evidence-json]')).artifacts[4].payload;
    expect(receipt.executor.module_hash).toBe(kernel.hashBytes(await compiled(source().value)));
  });

  it('says what the compiler said, in words, when the compile succeeds', async () => {
    click('[data-play-publish]');
    await published();
    expect($('[data-play-problems]').dataset.status).toBe('ok');
    expect(text('[data-play-problems]')).toMatch(/^Compiled \d\.\d KB in \d+ ms\. It imports nothing, as a Froglet function must\.$/);
    expect(problemsShown()).toEqual([]);
  });

  it('shows a mistake where it is, moves the cursor there, marks the line, and publishes nothing', async () => {
    typeSource(adder.code.replace('${a * b}', '${a * bee}'));
    click('[data-play-publish]');
    await vi.waitFor(() => expect($('[data-play-problems]').dataset.status).toBe('error'), { timeout: 30_000 });
    await vi.waitFor(() => expect($<HTMLButtonElement>('[data-play-publish]').disabled).toBe(false));
    const line = source().value.split('\n').findIndex((row) => row.includes('bee')) + 1;
    const column = source().value.split('\n')[line - 1].indexOf('bee') + 1;
    expect(text('[data-play-problems]')).toContain('1 problem in the code.');
    expect(problemsShown()).toEqual([`Line ${line}, column ${column}Cannot find name 'bee'.`]);
    expect(markedLines()).toEqual([line]);
    expect(source().value.slice(source().selectionStart, source().selectionEnd)).toBe('bee');
    expect(document.activeElement).toBe(source());
    // Nothing was published: Bob is as he was, and Alice has nothing to call.
    expect($('[data-play-bob-status]').dataset.status).toBe('idle');
    expect(text('[data-play-bob-status]')).toBe('not published');
    expect($<HTMLButtonElement>('[data-play-call]').disabled).toBe(true);
    expect($('[data-play-facts]').hidden).toBe(true);
    expect(text('[data-play-wire]')).toBe('No messages yet.');
  });

  it('takes the cursor to a problem again when its place is pressed', async () => {
    typeSource(adder.code.replace('${a * b}', '${a * bee}'));
    click('[data-play-publish]');
    await vi.waitFor(() => expect($('[data-play-problems]').dataset.status).toBe('error'), { timeout: 30_000 });
    source().setSelectionRange(0, 0);
    click('.play__jump');
    expect(source().value.slice(source().selectionStart, source().selectionEnd)).toBe('bee');
  });

  it('shows what the compiler quotes from the source as text, never as markup', async () => {
    // The compiler repeats a path it could not find, and that is the person's own text.
    typeSource('import { a } from "<img src=x onerror=alert(1)>";\nfunction respond(request: string): string {\n  return request;\n}');
    click('[data-play-publish]');
    await vi.waitFor(() => expect($('[data-play-problems]').dataset.status).toBe('error'), { timeout: 30_000 });
    const problems = $('[data-play-problems]');
    expect(problems.textContent).toContain('<img src=x onerror=alert(1)>');
    expect(problems.querySelector('img')).toBeNull();
    expect(problems.querySelector('[onerror]')).toBeNull();
    expect(problems.querySelectorAll('*').length).toBeLessThan(12);
  });

  it('forgets what the compiler said as soon as the code changes, since it was about the old code', async () => {
    typeSource(adder.code.replace('${a * b}', '${a * bee}'));
    click('[data-play-publish]');
    await vi.waitFor(() => expect($('[data-play-problems]').dataset.status).toBe('error'), { timeout: 30_000 });
    typeSource(adder.code);
    expect(text('[data-play-problems]')).toBe('');
    expect($('[data-play-problems]').hasAttribute('data-status')).toBe(false);
    expect(markedLines()).toEqual([]);
  });

  it('explains a call the host would have to answer, since a function may not import anything', async () => {
    typeSource(adder.code.replace('const sum = a + b;', 'const sum = a + b + <i64>Math.random();'));
    click('[data-play-publish]');
    await vi.waitFor(() => expect($('[data-play-problems]').dataset.status).toBe('error'), { timeout: 30_000 });
    expect(text('[data-play-problems]')).toContain('Math.random() (env.seed)');
    expect(text('[data-play-problems]')).toContain('may not import anything');
    expect($('[data-play-bob-status]').dataset.status).toBe('idle');
    expect($<HTMLButtonElement>('[data-play-call]').disabled).toBe(true);
  });

  it('says the trouble is in the contract when a bracket is left open, and where the code redefines a name', async () => {
    typeSource('function respond(request: string): string {\n  return request;');
    click('[data-play-publish]');
    await vi.waitFor(() => expect($('[data-play-problems]').dataset.status).toBe('error'), { timeout: 30_000 });
    expect(text('[data-play-problems]')).toContain('The compiler reports this in the contract that follows your code.');
    expect(document.querySelector('.play__jump')).toBeNull();
  });

  it('gives a function that answers with something that is not JSON a signed failure, and says why', async () => {
    choose('echo');
    typeSource(FUNCTIONS.find((fn) => fn.id === 'echo')!.code.replace('return request;', "return 'hello';"));
    click('[data-play-publish]');
    await published();
    click('[data-play-call]');
    await called();
    expect($('[data-play-outcome]').dataset.status).toBe('failed');
    expect(text('[data-play-outcome]')).toContain('execution_failed: expected value at line 1 column 1');
    expect($('[data-play-chain-badge]').dataset.status).toBe('verified');
  });

  it('gives a function that traps a signed failure, and one that stops asserting a way to see it', async () => {
    choose('echo');
    typeSource(FUNCTIONS.find((fn) => fn.id === 'echo')!.code.replace('return request;', 'assert(request.length > 1000);\n  return request;'));
    click('[data-play-publish]');
    await published();
    click('[data-play-call]');
    await called();
    expect($('[data-play-outcome]').dataset.status).toBe('failed');
    expect(text('[data-play-outcome]')).toMatch(/unreachable/i);
  });

  it('runs a function that is written from nothing, with whatever service id the person gives it', async () => {
    choose('echo');
    typeSource("function respond(request: string): string {\n  return '{\"length\":' + request.length.toString() + '}';\n}");
    $<HTMLInputElement>('[data-play-service-id]').value = 'me.length';
    click('[data-play-publish]');
    await published();
    expect(text('[data-play-bob-status]')).toBe('published · me.length');
    $<HTMLTextAreaElement>('[data-play-input]').value = '{ "b": 7,  "a": 6 }';
    click('[data-play-call]');
    await called();
    // The function got the canonical text, which is shorter than the spaced text that was typed.
    expect(text('[data-play-outcome] pre')).toBe(JSON.stringify({ length: '{"a":6,"b":7}'.length }, null, 2));
  });

  it('shows the compiler loading, then compiling, then Bob publishing', async () => {
    const phases: string[] = [];
    let open!: () => void;
    const gate = new Promise<void>((resolve) => (open = resolve));
    const slow: Compiler = {
      preload: () => {},
      dispose: () => {},
      compile: async (code, onPhase) => {
        onPhase?.('loading');
        phases.push(text('[data-play-bob-status]'));
        await gate;
        onPhase?.('compiling');
        phases.push(text('[data-play-bob-status]'));
        return compiler.compile(code);
      },
    };
    start({ compiler: slow });
    click('[data-play-publish]');
    await vi.waitFor(() => expect(phases).toEqual(['loading the compiler']));
    open();
    await published();
    expect(phases).toEqual(['loading the compiler', 'compiling']);
  });

  it('starts loading the compiler when the person shows they mean to use it, and not before', () => {
    const preload = vi.fn();
    start({ compiler: { preload, compile: async () => ({ ok: false, kind: 'unavailable', message: 'never', ms: 0 }), dispose: () => {} } });
    expect(preload).not.toHaveBeenCalled();
    choose('upload');
    $('[data-play-bob]').dispatchEvent(new FocusEvent('focusin', { bubbles: true }));
    expect(preload).not.toHaveBeenCalled();
    choose('adder');
    $('[data-play-editor]').dispatchEvent(new Event('pointerenter'));
    expect(preload).toHaveBeenCalledTimes(1);
    $('[data-play-bob]').dispatchEvent(new FocusEvent('focusin', { bubbles: true }));
    $('[data-play-publish]').dispatchEvent(new Event('pointerenter'));
    expect(preload).toHaveBeenCalledTimes(1);
  });

  it('reports a compiler that will not load, keeps Bob as he was, and works when tried again', async () => {
    let attempts = 0;
    const flaky: Compiler = {
      preload: () => {},
      dispose: () => {},
      compile: async (code) => {
        attempts += 1;
        return attempts === 1 ? { ok: false, kind: 'unavailable', message: 'The compiler could not load: Failed to fetch', ms: 0 } : compiler.compile(code);
      },
    };
    start({ compiler: flaky });
    click('[data-play-publish]');
    await vi.waitFor(() => expect(text('[data-play-problems]')).toContain('The compiler could not load: Failed to fetch'));
    await vi.waitFor(() => expect($<HTMLButtonElement>('[data-play-publish]').disabled).toBe(false));
    expect($('[data-play-bob-status]').dataset.status).toBe('idle');
    click('[data-play-publish]');
    await published();
    expect(attempts).toBe(2);
    expect(text('[data-play-bob-status]')).toBe('published · demo.adder');
  });

  it('offers the module it compiled for download, under the service id, and only while the code still matches it', async () => {
    expect($<HTMLButtonElement>('[data-play-download]').disabled).toBe(true);
    click('[data-play-publish]');
    await published();
    expect($<HTMLButtonElement>('[data-play-download]').disabled).toBe(false);
    click('[data-play-download]');
    expect(downloads).toHaveLength(1);
    expect(downloads[0].name).toBe('demo.adder.wasm');
    expect(kernel.hashBytes(downloads[0].bytes)).toBe(kernel.hashBytes(await compiled(adder.code)));
    // The receipt's module hash is the hash of exactly what was downloaded.
    click('[data-play-call]');
    await called();
    expect(JSON.parse(text('[data-play-evidence-json]')).artifacts[4].payload.executor.module_hash).toBe(kernel.hashBytes(downloads[0].bytes));
    typeSource(`${adder.code}\n// changed after compiling`);
    expect($<HTMLButtonElement>('[data-play-download]').disabled).toBe(true);
    typeSource(adder.code);
    expect($<HTMLButtonElement>('[data-play-download]').disabled).toBe(false);
  });

  it('names a download after a service id only when it is a valid one', async () => {
    click('[data-play-publish]');
    await published();
    $<HTMLInputElement>('[data-play-service-id]').value = 'Not Valid/../x';
    click('[data-play-download]');
    expect(downloads.at(-1)!.name).toBe('function.wasm');
  });

  it('refuses source that is too long without asking the compiler', async () => {
    const compile = vi.fn();
    start({ compiler: { preload: () => {}, compile: (code) => (compile(code), compiler.compile(code)), dispose: () => {} } });
    typeSource(`${adder.code}\n// ${'x'.repeat(70_000)}`);
    click('[data-play-publish]');
    await vi.waitFor(() => expect(text('[data-play-problems]')).toMatch(/The source is \d+ characters\. The most the editor takes is 65536\./));
    expect($('[data-play-bob-status]').dataset.status).toBe('idle');
  });

  it('starts over with the original source and no memory of the edits', async () => {
    typeSource(`${adder.code}\n// mine`);
    click('[data-play-publish]');
    await published();
    click('[data-play-reset]');
    expect(source().value).toBe(adder.code);
    expect($('[data-play-reset-source]').hidden).toBe(true);
    expect(text('[data-play-problems]')).toBe('');
    expect($<HTMLButtonElement>('[data-play-download]').disabled).toBe(true);
    choose('fibonacci');
    choose('adder');
    expect(source().value).toBe(adder.code);
  });
});

describe('what the page says', () => {
  const ok = {
    ok: true as const,
    status: 'succeeded' as const,
    result: { sum: 13 },
    artifacts: {} as never,
    chain: { valid: true, chain_evaluated: true, artifacts: [] },
  };

  it('describes each kind of outcome in its own words', () => {
    expect(describeOutcome(ok)).toMatchObject({ status: 'verified', title: 'Verified', result: '{\n  "sum": 13\n}' });
    expect(describeOutcome({ ...ok, status: 'failed', failure: { code: 'execution_timed_out', message: 'too slow' } })).toMatchObject({ status: 'failed', detail: "Bob's signed receipt says execution_timed_out: too slow" });
    expect(describeOutcome({ ok: false, stage: 'quote', refusedBy: 'alice', reason: 'The quote does not verify.', artifacts: {} })).toEqual({ status: 'refused', title: 'Alice refused', detail: 'The quote does not verify.' });
    expect(describeOutcome({ ok: false, stage: 'deal', refusedBy: 'bob', reason: 'no capacity', artifacts: {} })).toMatchObject({ title: 'Bob refused' });
    expect(describeOutcome({ ok: false, stage: 'input', refusedBy: 'alice', reason: 'bad', artifacts: {} })).toMatchObject({ title: 'Not sent', hint: 'Nothing was sent.' });
  });

  it('describes a chain by what broke', () => {
    expect(describeChain({ valid: true, chain_evaluated: true, artifacts: [] }).valid).toBe(true);
    expect(describeChain({ valid: false, chain_evaluated: false, artifacts: [{ artifact_type: 'receipt', status: 'invalid' } as never] }).text).toBe('Verification failed: the receipt no longer matches what was signed.');
    expect(describeChain({ valid: false, chain_evaluated: false, error: 'not JSON', artifacts: [] }).text).toBe('not JSON');
    expect(describeChain({ valid: false, chain_evaluated: false, artifacts: [] }).text).toContain('do not form a complete chain');
  });

  it('describes a compile by how it went, and each problem by where it is', () => {
    const module = new Uint8Array(3494);
    expect(describeCompile({ ok: true, module, warnings: [], ms: 120 })).toEqual({ status: 'ok', summary: 'Compiled 3.4 KB in 120 ms. It imports nothing, as a Froglet function must.', items: [] });
    const warning = { severity: 'warning' as const, code: 1, message: 'careful', line: 2, column: 3, endLine: 2, endColumn: 5, inContract: false };
    expect(describeCompile({ ok: true, module, warnings: [warning], ms: 1 })).toMatchObject({ status: 'warning', items: [{ severity: 'warning', message: 'careful', line: 2 }] });
    const error = { ...warning, severity: 'error' as const, message: 'broken' };
    expect(describeCompile({ ok: false, kind: 'errors', diagnostics: [error], ms: 1 })).toMatchObject({ status: 'error', summary: '1 problem in the code.' });
    expect(describeCompile({ ok: false, kind: 'errors', diagnostics: [error, { ...error, inContract: true, line: null, column: null }], ms: 1 })).toMatchObject({ summary: '2 problems in the code.', items: [{}, { note: expect.stringContaining('in the contract that follows your code') }] });
    expect(describeCompile({ ok: false, kind: 'timeout', message: 'Compiling took longer than 15 seconds, so it was stopped.', ms: 0 })).toEqual({ status: 'error', summary: 'Compiling took longer than 15 seconds, so it was stopped.', items: [] });
  });

  it('formats sizes, hashes, and Bob\'s activity', () => {
    expect(sizeLabel(512)).toBe('512 bytes');
    expect(sizeLabel(2048)).toBe('2.0 KB');
    expect(sizeLabel(28_036)).toBe('27 KB');
    expect(short('abcdef0123456789abcdef')).toBe('abcdef012345…');
    expect(short('abc')).toBe('abc');
    const event = (outcome: never) => ({ type: 'executed' as const, dealId: 'x', serviceId: 'demo.x', outcome, ms: 12.4 });
    expect(describeEvent(event({ ok: true } as never))).toBe('Bob ran demo.x in 12 ms.');
    expect(describeEvent(event({ ok: false, timedOut: true, error: 'slow' } as never))).toBe('Bob stopped demo.x at his time limit (12 ms).');
    expect(describeEvent(event({ ok: false, error: 'unreachable' } as never))).toBe('demo.x failed after 12 ms: unreachable');
  });

  it('refuses to start against markup that is missing a part', () => {
    document.body.innerHTML = '<div data-playground hidden></div>';
    expect(() => initPlayground(document, { loadKernel: async () => kernel, loadVerifier: async () => verifier, runModule: nodeRunner(), functions: [], compiler })).toThrow(/no \[data-play-bob\]/);
  });

  it('does nothing on a page without the playground', () => {
    document.body.innerHTML = '<p>nothing here</p>';
    expect(() => initPlayground(document, { loadKernel: async () => kernel, loadVerifier: async () => verifier, runModule: nodeRunner(), functions: [], compiler })).not.toThrow();
  });
});
