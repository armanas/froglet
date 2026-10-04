import { describe, expect, it } from 'vitest';
import { WHITEBOARD, whiteboardPalette } from '../demo/whiteboard';

type Rgba = [number, number, number, number];

function parse(color: string): Rgba {
  if (color.startsWith('#')) {
    const h = color.slice(1);
    return [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16)).concat(1) as Rgba;
  }
  const parts = /rgba?\(([^)]+)\)/.exec(color)![1].split(',').map((p) => parseFloat(p));
  return [parts[0], parts[1], parts[2], parts[3] ?? 1];
}

function opaqueOver(fg: Rgba, bg: Rgba): [number, number, number] {
  const a = fg[3];
  return [0, 1, 2].map((i) => fg[i] * a + bg[i] * (1 - a)) as [number, number, number];
}

function luminance([r, g, b]: [number, number, number]): number {
  const lin = (v: number) => {
    const c = v / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** WCAG contrast ratio of `fg` drawn over the palette background. */
function contrastOnBackground(fg: string, bg: string): number {
  const back = parse(bg);
  const backRgb: [number, number, number] = [back[0], back[1], back[2]];
  const la = luminance(opaqueOver(parse(fg), back));
  const lb = luminance(backRgb);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}

describe('whiteboard theme palettes', () => {
  it('selects the light palette only for the light theme', () => {
    expect(whiteboardPalette('light').bg).not.toBe(whiteboardPalette('dark').bg);
    expect(whiteboardPalette('dark')).toBe(whiteboardPalette(undefined));
    expect(whiteboardPalette(null)).toBe(whiteboardPalette('something-else'));
  });

  it('keeps the exported default palette equal to the dark palette', () => {
    expect(WHITEBOARD.COLORS).toBe(whiteboardPalette('dark'));
  });

  it('defines every palette key in both themes', () => {
    expect(Object.keys(whiteboardPalette('light')).sort()).toEqual(Object.keys(whiteboardPalette('dark')).sort());
  });

  it('keeps light-theme diagram text at WCAG AA contrast on the scene background', () => {
    const light = whiteboardPalette('light');
    for (const key of ['text', 'muted', 'accent', 'warn', 'highlightStroke'] as const) {
      expect(contrastOnBackground(light[key], light.bg), `light ${key}`).toBeGreaterThanOrEqual(4.5);
    }
  });

  it('keeps dark-theme primary diagram colours at WCAG AA contrast', () => {
    const dark = whiteboardPalette('dark');
    for (const key of ['text', 'accent', 'warn', 'highlightStroke'] as const) {
      expect(contrastOnBackground(dark[key], dark.bg), `dark ${key}`).toBeGreaterThanOrEqual(4.5);
    }
  });
});
