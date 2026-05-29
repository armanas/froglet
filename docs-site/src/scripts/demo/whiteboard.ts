// ═══════ Whiteboard canvas renderer ═══════
// Clean, crisp, high-precision technical diagram rendering (No gamification, high contrast, perfect alignment)

import type { Step, BoardNode, BoardArrow, BoardNote } from './steps';

/** Named constants for the clean visual workspace */
export const WHITEBOARD = {
  NODE_RADIUS: 56,
  NODE_RADIUS_SMALL: 34,
  ARROW_HEAD_SIZE: 10,
  FRAME_MARGIN: 16,
  ANIMATION_DURATION_MS: 1200,
  DASH_PATTERN: [5, 5] as readonly number[],
  // Sleek dark-mode developer color system matching tokens.css
  COLORS: {
    bg: '#0a0d0a', // very rich solid dark green-black background
    grid: 'rgba(232, 237, 230, 0.035)', // sharp, clean grid line
    text: '#e8ede6', // high-contrast crisp text
    muted: 'rgba(154, 164, 151, 0.72)', // clean muted labels
    accent: '#52c72a', // crisp green highlights
    accentDim: 'rgba(82, 199, 42, 0.08)',
    warn: '#f5c518',
    frame: 'rgba(232, 237, 230, 0.06)',
    labelBackplate: '#0a0d0a', // solid background for text blocks
    highlightFill: 'rgba(82,199,42,0.12)',
    defaultFill: 'rgba(23,27,24,0.7)',
    highlightStroke: '#a8e88a',
    defaultStroke: 'rgba(232,237,230,0.6)',
  },
  FONTS: {
    hand: "'JetBrains Mono', ui-monospace, Menlo, Consolas, monospace", // changed hand font to mono for premium crisp style
    mono: "'JetBrains Mono', ui-monospace, Menlo, Consolas, monospace",
  },
} as const;

interface NodePosition {
  cx: number;
  cy: number;
  r: number;
  w: number;
  h: number;
}

