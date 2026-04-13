// ═══════ Terminal typing animation ═══════
// Extracted from demo.astro inline script (Requirement 1.3)

import type { TerminalLine } from './steps';

export interface TerminalAnimator {
  animate(lines: TerminalLine[]): Promise<void>;
  skip(): void;
  isTyping(): boolean;
  destroy(): void;
}

// ── Timing constants ──
const CHAR_DELAY_BASE = 10;
const CHAR_DELAY_JITTER = 14;
const PROMPT_PAUSE = 140;
const POST_COMMAND_PAUSE = 70;
const OUTPUT_LINE_PAUSE = 45;

// ── DOM helpers ──

function promptMarkup(p: string): string {
  return p === '~'
    ? `<span class="tp">${p}</span> <span class="tv">&#10095;</span> `
    : `<span class="tv">${p}</span> `;
}

function appendLine(body: HTMLElement, line: TerminalLine): void {
  const d = document.createElement('div');
  d.innerHTML =
    line.p !== undefined
      ? `${promptMarkup(line.p)}<span class="tc">${line.c}</span>`
      : (line.o ?? '');
  body.appendChild(d);
}

function appendCursor(body: HTMLElement): void {
  const c = document.createElement('div');
  c.className = 'cursor-line';
  c.innerHTML = `<span class="tp">~</span> <span class="tv">&#10095;</span> <span class="cursor"></span>`;
  body.appendChild(c);
  body.scrollTop = body.scrollHeight;
}

function appendRest(body: HTMLElement, lines: TerminalLine[], i: number): void {
  for (; i < lines.length; i++) appendLine(body, lines[i]);
  body.scrollTop = body.scrollHeight;
}


// ── Factory ──

export function createTerminalAnimator(body: HTMLElement): TerminalAnimator {
  let typing = false;
  let skipRequested = false;
  let runId = 0;
  let destroyed = false;

  /** Wait for `ms` milliseconds, returning early on abort or skip. */
  async function waitOrSkip(
    ms: number,
    rid: number,
  ): Promise<'ok' | 'skip' | 'abort'> {
    const end = performance.now() + ms;
    while (performance.now() < end) {
      if (rid !== runId) return 'abort';
      if (skipRequested) return 'skip';
      await new Promise<void>((r) => requestAnimationFrame(r));
    }
    return rid !== runId ? 'abort' : skipRequested ? 'skip' : 'ok';
  }

  async function animate(lines: TerminalLine[]): Promise<void> {
    if (destroyed) return;

    const rid = ++runId;
    typing = true;
    skipRequested = false;
    body.innerHTML = '';

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];

      if (line.p !== undefined) {
        // Command line — type character by character
        let s = await waitOrSkip(PROMPT_PAUSE, rid);
        if (s === 'abort') return;
        if (s === 'skip') { appendRest(body, lines, i); break; }

        const div = document.createElement('div');
        const pr = promptMarkup(line.p);
        div.innerHTML = pr;
        body.appendChild(div);
        body.scrollTop = body.scrollHeight;

        let skipped = false;
        const cmd = line.c ?? '';
        for (let j = 0; j < cmd.length; j++) {
          s = await waitOrSkip(CHAR_DELAY_BASE + Math.random() * CHAR_DELAY_JITTER, rid);
          if (s === 'abort') return;
          if (s === 'skip') { skipped = true; break; }
          div.innerHTML = `${pr}<span class="tc">${cmd.slice(0, j + 1)}</span><span class="cursor"></span>`;
          body.scrollTop = body.scrollHeight;
        }

        div.innerHTML = `${pr}<span class="tc">${cmd}</span>`;
        body.scrollTop = body.scrollHeight;

        if (skipped) { appendRest(body, lines, i + 1); break; }

        s = await waitOrSkip(POST_COMMAND_PAUSE, rid);
        if (s === 'abort') return;
        if (s === 'skip') { appendRest(body, lines, i + 1); break; }
      } else {
        // Output line — show after a short pause
        const s = await waitOrSkip(OUTPUT_LINE_PAUSE, rid);
        if (s === 'abort') return;
        if (s === 'skip') { appendRest(body, lines, i); break; }
        appendLine(body, line);
        body.scrollTop = body.scrollHeight;
      }
    }

    if (rid !== runId) return;
    appendCursor(body);
    typing = false;
    skipRequested = false;
  }

  function skip(): void {
    skipRequested = true;
  }

  function isTyping(): boolean {
    return typing;
  }

  function destroy(): void {
    destroyed = true;
    runId++;          // abort any in-flight animation
    typing = false;
    skipRequested = false;
  }

  return { animate, skip, isTyping, destroy };
}
