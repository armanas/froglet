import { describe, expect, it } from 'vitest';
import { GET, getStaticPaths } from '../../pages/[journey]/agent.md';
import { readFileSync } from 'node:fs';

describe('plain-text agent entrypoints', () => {
  for (const route of getStaticPaths()) {
    it(`serves the complete ${route.params.journey} guide with resolvable links`, async () => {
      const response = await GET({ props: route.props } as Parameters<typeof GET>[0]);
      const body = await response.text();
      expect(response.headers.get('content-type')).toBe('text/markdown; charset=utf-8');
      expect(body).toMatch(/^# /);
      expect(body).not.toMatch(/^---/);
      expect(body).not.toMatch(/\]\(\//);
      // Preserve executable code verbatim, rather than a lossy summary.
      const blocks = route.props.guide.match(/```[\s\S]*?```/g) ?? [];
      expect(blocks.length).toBeGreaterThan(0);
      for (const block of blocks) expect(body).toContain(block);
    });
  }

  it('starts publication from the native machine guide, not the hosted trial', () => {
    const manifest = JSON.parse(readFileSync('public/agent-tasks.json', 'utf8'));
    const task = manifest.task_contracts.find((item: { task_id: string }) => item.task_id === manifest.default_task_id);
    expect(task.entrypoint).toBe('https://froglet.dev/publish/agent.md');
    const page = readFileSync('src/pages/publish.astro', 'utf8');
    expect(page).toContain("new URL('/publish/agent.md', siteOrigin)");
    const homepage = readFileSync('src/pages/index.astro', 'utf8');
    expect(homepage).toContain('${siteOrigin}/publish/agent.md');
  });
});
