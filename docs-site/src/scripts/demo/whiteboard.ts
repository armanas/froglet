// ═══════ Whiteboard canvas renderer ═══════
// Extracted from demo.astro inline script (Requirements 1.3, 7.2, 13.3)

import type { Step, BoardNode, BoardArrow, BoardNote } from './steps';

/** Named constants replacing all magic numbers in the whiteboard renderer */
export const WHITEBOARD = {
  NODE_RADIUS: 48,
  NODE_RADIUS_SMALL: 28,
  ARROW_HEAD_SIZE: 8,
  ARROW_PAD: 52,
  GRID_SPACING_Y: 44,
  GRID_MIN_SPACING_X: 120,
  FRAME_MARGIN: 16,
  ANIMATION_DURATION_MS: 1200,
  GRID_START: 40,
  GRID_INSET: 30,
  GRID_LINE_WIDTH: 0.5,
  FRAME_LINE_WIDTH: 1,
  ARROW_LINE_WIDTH: 1.4,
  ARROW_HEAD_LINE_WIDTH: 1.3,
  ARROW_HEAD_ANGLE: 0.4,
  ARROW_LABEL_FONT_SIZE: 12,
  ARROW_LABEL_OFFSET_Y: 8,
  NODE_LABEL_FONT_SIZE: 22,
  NODE_LABEL_FONT_SIZE_SMALL: 13,
  NODE_SUB_FONT_SIZE: 12,
  NODE_SUB_FONT_SIZE_SMALL: 10,
  NODE_SUB_OFFSET_Y: 14,
  NODE_SUB_OFFSET_Y_SMALL: 10,
  NODE_LABEL_OFFSET_Y: 6,
  NODE_HIGHLIGHT_LINE_WIDTH: 1.8,
  NODE_DEFAULT_LINE_WIDTH: 1.3,
  NOTE_DEFAULT_SIZE: 13,
  CHALK_OFFSETS: [
    { dx: 0, dy: 0, alpha: 0.85 },
    { dx: -0.4, dy: 0.3, alpha: 0.18 },
    { dx: 0.35, dy: -0.25, alpha: 0.12 },
  ] as const,
  CHALK_LINE_OFFSETS: [
    { dx: 0, dy: 0, alphaScale: 1, widthAdd: 0 },
    { dx: -0.5, dy: 0.4, alphaScale: 0.2, widthAdd: 0.6 },
    { dx: 0.4, dy: -0.3, alphaScale: 0.15, widthAdd: 0.9 },
  ] as const,
  DASH_PATTERN: [6, 8] as readonly number[],
  COLORS: {
    bg: '#101712',
    grid: 'rgba(231,238,222,0.035)',
    text: '#edf1e7',
    muted: 'rgba(213,223,204,0.52)',
    accent: '#b8ff9a',
    accentDim: 'rgba(184,255,154,0.25)',
    warn: '#efe39a',
    frame: 'rgba(205,223,198,0.10)',
    highlightFill: 'rgba(184,255,154,0.08)',
    defaultFill: 'rgba(15,22,17,0.12)',
    highlightStroke: '#b8ff9a',
    defaultStroke: 'rgba(233,240,226,0.6)',
  },
  FONTS: {
    hand: "'Source Sans 3', system-ui, sans-serif",
    mono: "'JetBrains Mono', monospace",
  },
} as const;


interface NodePosition {
  cx: number;
  cy: number;
  r: number;
}

/**
 * Initialize the whiteboard canvas renderer.
 *
 * @param canvas - The canvas element to render into
 * @param getStep - Returns the current Step data
 * @param getSceneStartedAt - Returns the timestamp when the current scene started
 * @returns Object with resize() and destroy() methods
 */
