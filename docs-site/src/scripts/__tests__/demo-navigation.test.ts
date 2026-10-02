import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { initDemo } from '../demo';

function installDemoDom(): void {
  document.body.innerHTML = `
    <div id="scene"><canvas></canvas></div>
    <div id="lesson-card"></div>
    <div id="annotation"></div>
    <div id="term-body"></div>
    <div class="pips"></div>
    <button id="prevBtn">Back</button>
    <button id="nextBtn">Continue</button>
  `;
}

describe('demo navigation state', () => {
  let now: number;
  let rafCallbacks: Array<FrameRequestCallback>;

  beforeEach(() => {
    now = 0;
    rafCallbacks = [];
    installDemoDom();

    vi.spyOn(performance, 'now').mockImplementation(() => now);
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => {
      rafCallbacks.push(cb);
      return rafCallbacks.length;
    });
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    document.body.innerHTML = '';
  });

  it('labels the next button as Skip while the terminal animation is typing', async () => {
    initDemo();

    const nextBtn = document.getElementById('nextBtn') as HTMLButtonElement;
    expect(nextBtn.textContent).toBe('Skip');

    // Finish the started animation before jsdom tears down its global rAF.
    nextBtn.click();
    for (let tick = 0; tick < 8; tick++) {
      for (const callback of rafCallbacks.splice(0)) callback(now);
      await Promise.resolve();
    }
    expect(nextBtn.textContent).toBe('Continue');
  });
});

describe('walkthrough keyboard shortcuts', () => {
  let now: number;
  let rafCallbacks: Array<FrameRequestCallback>;

  function installScopedDom(): void {
    document.body.innerHTML = `
      <input id="outside" />
      <section data-walkthrough tabindex="0">
        <div id="scene"><canvas></canvas></div>
        <div id="lesson-card"></div>
        <div id="annotation"></div>
        <div id="term-body"></div>
        <div class="pips"></div>
        <input id="inside-field" />
        <button id="prevBtn">Back</button>
        <button id="nextBtn">Continue</button>
      </section>
    `;
  }

  function press(target: EventTarget, init: KeyboardEventInit): void {
    target.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...init }));
  }

  async function flushAnimation(): Promise<void> {
    for (let tick = 0; tick < 8; tick++) {
      for (const callback of rafCallbacks.splice(0)) callback(now);
      await Promise.resolve();
    }
  }

  const stepLabel = (): string => document.querySelector('.step-n')?.textContent ?? '';

  beforeEach(() => {
    now = 0;
    rafCallbacks = [];
    installScopedDom();
    vi.spyOn(performance, 'now').mockImplementation(() => now);
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => {
      rafCallbacks.push(cb);
      return rafCallbacks.length;
    });
    vi.spyOn(HTMLCanvasElement.prototype, 'getContext').mockReturnValue(null);
  });

  afterEach(() => {
    vi.restoreAllMocks();
    document.body.innerHTML = '';
  });

  it('ignores arrow keys pressed anywhere outside the walkthrough', async () => {
    initDemo();
    const nextBtn = document.getElementById('nextBtn') as HTMLButtonElement;
    expect(nextBtn.textContent).toBe('Skip');

    press(document.body, { key: 'ArrowRight' });
    press(document.getElementById('outside')!, { key: 'ArrowRight' });
    press(document.getElementById('outside')!, { key: 'Escape' });
    await flushAnimation();

    // Typing was neither skipped nor advanced.
    expect(nextBtn.textContent).toBe('Skip');
    expect(stepLabel()).toBe('Step 1 / 11');
  });

  it('steps with the arrow keys once focus is inside the walkthrough', async () => {
    initDemo();
    const nextBtn = document.getElementById('nextBtn') as HTMLButtonElement;

    press(nextBtn, { key: 'ArrowRight' }); // first press skips the typing animation
    await flushAnimation();
    expect(nextBtn.textContent).toBe('Continue');

    press(nextBtn, { key: 'ArrowRight' }); // second press advances
    await flushAnimation();
    expect(stepLabel()).toBe('Step 2 / 11');
  });

  it('ignores modified keys and keys typed into form fields inside the walkthrough', async () => {
    initDemo();
    const nextBtn = document.getElementById('nextBtn') as HTMLButtonElement;
    const field = document.getElementById('inside-field')!;

    press(field, { key: 'ArrowRight' });
    press(nextBtn, { key: 'ArrowRight', metaKey: true });
    press(nextBtn, { key: 'ArrowRight', shiftKey: true });
    await flushAnimation();

    expect(nextBtn.textContent).toBe('Skip');
    expect(stepLabel()).toBe('Step 1 / 11');
  });
});
