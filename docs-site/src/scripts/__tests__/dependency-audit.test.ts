// @vitest-environment node
import { beforeAll, describe, expect, it } from 'vitest';
import { dirname, resolve } from 'node:path';
import { createRequire } from 'node:module';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import {
  evaluateAudit,
  inspectWorkerBundle,
  loadApplicabilityContext,
  scanProjectSources,
  validateApplicability,
  validatePublishedVersions,
} from '../../../scripts/audit-dependencies.mjs';

const siteRoot = resolve(dirname(fileURLToPath(import.meta.url)), '../../..');

it('the website and Miniflare actually load the patched Sharp and librsvg binaries', async () => {
  const require = createRequire(resolve(siteRoot, 'package.json'));
  const direct = require('sharp');
  const miniflareRequire = createRequire(require.resolve('miniflare'));
  const indirect = miniflareRequire('sharp');
  for (const sharp of [direct, indirect]) {
    expect(sharp.versions.sharp).toBe('0.35.5');
    expect(sharp.versions.rsvg).toBe('2.63.2');
    // Exercise the actual native SVG decoder used by the build/preview tools.
    const rendered = await sharp(Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2" fill="#ff0000"/></svg>'))
      .removeAlpha().raw().toBuffer({ resolveWithObject: true });
    expect(rendered.info.width).toBe(2);
    expect(rendered.info.height).toBe(2);
    expect([...rendered.data]).toEqual([255, 0, 0, 255, 0, 0, 255, 0, 0, 255, 0, 0]);
  }
});
const now = new Date('2026-10-03T12:00:00Z');
const advisory = {
  source: 1240991,
  title: 'http-cache-semantics max-stale handling can disclose cross-user cached responses',
  name: 'http-cache-semantics', dependency: 'http-cache-semantics',
  url: 'https://github.com/advisories/GHSA-ch52-4w7c-c8xp',
  severity: 'high', range: '<=4.2.0',
};
const report = () => ({
  auditReportVersion: 2,
  metadata: { vulnerabilities: { info: 0, low: 0, moderate: 0, high: 3, critical: 0, total: 3 } },
  vulnerabilities: {
    'http-cache-semantics': { name: 'http-cache-semantics', severity: 'high', via: [{ ...advisory }] },
    astro: { name: 'astro', severity: 'high', via: ['http-cache-semantics'] },
    '@astrojs/starlight': { name: '@astrojs/starlight', severity: 'high', via: ['astro'] },
  },
});

function runGuardWithCleanAudit(versions: string[], options: { symlink?: boolean; preserveSymlinksMain?: boolean } = {}) {
  const directory = mkdtempSync(resolve(tmpdir(), 'froglet-clean-audit-'));
  const calls = resolve(directory, 'calls.jsonl');
  try {
    const clean = { auditReportVersion: 2, vulnerabilities: {}, metadata: { vulnerabilities: { high: 0, critical: 0 } } };
    const shim = String.raw`#!${process.execPath}
const fs = require('node:fs');
const args = process.argv.slice(2);
fs.appendFileSync(${JSON.stringify(calls)}, JSON.stringify(args) + '\n');
if (JSON.stringify(args) === JSON.stringify(['audit', '--json'])) console.log(${JSON.stringify(JSON.stringify(clean))});
else if (JSON.stringify(args) === JSON.stringify(['view', 'http-cache-semantics', 'versions', '--json', '--registry=https://registry.npmjs.org'])) console.log(${JSON.stringify(JSON.stringify(versions))});
else process.exitCode = 99;
`;
    writeFileSync(resolve(directory, 'npm'), shim, { mode: 0o700 });
    let entrypoint = resolve(siteRoot, 'scripts/audit-dependencies.mjs');
    if (options.symlink) {
      const link = resolve(directory, 'audit-dependencies.mjs');
      symlinkSync(entrypoint, link);
      entrypoint = link;
    }
    const args = options.preserveSymlinksMain ? ['--preserve-symlinks-main', entrypoint] : [entrypoint];
    const result = spawnSync(process.execPath, args, {
      cwd: siteRoot, encoding: 'utf8', timeout: 45_000,
      env: { ...process.env, PATH: `${directory}:${process.env.PATH ?? ''}` },
    });
    return { result, calls: readFileSync(calls, 'utf8').trim().split('\n').map((line) => JSON.parse(line)) };
  } finally { rmSync(directory, { recursive: true, force: true }); }
}

describe('docs dependency audit policy', () => {
  it('allows only the exact reviewed advisory and its known dependency effects', () => {
    expect(evaluateAudit(report()).exceptedPackages).toEqual(['@astrojs/starlight', 'astro', 'http-cache-semantics']);
  });

  it('accepts a clean audit without claiming an exception was needed', () => {
    expect(evaluateAudit({ auditReportVersion: 2, vulnerabilities: {}, metadata: { vulnerabilities: { high: 0, critical: 0 } } }).exceptedPackages).toEqual([]);
  });

  it.each(['high', 'critical'])('fails a different %s advisory even alongside the allowed one', (severity) => {
    const audit: any = report();
    audit.vulnerabilities.astro.via.push({ ...advisory, name: 'astro', dependency: 'astro', severity, url: 'https://github.com/advisories/GHSA-other' });
    expect(() => evaluateAudit(audit)).toThrow(/not covered|Invalid npm audit/);
  });

  it.each(['url', 'name', 'dependency', 'range', 'severity', 'source', 'title'])('invalidates changed advisory %s', (field) => {
    const audit: any = report();
    audit.vulnerabilities['http-cache-semantics'].via[0][field] = field === 'severity' ? 'critical' : 'different';
    expect(() => evaluateAudit(audit)).toThrow(/not covered|Invalid npm audit/);
  });

  it('fails an unreviewed affected package even when it points to the same advisory', () => {
    const audit: any = report();
    audit.vulnerabilities.other = { name: 'other', severity: 'high', via: ['http-cache-semantics'] };
    expect(() => evaluateAudit(audit)).toThrow(/not covered|Invalid npm audit/);
  });

  it.each([
    { error: { code: 'ENETUNREACH' } },
    { auditReportVersion: 2, vulnerabilities: {} },
    { auditReportVersion: 1, vulnerabilities: {}, metadata: { vulnerabilities: {} } },
  ])('fails malformed audit or network-error data', (audit) => {
    expect(() => evaluateAudit(audit)).toThrow(/Invalid npm audit/);
  });

  it('fails missing or cyclic dependency findings', () => {
    const audit: any = report();
    delete audit.vulnerabilities['http-cache-semantics'];
    expect(() => evaluateAudit(audit)).toThrow(/not covered|Invalid npm audit/);
    audit.vulnerabilities.astro.via = ['@astrojs/starlight'];
    expect(() => evaluateAudit(audit)).toThrow(/not covered/);
  });

  it.each(['critical ', 'unknown', 'toString', 'constructor', '__proto__', ['high'], ['critical'], undefined])('fails unrecognized audit severity %s', (severity) => {
    const audit: any = report();
    audit.vulnerabilities['http-cache-semantics'].severity = severity;
    expect(() => evaluateAudit(audit)).toThrow(/Invalid npm audit finding/);
  });

  it.each(['constructor', '__proto__', ['high'], ['critical']])('rejects malformed underlying advisory severity %s', (severity) => {
    const audit: any = report();
    audit.vulnerabilities['http-cache-semantics'].via[0].severity = severity;
    expect(() => evaluateAudit(audit)).toThrow(/Invalid npm audit dependency severity/);
  });

  it('fails truncated severity counts instead of passing a seemingly clean report', () => {
    const audit: any = report();
    audit.vulnerabilities = {};
    expect(() => evaluateAudit(audit)).toThrow(/severity counts/);
  });

  it('fails a high aggregate without any high underlying advisory', () => {
    const audit: any = report();
    audit.vulnerabilities['http-cache-semantics'].via[0].severity = 'moderate';
    expect(() => evaluateAudit(audit)).toThrow(/aggregate severity/);
  });

  it('does not broaden the previous high threshold to reject moderate-only advisories', () => {
    const audit: any = report();
    audit.vulnerabilities.other = { name: 'other', severity: 'moderate', via: [{ ...advisory, url: 'other', severity: 'moderate' }] };
    expect(evaluateAudit(audit).exceptedPackages).toHaveLength(3);
  });

  it('preserves the high threshold for a moderate advisory inside an already-high package', () => {
    const audit: any = report();
    audit.vulnerabilities.astro.via.push({ ...advisory, name: 'astro', dependency: 'astro', url: 'https://example.test/moderate', severity: 'moderate' });
    expect(evaluateAudit(audit).exceptedPackages).toHaveLength(3);
  });

  it('requires a review as soon as a stable version newer than reviewed 4.3.0 is published', () => {
    expect(() => validatePublishedVersions(['4.1.1', '4.2.0', '4.3.0'])).not.toThrow();
    for (const newer of ['4.3.1', '4.3.1+build.1', '4.4.0', '5.0.0+build.1']) {
      expect(() => validatePublishedVersions(['4.3.0', newer])).toThrow(/newer published/);
    }
    expect(() => validatePublishedVersions(['4.3.0', 'garbage'])).toThrow(/Invalid npm registry/);
    expect(() => validatePublishedVersions(['4.3.0', '4.3.1-01'])).toThrow(/Invalid npm registry/);
    expect(() => validatePublishedVersions(['4.2.0'])).toThrow(/Invalid npm registry/);
    expect(() => validatePublishedVersions({ error: 'offline' })).toThrow(/Invalid npm registry/);
  });
});

describe('production source inventory', () => {
  it('includes executable JS/TS variants and rejects source symlinks', () => {
    const directory = mkdtempSync(resolve(tmpdir(), 'froglet-audit-inventory-'));
    try {
      mkdirSync(resolve(directory, 'src'));
      for (const extension of ['js', 'jsx', 'mjs', 'cjs', 'ts', 'tsx', 'mts', 'cts', 'astro', 'mdx']) {
        writeFileSync(resolve(directory, `src/example.${extension}`), 'import "astro:assets";');
      }
      expect(Object.keys(scanProjectSources(directory))).toHaveLength(10);
      symlinkSync(resolve(directory, 'src/example.ts'), resolve(directory, 'src/linked.ts'));
      expect(() => scanProjectSources(directory)).toThrow(/symlink/);
    } finally { rmSync(directory, { recursive: true, force: true }); }
  });
});

describe('reviewed website applicability boundaries', () => {
  let baseline: any;
  beforeAll(async () => {
    baseline = { ...loadApplicabilityContext(siteRoot), bundle: await inspectWorkerBundle(siteRoot), now };
  }, 30_000);

  const copy = () => structuredClone(baseline);

  it('a clean npm audit still runs the real registry and deployment-bundle review', () => {
    const { result, calls } = runGuardWithCleanAudit(['4.2.0', '4.3.0']);
    expect(result.error).toBeUndefined();
    expect(result.status, result.stderr || result.stdout).toBe(0);
    expect(calls).toEqual([
      ['audit', '--json'],
      ['view', 'http-cache-semantics', 'versions', '--json', '--registry=https://registry.npmjs.org'],
    ]);
    expect(result.stdout).toContain('Raw npm audit: high=0, critical=0.');
    expect(result.stdout).toContain('Known max-stale/shared-cookie behavior is unchanged');
    expect(result.stdout).toContain('even with a clean raw audit');
    expect(result.stdout).toContain(`Actual dry-run Worker SHA-256: ${createHash('sha256').update(baseline.bundle.code).digest('hex')}`);
  }, 45_000);

  it('the real guard rejects a newer stable version even when npm audit is clean', () => {
    const { result } = runGuardWithCleanAudit(['4.3.0', '4.3.1']);
    expect(result.error).toBeUndefined();
    expect(result.status).toBe(1);
    expect(result.stderr).toContain('A newer published http-cache-semantics version requires review');
    expect(result.stdout).not.toContain('Actual dry-run Worker SHA-256:');
  }, 45_000);

  it.each([false, true])('executes the guard through a symlink (preserve-symlinks-main=%s)', (preserveSymlinksMain) => {
    const { result, calls } = runGuardWithCleanAudit(['4.3.0', '4.3.1'], { symlink: true, preserveSymlinksMain });
    expect(result.error).toBeUndefined();
    expect(result.status).toBe(1);
    expect(result.stderr).toContain('A newer published http-cache-semantics version requires review');
    expect(calls).toEqual([
      ['audit', '--json'],
      ['view', 'http-cache-semantics', 'versions', '--json', '--registry=https://registry.npmjs.org'],
    ]);
  }, 45_000);

  it('can be imported from Node stdin without treating the import as a CLI invocation', () => {
    const result = spawnSync(process.execPath, ['--input-type=module', '-'], {
      cwd: siteRoot, encoding: 'utf8', timeout: 10_000,
      input: `import ${JSON.stringify(new URL('../../../scripts/audit-dependencies.mjs', import.meta.url).href)}; console.log('Imported guard without invoking it.');`,
    });
    expect(result.error).toBeUndefined();
    expect(result.status, result.stderr).toBe(0);
    expect(result.stdout.trim()).toBe('Imported guard without invoking it.');
  });

  it('checks an actual fresh Wrangler bundle against the reviewed source and static configuration', () => {
    expect(() => validateApplicability(baseline)).not.toThrow();
    expect(baseline.bundle.sources).not.toContain('node_modules/astro/dist/assets/build/remote.js');
  });

  it.each(['astro', 'http-cache-semantics', 'wrangler'])('invalidates changed %s version', (name) => {
    const context = copy();
    context.versions[name] = '999.0.0';
    expect(() => validateApplicability(context)).toThrow(/version/);
  });

  it('expires at the beginning of November 16 UTC', () => {
    const context = copy();
    context.now = new Date('2026-11-16T00:00:00Z');
    expect(() => validateApplicability(context)).toThrow(/expired/);
  });

  it.each(['astroConfig', 'wranglerConfig', 'remoteSource'])('invalidates reviewed %s changes', (field) => {
    const context = copy();
    context[field] += '\n// changed';
    expect(() => validateApplicability(context)).toThrow(/changed/);
  });

  it.each(['dependencies', 'optionalDependencies', 'peerDependencies', 'devDependencies'])('invalidates a new %s consumer', (field) => {
    const context = copy();
    context.lock.packages['node_modules/other'] = { version: '1.0.0', [field]: { 'http-cache-semantics': '^4.2.0' } };
    expect(() => validateApplicability(context)).toThrow(/consumer/);
  });

  it('rejects npm aliases and extra declarations on an existing consumer', () => {
    const context = copy();
    context.lock.packages[''].dependencies.other = 'npm:http-cache-semantics@4.2.0';
    expect(() => validateApplicability(context)).toThrow(/consumer/);
    delete context.lock.packages[''].dependencies.other;
    context.lock.packages['node_modules/astro'].peerDependencies['http-cache-semantics'] = '^4.2.0';
    expect(() => validateApplicability(context)).toThrow(/consumer/);
  });

  it('rejects new project asset imports even outside the Worker source graph', () => {
    const context = copy();
    context.projectSources['src/pages/private.astro'] = 'import { Image } from "astro:assets";';
    expect(() => validateApplicability(context)).toThrow(/import/);
  });

  it.each([
    'import /*comment*/ "http-cache-semantics";',
    'import { foo } from "http-cache-semantics/index.js";',
    'import(`astro:assets`);',
    'import "http\\u002dcache-semantics";',
    'import "astro\\x3aassets";',
    'import "astro\\:assets";',
    'import "http\\-cache-semantics";',
    'require("as\\164ro:assets");',
    'import "astro:\\\nassets";',
    'import "astro:\\\r\nassets";',
    'import "astro:\\' + String.fromCharCode(0x2028) + 'assets";',
    'import "astro:\\' + String.fromCharCode(0x2029) + 'assets";',
    'const a="/*"; import "astro:assets"; const b="*/";',
  ])('rejects disguised/deep/template imports: %s', (source) => {
    const context = copy();
    context.projectSources['src/pages/private.astro'] = source;
    expect(() => validateApplicability(context)).toThrow(/import/);
  });

  it('rejects production references into excluded test modules', () => {
    const context = copy();
    context.projectSources['src/hidden.tsx'] = 'import "./__tests__/cache.ts";';
    expect(() => validateApplicability(context)).toThrow(/excluded test module/);
    delete context.projectSources['src/hidden.tsx'];
    context.projectSources['src/data/maturity.ts'] += '\nimport "../__tests__/cache.ts";';
    expect(() => validateApplicability(context)).toThrow(/excluded test module/);
  });

  it('invalidates reviewed Worker source changes', () => {
    const context = copy();
    context.reviewedSources['src/worker.ts'] += '\n// changed';
    expect(() => validateApplicability(context)).toThrow(/Worker source/);
  });

  it('rejects an additional bundled package even when the output hash is unchanged', () => {
    const context = copy();
    context.bundle.sources.push('node_modules/astro/dist/assets/build/remote.js');
    expect(() => validateApplicability(context)).toThrow(/bundle sources/);
  });

  it('rejects altered bundle bytes and missing source-map proof', () => {
    const context = copy();
    context.bundle.code += '\n// changed';
    expect(() => validateApplicability(context)).toThrow(/bundle changed/);
    context.bundle = baseline.bundle;
    context.bundle = { ...context.bundle, sourcesMatch: false };
    expect(() => validateApplicability(context)).toThrow(/source map/);
  });

  it('does not pin the generated Wasm content hash as JavaScript cache behavior', () => {
    const context = copy();
    context.bundle.code = context.bundle.code.replace(/\.\/[a-f0-9]{40}-froglet_verify_bg\.wasm/, './0000000000000000000000000000000000000000-froglet_verify_bg.wasm');
    expect(() => validateApplicability(context)).not.toThrow();
  });
});
