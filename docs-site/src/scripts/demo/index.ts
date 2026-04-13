// ═══════ Demo page entry point ═══════
// Initializes whiteboard, terminal, step navigation, keyboard shortcuts,
// and pip indicators. (Requirements 1.3, 6.1, 6.2, 6.3, 6.4)

import { STEPS } from './steps';
import { initWhiteboard } from './whiteboard';
import { createTerminalAnimator } from './terminal';
import type { TerminalAnimator } from './terminal';

/**
 * Initialize the demo page: whiteboard canvas, terminal animator,
 * step navigation, keyboard shortcuts, and pip indicators.
 */
export function initDemo(): void {
  // ── DOM elements ──
  const canvas = document.querySelector<HTMLCanvasElement>('#scene canvas');
  const lessonCard = document.getElementById('lesson-card');
  const annotation = document.getElementById('annotation');
  const termBody = document.getElementById('term-body');
  const pipsContainer = document.querySelector<HTMLElement>('.pips');
  const prevBtn = document.getElementById('prevBtn') as HTMLButtonElement | null;
  const nextBtn = document.getElementById('nextBtn') as HTMLButtonElement | null;

  if (!canvas || !lessonCard || !termBody) return;

  // ── State ──
  let step = 0;
  let sceneStartedAt = performance.now();

  // ── Terminal animator ──
  const terminal: TerminalAnimator = createTerminalAnimator(termBody);

  // ── Whiteboard ──
  const whiteboard = initWhiteboard(
    canvas,
    () => STEPS[step],
    () => sceneStartedAt,
  );

  // ── Nav state ──
  function updateNavState(): void {
    if (prevBtn) prevBtn.disabled = terminal.isTyping();
    if (nextBtn) {
      nextBtn.textContent = terminal.isTyping()
        ? 'Skip'
        : step === STEPS.length - 1
          ? 'Restart'
          : 'Continue';
    }
  }

  // ── Render lesson card, annotation, pips, and trigger terminal animation ──
  function renderLesson(): void {
    const s = STEPS[step];

    // Lesson card content
    lessonCard.innerHTML =
      `<div class="step-n">Step ${step + 1} / ${STEPS.length}</div>` +
      `<div class="step-t">${s.t}</div>` +
      `<div class="step-desc">${s.d} <a href="${s.link}">Learn more &#8594;</a></div>`;

    // Annotation
    if (annotation) {
      if (s.note) {
        annotation.innerHTML = s.note;
        annotation.style.display = 'block';
      } else {
        annotation.style.display = 'none';
      }
    }

    // Pip indicators
    if (pipsContainer) {
      pipsContainer.innerHTML = STEPS.map((_, i) =>
        `<div class="pip ${i < step ? 'done' : i === step ? 'on' : ''}" data-step="${i}"></div>`,
      ).join('');
    }

    // Update canvas aria-label for accessibility (Req 5.3)
    canvas!.setAttribute('aria-label', `Animated whiteboard: ${s.t}`);

    updateNavState();
    sceneStartedAt = performance.now();

    // Animate terminal, then update nav state when done
    terminal.animate(s.term).then(() => {
      updateNavState();
    });
  }

  // ── Navigation ──
  function go(d: number): void {
    // If typing and user presses next → skip animation (Req 6.3)
    if (d === 1 && terminal.isTyping()) {
      terminal.skip();
      updateNavState();
      return;
    }
    // Block navigation while typing
    if (terminal.isTyping()) return;

    if (d === 1 && step === STEPS.length - 1) {
      step = 0;
    } else if (d === -1 && step === 0) {
      step = STEPS.length - 1;
    } else {
      step = Math.max(0, Math.min(STEPS.length - 1, step + d));
    }
    renderLesson();
  }

  function goTo(i: number): void {
    if (terminal.isTyping()) return;
    step = Math.max(0, Math.min(STEPS.length - 1, i));
    renderLesson();
  }

  // ── Button click handlers ──
  if (prevBtn) {
    prevBtn.addEventListener('click', () => go(-1));
  }
  if (nextBtn) {
    nextBtn.addEventListener('click', () => go(1));
  }

  // ── Pip click handlers ──
  if (pipsContainer) {
    pipsContainer.addEventListener('click', (e) => {
      const target = e.target as HTMLElement;
      const stepAttr = target.dataset.step;
      if (stepAttr !== undefined) {
        goTo(parseInt(stepAttr, 10));
      }
    });
  }

  // ── Keyboard shortcuts (Req 6.1, 6.2, 6.3) ──
  document.addEventListener('keydown', (e: KeyboardEvent) => {
    switch (e.key) {
      case 'ArrowRight':
        go(1);
        break;
      case 'ArrowLeft':
        go(-1);
        break;
      case 'Escape':
        if (terminal.isTyping()) {
          terminal.skip();
          updateNavState();
        }
        break;
    }
  });

  // ── Window resize (Req 1.3) ──
  window.addEventListener('resize', () => {
    whiteboard.resize();
  });

  // ── Initial render ──
  renderLesson();
}