export function initWhiteboard(
  canvas: HTMLCanvasElement,
  getStep: () => Step,
  getSceneStartedAt: () => number,
): { resize: () => void; destroy: () => void } {
  const ctx = canvas.getContext('2d');

  // Null-check guard: skip rendering if 2D context unavailable (Req 3.2)
  if (!ctx) {
    return {
      resize() {},
      destroy() {},
    };
  }

  const WB = WHITEBOARD;
  let W = 0;
  let H = 0;
  let animationFrameId: number | null = null;
  let destroyed = false;

  // ── helpers ──

  function logicalW(): number {
    return W / devicePixelRatio;
  }

  function logicalH(): number {
    return H / devicePixelRatio;
  }

  function resize(): void {
    const scene = canvas.parentElement;
    if (!scene) return;
    W = Math.max(1, scene.clientWidth * devicePixelRatio);
    H = Math.max(1, scene.clientHeight * devicePixelRatio);
    canvas.width = W;
    canvas.height = H;
    ctx.setTransform(devicePixelRatio, 0, 0, devicePixelRatio, 0, 0);
  }

  // ── chalk-style line helper ──

  function chalkLine(
    ax: number, ay: number,
    bx: number, by: number,
    color: string, width: number, alpha: number, isDashed: boolean,
  ): void {
    for (const o of WB.CHALK_LINE_OFFSETS) {
      ctx.save();
      ctx.globalAlpha = alpha * o.alphaScale;
      if (isDashed) ctx.setLineDash(WB.DASH_PATTERN as number[]);
      ctx.beginPath();
      ctx.moveTo(ax + o.dx, ay + o.dy);
      ctx.lineTo(bx + o.dx, by + o.dy);
      ctx.strokeStyle = color;
      ctx.lineWidth = width + o.widthAdd;
      ctx.lineCap = 'round';
      ctx.stroke();
      if (isDashed) ctx.setLineDash([]);
      ctx.restore();
    }
  }

  // ── background ──

  function drawBg(time: number): void {
    const ww = logicalW();
    const hh = logicalH();
    ctx.clearRect(0, 0, ww, hh);
    ctx.fillStyle = WB.COLORS.bg;
    ctx.fillRect(0, 0, ww, hh);

    // subtle chalk grid
    ctx.strokeStyle = WB.COLORS.grid;
    ctx.lineWidth = WB.GRID_LINE_WIDTH;
    for (let y = WB.GRID_START; y < hh; y += WB.GRID_SPACING_Y) {
      ctx.beginPath();
      ctx.moveTo(WB.GRID_INSET, y);
      ctx.lineTo(ww - WB.GRID_INSET, y);
      ctx.stroke();
    }
    for (let x = WB.GRID_START; x < ww; x += Math.max(WB.GRID_MIN_SPACING_X, ww / 5)) {
      ctx.beginPath();
      ctx.moveTo(x, WB.GRID_INSET);
      ctx.lineTo(x, hh - WB.GRID_INSET);
      ctx.stroke();
    }

    // frame with subtle pulse
    const pulse = 0.5 + 0.15 * Math.sin(time / 1000);
    ctx.strokeStyle = `rgba(229,239,223,${0.06 + pulse * 0.04})`;
    ctx.lineWidth = WB.FRAME_LINE_WIDTH;
    ctx.strokeRect(
      WB.FRAME_MARGIN, WB.FRAME_MARGIN,
      ww - WB.FRAME_MARGIN * 2, hh - WB.FRAME_MARGIN * 2,
    );
  }

  // ── node drawing (Req 13.3: text labels inside/adjacent to circles) ──

  function drawNode(nd: BoardNode): NodePosition {
    const ww = logicalW();
    const hh = logicalH();
    const cx = nd.x * ww;
    const cy = nd.y * hh;
    const r = nd.small ? WB.NODE_RADIUS_SMALL : WB.NODE_RADIUS;

    // square fill
    ctx.fillStyle = nd.highlight ? WB.COLORS.highlightFill : WB.COLORS.defaultFill;
    ctx.fillRect(cx - r, cy - r, r * 2, r * 2);

    // chalk square: multi-pass offset rendering
    const col = nd.highlight ? WB.COLORS.highlightStroke : WB.COLORS.defaultStroke;
    const lw = nd.highlight ? WB.NODE_HIGHLIGHT_LINE_WIDTH : WB.NODE_DEFAULT_LINE_WIDTH;
    for (const o of WB.CHALK_OFFSETS) {
      ctx.save();
      ctx.globalAlpha = o.alpha;
      ctx.strokeStyle = col;
      ctx.lineWidth = lw + o.alpha * 0.6;
      ctx.strokeRect(cx - r + o.dx, cy - r + o.dy, r * 2, r * 2);
      ctx.restore();
    }

    // Sequence diagram lifeline (dashed vertical line below node)
    if (nd.lifeline !== undefined) {
      const lifelineEnd = nd.lifeline * hh;
      chalkLine(cx, cy + r, cx, lifelineEnd, WB.COLORS.defaultStroke, 0.8, 0.4, true);
    }

    // Text label inside the square (Req 13.3: accessibility)
    const labelSize = nd.small ? WB.NODE_LABEL_FONT_SIZE_SMALL : WB.NODE_LABEL_FONT_SIZE;
    ctx.font = `700 ${labelSize}px ${WB.FONTS.hand}`;
    ctx.fillStyle = nd.highlight ? WB.COLORS.accent : WB.COLORS.text;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillText(nd.label, cx, cy - (nd.sub ? WB.NODE_LABEL_OFFSET_Y : 0));

    // Sub-label below the main label
    if (nd.sub) {
      const subSize = nd.small ? WB.NODE_SUB_FONT_SIZE_SMALL : WB.NODE_SUB_FONT_SIZE;
      const subOffset = nd.small ? WB.NODE_SUB_OFFSET_Y_SMALL : WB.NODE_SUB_OFFSET_Y;
      ctx.font = `400 ${subSize}px ${WB.FONTS.hand}`;
      ctx.fillStyle = WB.COLORS.muted;
      ctx.fillText(nd.sub, cx, cy + subOffset);
    }

    ctx.textAlign = 'left';
    ctx.textBaseline = 'alphabetic';
    return { cx, cy, r };
  }

  // ── arrow drawing ──

  function drawArrowLine(
    x1: number, y1: number,
    x2: number, y2: number,
    label: string | undefined,
    bidi: boolean | undefined,
    dashed: boolean,
    progress: number,
  ): void {
    if (progress <= 0) return;

    const dx = x2 - x1;
    const dy = y2 - y1;
    const len = Math.hypot(dx, dy);
    const nx = dx / len;
    const ny = dy / len;

    const s1x = x1 + nx * WB.ARROW_PAD;
    const s1y = y1 + ny * WB.ARROW_PAD;
    const s2x = x1 + nx * (WB.ARROW_PAD + (len - WB.ARROW_PAD * 2) * progress);
    const s2y = y1 + ny * (WB.ARROW_PAD + (len - WB.ARROW_PAD * 2) * progress);

    chalkLine(s1x, s1y, s2x, s2y, WB.COLORS.accent, WB.ARROW_LINE_WIDTH, 0.8, dashed);

    // arrowhead (chalk style)
    if (progress > 0.9) {
      const angle = Math.atan2(dy, dx);
      const sz = WB.ARROW_HEAD_SIZE;
      const lx = s2x - Math.cos(angle - WB.ARROW_HEAD_ANGLE) * sz;
      const ly = s2y - Math.sin(angle - WB.ARROW_HEAD_ANGLE) * sz;
      const rx = s2x - Math.cos(angle + WB.ARROW_HEAD_ANGLE) * sz;
      const ry = s2y - Math.sin(angle + WB.ARROW_HEAD_ANGLE) * sz;
      chalkLine(lx, ly, s2x, s2y, WB.COLORS.accent, WB.ARROW_HEAD_LINE_WIDTH, 0.85, false);
      chalkLine(rx, ry, s2x, s2y, WB.COLORS.accent, WB.ARROW_HEAD_LINE_WIDTH, 0.85, false);

      if (bidi) {
        const a2 = angle + Math.PI;
        const l2x = s1x - Math.cos(a2 - WB.ARROW_HEAD_ANGLE) * sz;
        const l2y = s1y - Math.sin(a2 - WB.ARROW_HEAD_ANGLE) * sz;
        const r2x = s1x - Math.cos(a2 + WB.ARROW_HEAD_ANGLE) * sz;
        const r2y = s1y - Math.sin(a2 + WB.ARROW_HEAD_ANGLE) * sz;
        chalkLine(l2x, l2y, s1x, s1y, WB.COLORS.accent, WB.ARROW_HEAD_LINE_WIDTH, 0.85, false);
        chalkLine(r2x, r2y, s1x, s1y, WB.COLORS.accent, WB.ARROW_HEAD_LINE_WIDTH, 0.85, false);
      }
    }

    // label
    if (label && progress > 0.5) {
      const mx = (s1x + s2x) / 2;
      const my = (s1y + s2y) / 2;
      ctx.save();
      ctx.font = `700 ${WB.ARROW_LABEL_FONT_SIZE}px ${WB.FONTS.hand}`;
      ctx.fillStyle = WB.COLORS.accent;
      ctx.globalAlpha = Math.min(1, (progress - 0.5) * 4);
      ctx.textAlign = 'center';
      ctx.fillText(label, mx, my - WB.ARROW_LABEL_OFFSET_Y);
      ctx.textAlign = 'left';
      ctx.restore();
    }
  }

  // ── note drawing ──

  function drawNote(note: BoardNote): void {
    const ww = logicalW();
    const hh = logicalH();
    const x = note.x * ww;
    const y = note.y * hh;
    const sz = note.size || WB.NOTE_DEFAULT_SIZE;
    const col =
      note.color === 'accent' ? WB.COLORS.accent :
      note.color === 'warn' ? WB.COLORS.warn :
      note.color === 'muted' ? WB.COLORS.muted :
      WB.COLORS.text;
    const weight = (note.color === 'accent' || note.color === 'warn') ? 700 : 400;
    ctx.font = `${weight} ${sz}px ${WB.FONTS.hand}`;
    ctx.fillStyle = col;
    ctx.textAlign = 'left';
    ctx.fillText(note.text, x, y);
  }

  // ── scene composition ──

  function drawScene(time: number): void {
    drawBg(time);
    const s = getStep();
    if (!s || !s.board) return;

    const board = s.board;
    const progress = Math.min(1, (time - getSceneStartedAt()) / WB.ANIMATION_DURATION_MS);
    const ease = 1 - Math.pow(1 - progress, 3);

    // draw nodes
    const nodePos: Record<string, NodePosition> = {};
    for (const nd of board.nodes || []) {
      nodePos[nd.id] = drawNode(nd);
    }

    // draw arrows
    for (let i = 0; i < (board.arrows || []).length; i++) {
      const a = board.arrows[i];
      const from = nodePos[a.from];
      const to = nodePos[a.to];
      if (!from || !to) continue;
      const aProgress = Math.min(1, Math.max(0, (ease - i * 0.1) / 0.6));
      const fy = a.y !== undefined ? a.y * logicalH() : from.cy;
      const ty = a.y !== undefined ? a.y * logicalH() : to.cy;
      drawArrowLine(from.cx, fy, to.cx, ty, a.label, a.bidi, a.style === 'dashed', aProgress);
    }

    // draw notes with fade-in
    if (ease > 0.4) {
      ctx.globalAlpha = Math.min(1, (ease - 0.4) * 2.5);
      for (const note of board.notes || []) {
        drawNote(note);
      }
      ctx.globalAlpha = 1;
    }
  }

  // ── animation loop with try/catch (Req 3.3) ──

  function loop(time?: number): void {
    if (destroyed) return;
    try {
      drawScene(time || performance.now());
    } catch (err) {
      console.error('[whiteboard] draw error:', err);
    }
    animationFrameId = requestAnimationFrame(loop);
  }

  // ── start ──

  resize();
  animationFrameId = requestAnimationFrame(loop);

  function destroy(): void {
    destroyed = true;
    if (animationFrameId !== null) {
      cancelAnimationFrame(animationFrameId);
      animationFrameId = null;
    }
  }

  return { resize, destroy };
}
