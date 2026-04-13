/**
 * Shared copy-to-clipboard module.
 * Used by all pages that have copy buttons with `data-copy` attributes.
 */

export interface ClipboardButtonOptions {
  /** Text to display on success. Default: "Copied" */
  successText?: string;
  /** Text to display on failure. Default: "Failed" */
  failureText?: string;
  /** Duration in ms to show status text. Default: 1400 */
  resetDelay?: number;
}

const DEFAULTS: Required<ClipboardButtonOptions> = {
  successText: 'Copied',
  failureText: 'Failed',
  resetDelay: 1400,
};

/**
 * Copy text to clipboard and manage button state.
 * Returns true on success, false on failure.
 */
export async function copyToClipboard(
  text: string,
  button: HTMLButtonElement,
  options?: ClipboardButtonOptions,
): Promise<boolean> {
  const opts = { ...DEFAULTS, ...options };
  const original = button.textContent;

  let success = false;
  try {
    await navigator.clipboard.writeText(text);
    button.textContent = opts.successText;
    success = true;
  } catch {
    button.textContent = opts.failureText;
  }

  setTimeout(() => {
    button.textContent = original;
  }, opts.resetDelay);

  return success;
}

/**
 * Initialize all copy buttons matching `[data-copy]` selector.
 * Each button copies the value of its `data-copy` attribute.
 */
export function initCopyButtons(
  root?: HTMLElement,
  options?: ClipboardButtonOptions,
): void {
  const container = root ?? document;
  const buttons = container.querySelectorAll<HTMLButtonElement>('[data-copy]');

  buttons.forEach((button) => {
    button.addEventListener('click', () => {
      const text = button.getAttribute('data-copy') ?? '';
      copyToClipboard(text, button, options);
    });
  });
}