export function initWhiteboard(
  canvas: HTMLCanvasElement,
  getStep: () => Step,
  getSceneStartedAt: () => number,
): {
  resize: () => void;
  destroy: () => void;
  setActiveTool: (tool: string | null) => void;
  clearBoard: () => void;
} {
  const ctx = canvas.getContext('2d');

  if (!ctx) {
    return {
      resize() {},
      destroy() {},
      setActiveTool() {},
      clearBoard() {},
    };
  }

  const WB = WHITEBOARD;
  let W = 0;
  let H = 0;
  let animationFrameId: number | null = null;
  let destroyed = false;

  function logicalW(): number {
    return W / devicePixelRatio;
  }

  function logicalH(): number {
    return H / devicePixelRatio;
  }

  function resize(): void {
    const scene = canvas.parentElement;
    if (!scene) return;
    const logicalWVal = Math.max(850, scene.clientWidth);
    const logicalHVal = scene.clientHeight;
    canvas.style.width = `${logicalWVal}px`;
    canvas.style.height = `${logicalHVal}px`;
    W = logicalWVal * devicePixelRatio;
    H = logicalHVal * devicePixelRatio;
    canvas.width = W;
    canvas.height = H;
    ctx.setTransform(devicePixelRatio, 0, 0, devicePixelRatio, 0, 0);
  }

  // ── Crisp Render Helpers ──

  function drawLine(
    ax: number, ay: number,
    bx: number, by: number,
    color: string, width: number, alpha: number, isDashed: boolean,
  ): void {
    ctx.save();
    ctx.globalAlpha = alpha;
    if (isDashed) ctx.setLineDash(WB.DASH_PATTERN as number[]);
    ctx.beginPath();
    ctx.moveTo(ax, ay);
    ctx.lineTo(bx, by);
    ctx.strokeStyle = color;
    ctx.lineWidth = width;
    ctx.lineCap = 'round';
    ctx.lineJoin = 'round';
    ctx.stroke();
    ctx.restore();
  }

  function drawRect(x: number, y: number, w: number, h: number, color: string, width: number, fill?: string): void {
    ctx.save();
    if (fill) {
      ctx.fillStyle = fill;
      ctx.fillRect(x, y, w, h);
    }
    ctx.strokeStyle = color;
    ctx.lineWidth = width;
    ctx.lineJoin = 'round';
    ctx.beginPath();
    ctx.rect(x, y, w, h);
    ctx.stroke();
    ctx.restore();
  }

  function drawRectProgressive(
    x: number, y: number,
    w: number, h: number,
    color: string, width: number,
    drawProgress: number,
    fill?: string,
  ): void {
    if (drawProgress <= 0) return;

    if (drawProgress >= 1) {
      drawRect(x, y, w, h, color, width, fill);
      return;
    }

    if (fill) {
      ctx.save();
      ctx.globalAlpha = drawProgress;
      ctx.fillStyle = fill;
      ctx.fillRect(x, y, w, h);
      ctx.restore();
    }

    const perimeter = (w * 2) + (h * 2);
    const targetLen = drawProgress * perimeter;
    let remaining = targetLen;

    // Segment 1: Top
    const draw1 = Math.min(w, remaining);
    if (draw1 > 0) {
      drawLine(x, y, x + draw1, y, color, width, 1, false);
      remaining -= draw1;
    }

    // Segment 2: Right
    const draw2 = Math.min(h, remaining);
    if (draw2 > 0) {
      drawLine(x + w, y, x + w, y + draw2, color, width, 1, false);
      remaining -= draw2;
    }

    // Segment 3: Bottom
    const draw3 = Math.min(w, remaining);
    if (draw3 > 0) {
      drawLine(x + w, y + h, x + w - draw3, y + h, color, width, 1, false);
      remaining -= draw3;
    }

    // Segment 4: Left
    const draw4 = Math.min(h, remaining);
    if (draw4 > 0) {
      drawLine(x, y + h, x, y + h - draw4, color, width, 1, false);
    }
  }

  function drawText(text: string, x: number, y: number, color: string, alpha = 1): void {
    ctx.save();
    ctx.globalAlpha = alpha;
    ctx.fillStyle = color;
    ctx.fillText(text, x, y);
    ctx.restore();
  }

  function drawTextProgressive(
    text: string,
    x: number, y: number,
    color: string,
    drawProgress: number,
    alpha = 1,
  ): void {
    if (drawProgress <= 0) return;
    const visibleChars = Math.floor(drawProgress * text.length);
    if (visibleChars <= 0) return;
    drawText(text.substring(0, visibleChars), x, y, color, alpha);
  }

  function drawTextCenteredProgressive(
    text: string,
    x: number, y: number,
    color: string,
    drawProgress: number,
    alpha = 1,
  ): void {
    if (drawProgress <= 0) return;
    const visibleChars = Math.floor(drawProgress * text.length);
    if (visibleChars <= 0) return;
    const visibleText = text.substring(0, visibleChars);

    ctx.save();
    ctx.textAlign = 'left';
    const fullWidth = ctx.measureText(text).width;
    const startX = x - fullWidth / 2;
    drawText(visibleText, startX, y, color, alpha);
    ctx.restore();
  }

  // ── Background & Grid ──

  function drawBg(time: number): void {
    const ww = logicalW();
    const hh = logicalH();
    ctx.clearRect(0, 0, ww, hh);

    // Deep modern charcoal solid background
    ctx.fillStyle = WB.COLORS.bg;
    ctx.fillRect(0, 0, ww, hh);

    // Crisp high-precision fine grid
    ctx.strokeStyle = WB.COLORS.grid;
    ctx.lineWidth = 1;
    const gridSize = 64;
    for (let x = 0; x < ww; x += gridSize) {
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, hh);
      ctx.stroke();
    }
    for (let y = 0; y < hh; y += gridSize) {
      ctx.beginPath();
      ctx.moveTo(0, y);
      ctx.lineTo(ww, y);
      ctx.stroke();
    }

    // Outer frame boundary
    drawRect(
      WB.FRAME_MARGIN, WB.FRAME_MARGIN,
      ww - WB.FRAME_MARGIN * 2, hh - WB.FRAME_MARGIN * 2,
      WB.COLORS.frame,
      1,
    );
  }

  // ── Components rendering ──

  function drawNode(nd: BoardNode, ease: number): NodePosition {
    const ww = logicalW();
    const hh = logicalH();
    const cx = nd.x * ww;
    const cy = nd.y * hh;
    const r = nd.small ? WB.NODE_RADIUS_SMALL : WB.NODE_RADIUS;

    // Setup typeface sizing
    const labelSize = nd.small ? 13 : 15;
    ctx.font = `700 ${labelSize}px ${WB.FONTS.mono}`;
    ctx.textBaseline = 'middle';

    // Calculate dynamic width based on actual text length
    const labelText = nd.label;
    const textWidth = ctx.measureText(labelText).width;
    
    // Nodes should be elegant rectangles with 24px padding on sides
    const boxW = Math.max(nd.small ? 84 : 124, textWidth + 24);
    const boxH = nd.small ? 38 : 56;

    // Node Outline & Fill Animation (ease: 0.0 to 0.4)
    const boxProgress = Math.min(1, ease / 0.4);
    const strokeColor = nd.highlight ? WB.COLORS.highlightStroke : WB.COLORS.defaultStroke;
    const fillColor = nd.highlight ? WB.COLORS.highlightFill : WB.COLORS.defaultFill;
    const lineWidth = nd.highlight ? 1.8 : 1.2;

    drawRectProgressive(
      cx - boxW / 2,
      cy - boxH / 2,
      boxW,
      boxH,
      strokeColor,
      lineWidth,
      boxProgress,
      fillColor,
    );

    // Sequence diagram lifeline (dashed vertical line below node, ease: 0.35 to 0.65)
    if (nd.lifeline !== undefined) {
      const lifelineProgress = Math.min(1, Math.max(0, (ease - 0.35) / 0.3));
      if (lifelineProgress > 0) {
        const lifelineEnd = (cy + boxH / 2) + (nd.lifeline * hh - (cy + boxH / 2)) * lifelineProgress;
        drawLine(
          cx,
          cy + boxH / 2,
          cx,
          lifelineEnd,
          WB.COLORS.defaultStroke,
          1,
          0.4 * lifelineProgress,
          true,
        );
      }
    }

    // Text label inside the rectangle (ease: 0.25 to 0.6)
    const labelProgress = Math.min(1, Math.max(0, (ease - 0.25) / 0.35));
    if (labelProgress > 0) {
      ctx.font = `700 ${labelSize}px ${WB.FONTS.mono}`;
      ctx.fillStyle = nd.highlight ? WB.COLORS.accent : WB.COLORS.text;
      ctx.textBaseline = 'middle';
      
      const labelY = cy - (nd.sub ? 8 : 0);
      drawTextCenteredProgressive(
        labelText,
        cx,
        labelY,
        nd.highlight ? WB.COLORS.accent : WB.COLORS.text,
        labelProgress,
      );
    }

    // Sub-label below the main label (ease: 0.35 to 0.7)
    if (nd.sub) {
      const subProgress = Math.min(1, Math.max(0, (ease - 0.35) / 0.35));
      if (subProgress > 0) {
        const subSize = nd.small ? 10 : 11;
        ctx.font = `400 ${subSize}px ${WB.FONTS.mono}`;
        ctx.fillStyle = WB.COLORS.muted;
        
        const subY = cy + (nd.small ? 10 : 13);
        drawTextCenteredProgressive(
          nd.sub,
          cx,
          subY,
          WB.COLORS.muted,
          subProgress,
          0.85 * subProgress,
        );
      }
    }

    ctx.textAlign = 'left';
    ctx.textBaseline = 'alphabetic';

    return { cx, cy, r, w: boxW, h: boxH };
  }

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
    if (len <= 0) return;

    const nx = dx / len;
    const ny = dy / len;

    const endX = x1 + nx * len * progress;
    const endY = y1 + ny * len * progress;

    // Draw neat crisp arrow line
    drawLine(x1, y1, endX, endY, WB.COLORS.accent, 1.8, 0.9, dashed);

    // Arrowhead drawing
    if (progress > 0.92) {
      const angle = Math.atan2(dy, dx);
      const headSize = WB.ARROW_HEAD_SIZE;
      const headAngle = 0.38;

      const lx = endX - Math.cos(angle - headAngle) * headSize;
      const ly = endY - Math.sin(angle - headAngle) * headSize;
      const rx = endX - Math.cos(angle + headAngle) * headSize;
      const ry = endY - Math.sin(angle + headAngle) * headSize;

      drawLine(lx, ly, endX, endY, WB.COLORS.accent, 1.8, 0.9, false);
      drawLine(rx, ry, endX, endY, WB.COLORS.accent, 1.8, 0.9, false);

      if (bidi) {
        const rAngle = angle + Math.PI;
        const l2x = x1 - Math.cos(rAngle - headAngle) * headSize;
        const l2y = y1 - Math.sin(rAngle - headAngle) * headSize;
        const r2x = x1 - Math.cos(rAngle + headAngle) * headSize;
        const r2y = y1 - Math.sin(rAngle + headAngle) * headSize;

        drawLine(l2x, l2y, x1, y1, WB.COLORS.accent, 1.8, 0.9, false);
        drawLine(r2x, r2y, x1, y1, WB.COLORS.accent, 1.8, 0.9, false);
      }
    }

    // Arrow text label
    if (label && progress > 0.4) {
      const mx = (x1 + endX) / 2;
      const my = (y1 + endY) / 2;
      ctx.save();
      ctx.font = `600 12px ${WB.FONTS.mono}`;
      ctx.textBaseline = 'middle';
      ctx.textAlign = 'center';

      const metrics = ctx.measureText(label);
      const bgW = metrics.width + 12;
      const bgH = 18;

      ctx.fillStyle = WB.COLORS.labelBackplate;
      ctx.fillRect(mx - bgW / 2, my - bgH / 2, bgW, bgH);

      ctx.strokeStyle = 'rgba(82, 199, 42, 0.2)';
      ctx.lineWidth = 1;
      ctx.strokeRect(mx - bgW / 2, my - bgH / 2, bgW, bgH);

      ctx.fillStyle = WB.COLORS.accent;
      drawText(label, mx, my, WB.COLORS.accent, Math.min(1, (progress - 0.4) * 5));
      ctx.restore();
    }
  }

  function drawNoteProgressive(note: BoardNote, noteProgress: number): void {
    const ww = logicalW();
    const hh = logicalH();
    const x = note.x * ww;
    const y = note.y * hh;
    const sz = Math.min(13, note.size || 13);
    const col =
      note.color === 'accent' ? WB.COLORS.accent :
      note.color === 'warn' ? WB.COLORS.warn :
      note.color === 'muted' ? WB.COLORS.muted :
      WB.COLORS.text;

    ctx.save();
    ctx.font = `400 ${sz}px ${WB.FONTS.mono}`;
    ctx.textBaseline = 'top';
    ctx.textAlign = 'left';
    drawTextProgressive(note.text, x, y, col, noteProgress, 0.95);
    ctx.restore();
  }

  // ── Scene Composition ──

  function drawScene(time: number): void {
    drawBg(time);
    const s = getStep();
    if (!s || !s.board) return;

    const board = s.board;
    const progress = Math.min(1, (time - getSceneStartedAt()) / WB.ANIMATION_DURATION_MS);
    const ease = 1 - Math.pow(1 - progress, 3); // nice cubic ease-out

    // 1. Draw system nodes
    const nodePos: Record<string, NodePosition> = {};
    for (const nd of board.nodes || []) {
      nodePos[nd.id] = drawNode(nd, ease);
    }

    // 2. Draw system arrows (calculating high-fidelity box intersection alignments)
    for (let i = 0; i < (board.arrows || []).length; i++) {
      const a = board.arrows[i];
      const from = nodePos[a.from];
      const to = nodePos[a.to];
      if (!from || !to) continue;

      const aProgress = Math.min(1, Math.max(0, (ease - i * 0.1) / 0.6));
      let x1 = from.cx;
      let y1 = from.cy;
      let x2 = to.cx;
      let y2 = to.cy;

      if (a.y !== undefined) {
        // Sequence diagram lifeline horizontal arrow
        const arrowY = a.y * logicalH();
        const isLeftToRight = x2 > x1;
        x1 = x1 + (isLeftToRight ? 3 : -3);
        x2 = x2 + (isLeftToRight ? -3 : 3);
        y1 = arrowY;
        y2 = arrowY;
        drawArrowLine(x1, y1, x2, y2, a.label, a.bidi, a.style === 'dashed', aProgress);
      } else {
        // Node-to-node arrow. Calculate bounding box intersection.
        const dx = x2 - x1;
        const dy = y2 - y1;
        const angle = Math.atan2(dy, dx);

        const getBoxBoundaryOffset = (w: number, h: number, theta: number) => {
          const absCos = Math.abs(Math.cos(theta));
          const absSin = Math.abs(Math.sin(theta));
          if (w * absSin < h * absCos) {
            return (w / 2) / absCos;
          } else {
            return (h / 2) / absSin;
          }
        };

        const offset1 = getBoxBoundaryOffset(from.w, from.h, angle) + 4;
        const offset2 = getBoxBoundaryOffset(to.w, to.h, angle + Math.PI) + 4;

        x1 = from.cx + Math.cos(angle) * offset1;
        y1 = from.cy + Math.sin(angle) * offset1;
        x2 = to.cx - Math.cos(angle) * offset2;
        y2 = to.cy - Math.sin(angle) * offset2;

        drawArrowLine(x1, y1, x2, y2, a.label, a.bidi, a.style === 'dashed', aProgress);
      }
    }

    // 3. Draw footnotes / notes
    const noteProgress = Math.min(1, Math.max(0, (ease - 0.6) / 0.4));
    if (noteProgress > 0) {
      for (const note of board.notes || []) {
        drawNoteProgressive(note, noteProgress);
      }
    }
  }

  // ── Animation Loop ──

  function loop(time?: number): void {
    if (destroyed) return;
    try {
      drawScene(time || performance.now());
    } catch (err) {
      console.error('[whiteboard] draw error:', err);
    }
    animationFrameId = requestAnimationFrame(loop);
  }

  resize();
  animationFrameId = requestAnimationFrame(loop);

  function destroy(): void {
    destroyed = true;
    if (animationFrameId !== null) {
      cancelAnimationFrame(animationFrameId);
      animationFrameId = null;
    }
  }

  return {
    resize,
    destroy,
    setActiveTool(tool: string | null) {},
    clearBoard() {},
  };
}
