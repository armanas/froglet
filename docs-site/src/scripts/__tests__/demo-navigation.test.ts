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

  it('labels the next button as Skip while the terminal animation is typing', () => {
    initDemo();

    const nextBtn = document.getElementById('nextBtn') as HTMLButtonElement;
    expect(nextBtn.textContent).toBe('Skip');
  });
});
