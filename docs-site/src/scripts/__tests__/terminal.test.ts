import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { createTerminalAnimator } from '../demo/terminal';
import type { TerminalLine } from '../demo/steps';

// ── Helpers ──

function makeBody(): HTMLElement {
  const el = document.createElement('div');
  document.body.appendChild(el);
  return el;
}

const SIMPLE_LINES: TerminalLine[] = [
  { p: '~', c: 'echo hello' },
  { o: 'hello' },
];

describe('createTerminalAnimator', () => {
  let now: number;
  let rafCallbacks: Array<FrameRequestCallback>;
  let rafId: number;

  beforeEach(() => {
    now = 0;
    rafCallbacks = [];
    rafId = 0;

    vi.spyOn(performance, 'now').mockImplementation(() => now);
    vi.spyOn(window, 'requestAnimationFrame').mockImplementation((cb) => {
      rafCallbacks.push(cb);
      return ++rafId;
    });
  });

  afterEach(() => {
    vi.restoreAllMocks();
    document.body.innerHTML = '';
  });

  /** Flush all pending rAF callbacks, advancing time by `dt` ms each tick. */
  function flushFrames(ticks: number, dt = 50) {
    for (let i = 0; i < ticks; i++) {
      now += dt;
      const cbs = rafCallbacks.splice(0);
      for (const cb of cbs) cb(now);
    }
  }

  it('isTyping() returns false initially', () => {
    const body = makeBody();
    const animator = createTerminalAnimator(body);
    expect(animator.isTyping()).toBe(false);
  });

  it('isTyping() returns true during animation', async () => {
    const body = makeBody();
    const animator = createTerminalAnimator(body);

    // Start animation but don't let it finish
    const promise = animator.animate(SIMPLE_LINES);

    expect(animator.isTyping()).toBe(true);

    // Skip to finish so the promise resolves
    animator.skip();
    flushFrames(1);
    await promise;
  });

  it('skip() causes animation to complete immediately with all content rendered', async () => {
    const body = makeBody();
    const animator = createTerminalAnimator(body);

    const promise = animator.animate(SIMPLE_LINES);

    // Let the first rAF fire so the animation loop enters waitOrSkip
    flushFrames(1);

    animator.skip();
    // Flush frames so the skip is detected and remaining content is rendered
    flushFrames(5);

    await promise;

    // All lines should be rendered plus the trailing cursor line
    const divs = body.querySelectorAll('div');
    // Should contain the command line text and the output text
    const text = body.textContent ?? '';
    expect(text).toContain('echo hello');
    expect(text).toContain('hello');
  });

  it('isTyping() returns false after animation completes', async () => {
    const body = makeBody();
    const animator = createTerminalAnimator(body);

    const promise = animator.animate(SIMPLE_LINES);

    // Skip to finish quickly
    animator.skip();
    flushFrames(10);
    await promise;

    expect(animator.isTyping()).toBe(false);
  });

  it('destroy() stops animation and resets state', async () => {
    const body = makeBody();
    const animator = createTerminalAnimator(body);

    // Start animation
    const promise = animator.animate(SIMPLE_LINES);
    expect(animator.isTyping()).toBe(true);

    animator.destroy();

    // Flush remaining frames — animation should abort
    flushFrames(20);
    await promise;

    expect(animator.isTyping()).toBe(false);

    // After destroy, calling animate again should be a no-op
    const promise2 = animator.animate(SIMPLE_LINES);
    flushFrames(5);
    await promise2;
    expect(animator.isTyping()).toBe(false);
  });
});
