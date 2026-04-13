import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { copyToClipboard, initCopyButtons } from '../clipboard';

function makeButton(text: string): HTMLButtonElement {
  const btn = document.createElement('button');
  btn.textContent = text;
  return btn;
}

describe('copyToClipboard', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    Object.assign(navigator, {
      clipboard: { writeText: vi.fn() },
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('sets button text to "Copied" on success and returns true', async () => {
    (navigator.clipboard.writeText as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
    const btn = makeButton('Copy');

    const result = await copyToClipboard('hello', btn);

    expect(result).toBe(true);
    expect(btn.textContent).toBe('Copied');
  });

  it('sets button text to "Failed" when clipboard API throws and returns false', async () => {
    (navigator.clipboard.writeText as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('denied'));
    const btn = makeButton('Copy');

    const result = await copyToClipboard('hello', btn);

    expect(result).toBe(false);
    expect(btn.textContent).toBe('Failed');
  });

  it('resets button text to original after 1400ms on success', async () => {
    (navigator.clipboard.writeText as ReturnType<typeof vi.fn>).mockResolvedValue(undefined);
    const btn = makeButton('Copy');

    await copyToClipboard('hello', btn);
    expect(btn.textContent).toBe('Copied');

    vi.advanceTimersByTime(1399);
    expect(btn.textContent).toBe('Copied');

    vi.advanceTimersByTime(1);
    expect(btn.textContent).toBe('Copy');
  });

  it('resets button text to original after 1400ms on failure', async () => {
    (navigator.clipboard.writeText as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('denied'));
    const btn = makeButton('Copy');

    await copyToClipboard('hello', btn);
    expect(btn.textContent).toBe('Failed');

    vi.advanceTimersByTime(1400);
    expect(btn.textContent).toBe('Copy');
  });
});

describe('initCopyButtons', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    Object.assign(navigator, {
      clipboard: { writeText: vi.fn().mockResolvedValue(undefined) },
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('attaches click handlers to [data-copy] buttons', async () => {
    const container = document.createElement('div');
    const btn = document.createElement('button');
    btn.setAttribute('data-copy', 'some text');
    btn.textContent = 'Copy';
    container.appendChild(btn);

    initCopyButtons(container);
    btn.click();

    // Allow the async clipboard call to resolve
    await vi.advanceTimersByTimeAsync(0);

    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('some text');
    expect(btn.textContent).toBe('Copied');
  });
});
