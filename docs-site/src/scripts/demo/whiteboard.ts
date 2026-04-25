// ═══════ Whiteboard canvas renderer ═══════
// Extracted from demo.astro inline script (Requirements 1.3, 7.2, 13.3)

import type { Step, BoardNode, BoardArrow, BoardNote } from './steps';

/** Named constants replacing all magic numbers in the whiteboard renderer */
export const WHITEBOARD = {
  NODE_RADIUS: 56,
  NODE_RADIUS_SMALL: 34,
  ARROW_HEAD_SIZE: 11,
  ARROW_PAD: 64,
  GRID_SPACING_Y: 58,
  GRID_MIN_SPACING_X: 120,
  FRAME_MARGIN: 16,
  ANIMATION_DURATION_MS: 1200,
  GRID_START: 40,
  GRID_INSET: 30,
  GRID_LINE_WIDTH: 0.5,
  FRAME_LINE_WIDTH: 1,
  ARROW_LINE_WIDTH: 2.2,
  ARROW_HEAD_LINE_WIDTH: 2,
  ARROW_HEAD_ANGLE: 0.4,
  ARROW_LABEL_FONT_SIZE: 17,
  ARROW_LABEL_OFFSET_Y: 14,
  NODE_LABEL_FONT_SIZE: 30,
  NODE_LABEL_FONT_SIZE_SMALL: 16,
  NODE_SUB_FONT_SIZE: 16,
  NODE_SUB_FONT_SIZE_SMALL: 12,
  NODE_SUB_OFFSET_Y: 19,
  NODE_SUB_OFFSET_Y_SMALL: 13,
  NODE_LABEL_OFFSET_Y: 6,
  NODE_HIGHLIGHT_LINE_WIDTH: 1.8,
  NODE_DEFAULT_LINE_WIDTH: 1.3,
  NOTE_DEFAULT_SIZE: 16,
  CHALK_OFFSETS: [
    { dx: 0, dy: 0, alpha: 0.85 },
    { dx: -0.8, dy: 0.45, alpha: 0.2 },
    { dx: 0.65, dy: -0.45, alpha: 0.16 },
    { dx: 0.2, dy: 0.95, alpha: 0.09 },
  ] as const,
  CHALK_LINE_OFFSETS: [
    { dx: 0, dy: 0, alphaScale: 1, widthAdd: 0 },
    { dx: -0.7, dy: 0.5, alphaScale: 0.24, widthAdd: 0.75 },
    { dx: 0.55, dy: -0.45, alphaScale: 0.18, widthAdd: 1 },
    { dx: 0.15, dy: 0.9, alphaScale: 0.08, widthAdd: 1.4 },
  ] as const,
  DASH_PATTERN: [6, 8] as readonly number[],
  // Colors mirror design-system tokens from tokens.css. Canvas can't
  // read CSS vars directly, so these are hex/rgba equivalents of
  // --bg-elevated, --fg1/--fg2, --frog-400/--frog-200, --bolt, --border.
  COLORS: {
    bg: '#171b18',
    grid: 'rgba(43,48,40,0.65)',
    text: '#e8ede6',
    muted: 'rgba(154,164,151,0.78)',
    accent: '#52c72a',
    accentDim: 'rgba(82,199,42,0.15)',
    warn: '#f5c518',
    frame: 'rgba(43,48,40,0.6)',
    labelBackplate: 'rgba(23,27,24,0.88)',
    highlightFill: 'rgba(82,199,42,0.10)',
    defaultFill: 'rgba(23,27,24,0.4)',
    highlightStroke: '#a8e88a',
    defaultStroke: 'rgba(232,237,230,0.7)',
  },
  FONTS: {
    hand: "'JetBrains Mono', ui-monospace, Menlo, Consolas, monospace",
    mono: "'JetBrains Mono', ui-monospace, Menlo, Consolas, monospace",
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
      ctx.lineJoin = 'round';
      ctx.shadowColor = color;
      ctx.shadowBlur = 2.5;
      ctx.stroke();
      if (isDashed) ctx.setLineDash([]);
      ctx.restore();
    }
  }

  function chalkRect(x: number, y: number, w: number, h: number, color: string, width: number): void {
    for (const o of WB.CHALK_OFFSETS) {
      ctx.save();
      ctx.globalAlpha = o.alpha;
      ctx.strokeStyle = color;
      ctx.lineWidth = width + o.alpha * 0.7;
      ctx.lineJoin = 'round';
      ctx.shadowColor = color;
      ctx.shadowBlur = 2;
      ctx.beginPath();
      ctx.moveTo(x + o.dx, y + o.dy);
      ctx.lineTo(x + w + o.dx + 0.8, y + o.dy - 0.35);
      ctx.lineTo(x + w + o.dx - 0.4, y + h + o.dy + 0.75);
      ctx.lineTo(x + o.dx + 0.35, y + h + o.dy - 0.25);
      ctx.closePath();
      ctx.stroke();
      ctx.restore();
    }
  }

  function chalkText(text: string, x: number, y: number, color: string, alpha = 1): void {
    for (const o of WB.CHALK_OFFSETS) {
      ctx.save();
      ctx.globalAlpha = alpha * o.alpha;
      ctx.fillStyle = color;
      ctx.shadowColor = color;
      ctx.shadowBlur = 1.8;
      ctx.fillText(text, x + o.dx, y + o.dy);
      ctx.restore();
    }
  }

  // ── background ──

  function drawBg(time: number): void {
    const ww = logicalW();
    const hh = logicalH();
    ctx.clearRect(0, 0, ww, hh);
    const gradient = ctx.createLinearGradient(0, 0, ww, hh);
    gradient.addColorStop(0, '#07150f');
    gradient.addColorStop(0.56, WB.COLORS.bg);
    gradient.addColorStop(1, '#092316');
    ctx.fillStyle = gradient;
    ctx.fillRect(0, 0, ww, hh);

    // static chalk dust and erased smudges
    ctx.save();
    ctx.strokeStyle = 'rgba(232,238,225,0.026)';
    ctx.lineWidth = 0.8;
    for (let i = 0; i < 82; i++) {
      const x = ((i * 97) % Math.max(1, Math.floor(ww))) + 0.5;
      const y = ((i * 53) % Math.max(1, Math.floor(hh))) + 0.5;
      const len = 9 + ((i * 17) % 22);
      ctx.globalAlpha = 0.08 + ((i * 13) % 9) / 100;
      ctx.beginPath();
      ctx.moveTo(x, y);
      ctx.lineTo(x + len, y + ((i % 3) - 1) * 1.4);
      ctx.stroke();
    }
    ctx.restore();

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
    chalkRect(
      WB.FRAME_MARGIN, WB.FRAME_MARGIN,
      ww - WB.FRAME_MARGIN * 2, hh - WB.FRAME_MARGIN * 2,
      `rgba(229,239,223,${0.14 + pulse * 0.04})`,
      WB.FRAME_LINE_WIDTH,
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
    chalkRect(cx - r, cy - r, r * 2, r * 2, col, lw);

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
    chalkText(nd.label, cx, cy - (nd.sub ? WB.NODE_LABEL_OFFSET_Y : 0), nd.highlight ? WB.COLORS.accent : WB.COLORS.text);

    // Sub-label below the main label
    if (nd.sub) {
      const subSize = nd.small ? WB.NODE_SUB_FONT_SIZE_SMALL : WB.NODE_SUB_FONT_SIZE;
      const subOffset = nd.small ? WB.NODE_SUB_OFFSET_Y_SMALL : WB.NODE_SUB_OFFSET_Y;
      ctx.font = `400 ${subSize}px ${WB.FONTS.hand}`;
      ctx.fillStyle = WB.COLORS.muted;
      chalkText(nd.sub, cx, cy + subOffset, WB.COLORS.muted, 0.9);
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
      const metrics = ctx.measureText(label);
      ctx.fillStyle = WB.COLORS.labelBackplate;
      ctx.fillRect(
        mx - metrics.width / 2 - 10,
        my - WB.ARROW_LABEL_OFFSET_Y - WB.ARROW_LABEL_FONT_SIZE - 5,
        metrics.width + 20,
        WB.ARROW_LABEL_FONT_SIZE + 10,
      );
      ctx.fillStyle = WB.COLORS.accent;
      chalkText(label, mx, my - WB.ARROW_LABEL_OFFSET_Y, WB.COLORS.accent);
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
    chalkText(note.text, x, y, col, 0.95);
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
