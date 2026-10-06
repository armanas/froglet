import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { MATURITY } from '../../data/maturity';
import { brokenInternalLinks, docsSite, repoRoot, src } from './route-helpers';

const read = (path: string) => readFileSync(resolve(src, path), 'utf8');
const repo = (path: string) => readFileSync(resolve(repoRoot, path), 'utf8');
const page = read('pages/open-source.astro');
const css = read('styles/developers.css');
const cssRules = css.replace(/\/\*[\s\S]*?\*\//g, '');
const terminal = read('components/Terminal.astro');
const fixture = JSON.parse(repo('conformance/kernel_v1.json'));
/** Documents wrap their lines, so quoted sentences are compared with whitespace collapsed. */
const flat = (text: string) => text.replace(/\s+/g, ' ');

/** The entries of a `const name = [ ... ];` array in the page's frontmatter, one per `  { name:` line. */
function entriesOf(name: string): string[] {
  const block = page.match(new RegExp(`const ${name} = \\[([\\s\\S]*?)\\n\\];`))?.[1];
  if (!block) throw new Error(`No ${name} array in the page`);
  return block.split('\n').filter((line) => /^\s+\{ name: /.test(line));
}

describe('Developers page structure', () => {
  it('is built on the shared story layout with its own stylesheet and the agent metadata', () => {
    expect(page).toContain("import StoryLayout from '../layouts/StoryLayout.astro'");
    expect(page).toContain("import '../styles/developers.css'");
    expect(page).toContain("import Terminal from '../components/Terminal.astro'");
    expect(page).toContain('<AgentMeta slot="head" route="openSource" />');
    expect(page).not.toMatch(/<style[\s>]/);
    expect(page).not.toMatch(/style="/);
  });

  it('has one heading, and every jump link points at a section that exists', () => {
    expect(page.match(/<h1[\s>]/g)).toHaveLength(1);
    const targets = Array.from(page.matchAll(/href="#([a-z-]+)"/g)).map((match) => match[1]);
    expect(targets.length).toBeGreaterThanOrEqual(6);
    for (const id of new Set(targets)) expect(page, `#${id} has no target`).toContain(`id="${id}"`);
    // Other pages link to #integrations, so the status section keeps that id.
    for (const id of ['verify', 'playground', 'vision', 'tools', 'examples', 'integrations', 'build']) expect(page).toContain(`id="${id}"`);
  });

  it('keeps the statements other tests and readers rely on', () => {
    expect(page).toContain('runs supplied Wasm programs and a synthetic catalog without installation or an invitation');
    expect(page).toContain('current batch and GPU constraints');
    expect(page).toContain('href="/learn/cloud-trial/"');
    expect(page).toContain('Tor is an advanced self-hosted path');
    expect(page).toContain('Confidential execution and selective disclosure remain specifications');
    expect(page).toContain('Staked identity is designed, not deployed');
    expect(page).toContain('RAILS.map');
    expect(page).not.toMatch(/T4 verified|GPU is live|production[- ]ready|enterprise[- ]grade/i);
  });

  it('builds every maturity label from the maturity data', () => {
    expect(page).toMatch(/import \{[^}]*MATURITY[^}]*\} from '\.\.\/data\/maturity'/);
    expect(page).not.toMatch(/data-status="(spec|prototype|beta|production)"/);
    const ids = [
      ...Array.from(page.matchAll(/pill\('([a-z0-9-]+)'\)/g)).map((match) => match[1]),
      ...Array.from(page.matchAll(/maturity: \[([^\]]*)\]/g)).flatMap((match) => Array.from(match[1].matchAll(/'([a-z0-9-]+)'/g)).map((id) => id[1])),
    ];
    expect(ids.length).toBeGreaterThan(3);
    const known = new Set(MATURITY.map((entry) => entry.id));
    expect(ids.filter((id) => !known.has(id))).toEqual([]);
  });

  it('resolves every internal link, including anchors, to something that exists', () => {
    expect(page.match(/href="\/|href: '\//g)?.length).toBeGreaterThan(5);
    expect(brokenInternalLinks(page)).toEqual([]);
  });

  it('points every GitHub link at a file or folder that exists in the repository', () => {
    const paths = [
      ...Array.from(page.matchAll(/\b(?:blob|tree)\('([^']+)'\)/g)).map((match) => match[1]),
      ...Array.from(page.matchAll(/blob\(`docs\/\$\{file\}`\)/g)).length ? Array.from(page.matchAll(/\['([A-Z_]+\.md)', /g)).map((match) => `docs/${match[1]}`) : [],
    ];
    expect(paths.length).toBeGreaterThan(12);
    for (const path of new Set(paths)) expect(existsSync(resolve(repoRoot, path)), path).toBe(true);
  });
});

describe('Developers page leaves documentation to the docs', () => {
  it('links to the guides for setup, agents, publishing, the specification and the walkthrough', () => {
    for (const route of ['/learn/quickstart/', '/learn/agents/', '/learn/provider-onboarding/', '/learn/plugin-distribution/', '/spec/conformance/', '/spec/kernel/', '/spec/service-binding/', '/demo/', '/docs/']) {
      // A link in the page's data (single quotes) or its markup (double quotes), with or without an anchor.
      expect(page.includes(`'${route}'`) || page.includes(`"${route}"`) || page.includes(`"${route}#`), route).toBe(true);
    }
  });

  it('does not repeat how-to that the docs carry', () => {
    for (const text of ['setup-agent.sh', 'strict_checks.sh', 'cargo clippy', 'npx froglet-mcp', 'OperatorSetup', 'Install a release instead', 'Where things live in the repository', 'How one artifact is signed', 'A conforming runner must']) {
      expect(page, text).not.toContain(text);
    }
    expect(page).not.toMatch(/<h2[^>]*>[^<]*(Contribute|Implement it yourself)/);
  });

  it('gives every tool row a way to try it: one command from the README, or a note', () => {
    const tools = entriesOf('tools');
    expect(tools.length).toBe(5);
    for (const entry of tools) expect(entry, entry.slice(0, 60)).toMatch(/command: commands\.\w+|note: '/);
    const commandsUsed = tools.flatMap((entry) => Array.from(entry.matchAll(/command: commands\.(\w+)/g)).map((match) => match[1]));
    expect(commandsUsed.sort()).toEqual(['node', 'vectorsRust', 'verifyPython']);
  });

  it('shows the five integration cards, including the optional A2A adapter', () => {
    const cards = entriesOf('integrations');
    expect(cards).toHaveLength(5);
    expect(cards.some((entry) => entry.includes("name: 'A2A'"))).toBe(true);
    for (const entry of cards) expect(entry, entry.slice(0, 40)).toMatch(/links: \[\['[^']+', (?:'\/|blob\(|tree\()/);
  });

  it('says what runs in a browser today, and the repository compiles only the verifier and the signing kernel for one', () => {
    expect(page).toContain('In a browser, you can verify signed records and run the playground above: a provider and a consumer for free deals that live in your tab.');
    const scripts = readdirSync(resolve(docsSite, 'scripts'), { withFileTypes: true }).filter((entry) => entry.isFile());
    const compiledForTheBrowser = scripts.map((entry) => entry.name).filter((file) => readFileSync(resolve(docsSite, 'scripts', file), 'utf8').includes('wasm32-unknown-unknown'));
    expect(compiledForTheBrowser).toEqual(['build-playground.mjs', 'build-verifier.mjs']);
    expect(readFileSync(resolve(docsSite, 'scripts/build-verifier.mjs'), 'utf8')).toContain("'-p', 'froglet-verify'");
    expect(readFileSync(resolve(docsSite, 'scripts/build-playground.mjs'), 'utf8')).toContain("'-p', 'froglet-wasm'");
  });
});

describe('Developers page commands', () => {
  it('shows the verification commands the README documents, and the files they run exist', () => {
    const readme = repo('README.md');
    expect(page).toContain("verifyRust: 'cargo run -p froglet-verify -- conformance/kernel_v1.json'");
    expect(readme).toContain('cargo run -p froglet-verify -- conformance/kernel_v1.json');
    expect(page).toContain("vectorsRust: 'cargo test -p froglet --test kernel_conformance_vectors'");
    expect(repo('conformance/README.md')).toContain('cargo test -p froglet --test kernel_conformance_vectors');
    expect(existsSync(resolve(repoRoot, 'tests/kernel_conformance_vectors.rs'))).toBe(true);
    expect(page).toContain("verifyPython: 'PYTHONPATH=python/froglet-verify python3 -m froglet_verify.conformance conformance/kernel_v1.json'");
    expect(repo('python/froglet-verify/froglet_verify/conformance.py')).toContain('python -m froglet_verify.conformance');
  });

  it('starts the node the way the README does, with the role and ports the configuration document states', () => {
    expect(page).toContain("node: 'FROGLET_NODE_ROLE=dual cargo run -p froglet --bin froglet-node'");
    expect(repo('README.md')).toContain('cargo run -p froglet --bin froglet-node');
    expect(repo('docs/CONFIGURATION.md')).toMatch(/`FROGLET_NODE_ROLE` \| `provider` \| Node role: `provider`, `runtime`, or `dual`/);
    expect(page).toContain('The role defaults to provider');
    expect(repo('docs/CONFIGURATION.md')).toContain('`127.0.0.1:8080`');
    expect(repo('docs/CONFIGURATION.md')).toContain('`127.0.0.1:8081`');
    expect(repo('docs/openapi.yaml')).toMatch(/^  \/v1\/feed:/m);
  });

  it('pipes the node’s own feed straight into the verifier, with no other tool, as the README does', () => {
    const feed = page.match(/feed: `([^`]*)`/)?.[1] ?? '';
    const pipeline = "/v1/feed?limit=50' | cargo run -q -p froglet-verify -- -";
    expect(feed).toContain(pipeline);
    expect(repo('README.md')).toContain(pipeline);
    expect(feed).not.toContain('jq');
    expect(page).not.toContain('<code>jq</code>');
    expect(page).toContain('the verdict covers the page you fetched');
  });

  it('can do that because the node serves entries that carry the artifact under document, and the verifier reads them', () => {
    // The node side: /v1/feed is served by get_feed, whose items are LedgerArtifact entries with a `document`.
    expect(repo('src/api/mod.rs')).toContain('pub async fn get_feed(');
    expect(repo('src/db.rs')).toMatch(/pub struct LedgerArtifact \{[^}]*pub document: serde_json::Value/);
    // The verifier side is tested on a page a real node served, in Rust (library and CLI) and in Python.
    const served = JSON.parse(repo('froglet-verify/tests/fixtures/node_feed_page.json'));
    expect(served.artifacts.length).toBeGreaterThan(0);
    for (const entry of served.artifacts) {
      expect(entry.artifact_type, 'an entry is not itself an artifact').toBeUndefined();
      expect(entry.document.artifact_type).toBe(entry.kind);
    }
    expect(repo('froglet-verify/tests/feed_page.rs')).toContain('fn cli_verifies_the_served_page_from_stdin');
    expect(repo('python/froglet-verify/tests/test_cli.py')).toContain('class NodeFeedPageCliTests');
    // And the node's own suite feeds its live route to the verifier, so the two cannot drift apart.
    expect(repo('src/api/mod.rs')).toContain('async fn public_feed_page_verifies_offline_with_froglet_verify');
  });

  it('lists only author commands that the node’s own help prints', () => {
    const help = repo('src/bin/froglet-node.rs');
    const commands = Array.from(page.matchAll(/^\s+\['(froglet-node [^']+)', '/gm)).map((match) => match[1]);
    expect(commands.length).toBeGreaterThanOrEqual(8);
    for (const command of commands) {
      const base = command.split(/ [<\[-]/)[0];
      expect(help, base).toContain(base);
    }
  });
});

describe('Developers page examples', () => {
  it('builds the sample output from the frozen vectors, in the format the verifier prints', () => {
    for (const name of ['descriptor', 'offer', 'quote', 'deal', 'invoice_bundle', 'receipt']) {
      expect(fixture.artifacts[name].artifact_hash, name).toMatch(/^[0-9a-f]{64}$/);
    }
    expect(page).toContain("readFileSync(resolve(process.cwd(), '../conformance/kernel_v1.json'), 'utf8')");
    expect(page).toContain("`[ok ] ${String(artifact(name).artifact_type).padEnd(14)} ${String(fixture.artifacts[name].artifact_hash).slice(0, 12)}  envelope + semantics verified`");
    const cli = repo('froglet-verify/src/bin/froglet-verify.rs');
    expect(cli).toContain('"[{}] {:<14} {}  {}"');
    expect(cli).toContain('"ok "');
    expect(cli).toContain('"envelope + semantics verified"');
    expect(cli).toContain('take(12)');
    expect(cli).toContain('"chain {label}: ok"');
    expect(cli).toContain('"result: {}"');
    expect(cli).toContain('"VALID"');
  });

  it('prints chain paths that exist, in the order the verifier reports them for a paid chain', () => {
    const chain = repo('froglet-protocol/src/protocol/chain.rs');
    for (const variant of ['DescriptorOffer', 'OfferQuote', 'QuoteInvoiceBundleDeal', 'QuoteDealReceipt']) expect(chain, variant).toContain(variant);
    for (const label of ['descriptor_offer', 'offer_quote', 'quote_invoice_bundle_deal', 'quote_deal_receipt']) expect(page, label).toContain(`chain ${label}: ok`);
    expect(fixture.conformance_path.artifact_order).toContain('invoice_bundle');
  });

  it('builds the feed output from the captured page a real node served, in the same format', () => {
    expect(page).toContain("readFileSync(resolve(process.cwd(), '../froglet-verify/tests/fixtures/node_feed_page.json'), 'utf8')");
    expect(page).toContain("`[ok ] ${String(entry.document.artifact_type).padEnd(14)} ${String(entry.document.hash).slice(0, 12)}  envelope + semantics verified`");
    // A public feed is not a whole deal, so the verifier reports each artifact and claims no chain.
    expect(page).toMatch(/const feedOutput = \[\s*\.\.\.feedPage\.artifacts\.map\([^\n]+\n\s*'result: VALID',\s*\]/);
    const served = JSON.parse(repo('froglet-verify/tests/fixtures/node_feed_page.json'));
    expect(served.artifacts.map((entry: { kind: string }) => entry.kind)).toEqual(['descriptor', 'offer', 'offer', 'offer']);
    expect(page).toContain('The output is from a page a real node served, kept as a test fixture, so your hashes will differ.');
  });

  it('shows a real signed offer from the vectors', () => {
    expect(page).toContain("const exampleOffer = JSON.stringify(artifact('free_offer'), null, 2);");
    expect(fixture.artifacts.free_offer.artifact.artifact_type).toBe('offer');
  });

  it('gives the Wasm host contract that the service binding specifies', () => {
    const binding = repo('docs/SERVICE_BINDING.md');
    expect(binding).toContain('`memory` — 32-bit, unshared Wasm memory');
    expect(binding).toContain('`alloc(len: i32) -> i32`');
    expect(binding).toContain('`run(ptr: i32, len: i32) -> i64` — high 32 bits are result pointer, low');
    expect(binding).toContain('permits no imports or requested capabilities');
    expect(page).toContain('WebAssembly.instantiate(moduleBytes, {})');
  });
});

describe('Developers page claims match the repository', () => {
  it('names the toolchain, licence and platforms the repository states', () => {
    expect(repo('rust-toolchain.toml')).toContain('channel = "1.91.0"');
    // Both places the page names the toolchain must agree with it.
    expect(page).toContain('<li>Rust 1.91</li>');
    expect(page).toContain('You need Rust 1.91');
    expect(repo('LICENSE')).toContain('Apache License');
    expect(page).toContain('Apache-2.0');
    expect(repo('README.md')).toContain('Node.js `22.14.0` or newer');
    expect(page).toContain('Node.js 22.14 or newer');
    expect(repo('README.md')).toContain('This bridge needs no Node.js runtime.');
    expect(page).toContain('The native bridge needs no Node.js');
  });

  it('states the stability promise and the lack of certification', () => {
    expect(repo('docs/VERSIONING.md')).toContain('**Signed `froglet/v1` artifacts verify forever.**');
    expect(page).toContain('Signed froglet/v1 artifacts verify forever.');
    expect(flat(repo('conformance/README.md'))).toContain('There is no certification process beyond reproducing them.');
    expect(page).toContain('There is no certification beyond reproducing them.');
  });

  it('is honest that only the Python verifier is independent, and that it is verify-only and alpha', () => {
    const readme = repo('README.md');
    expect(flat(readme)).toContain('is independently implemented and checks the same public vectors');
    expect(flat(readme)).toContain('they do not provide implementation diversity');
    expect(repo('python/froglet-verify/pyproject.toml')).toContain('Development Status :: 3 - Alpha');
    expect(page).toContain('The Python verifier is the only independent implementation so far, and it is verify-only.');
    expect(page).toContain('Marked alpha in its package metadata.');
  });

  it('describes the contribution terms and the security policy accurately', () => {
    const contributing = repo('CONTRIBUTING.md');
    expect(flat(contributing)).toContain('does not currently require a separate CLA');
    expect(flat(contributing)).toContain('Developer Certificate of Origin (DCO)');
    expect(contributing).toContain('git commit -s');
    expect(page).toContain('Sign off every commit with git commit -s.');
    expect(page).toContain('There is a Developer Certificate of Origin, and no CLA.');
    expect(repo('SECURITY.md')).toContain('Only the latest release on the `main` branch is actively supported');
    expect(page).toContain('Only the latest release receives fixes.');
    expect(repo('conformance/README.md')).toContain('kernel_v1.json');
    expect(page).toContain('must update conformance/kernel_v1.json and say why');
  });

  it('says the browser build shares the Rust implementation and adds no independence', () => {
    expect(flat(repo('README.md'))).toContain('The Rust facade, browser WASM build, and in-repo Rust conformance runners share');
    expect(page).toContain('No, the same code');
    const entry = MATURITY.find((item) => item.id === 'verifier-wasm');
    expect(entry?.note).toContain('not a second implementation');
    expect(existsSync(resolve(repoRoot, entry!.evidence))).toBe(true);
  });
});

describe('Terminal component and stylesheet', () => {
  it('copies exactly the text it shows', () => {
    expect(terminal).toContain('data-copy={code}');
    expect(terminal).toContain('<code>{code}</code>');
    expect(terminal).toContain('<pre tabindex="0">');
  });

  it('marks output blocks as not copyable and long examples as scrollable', () => {
    expect(page.match(/label="output" copy=\{false\}/g)).toHaveLength(2);
    expect(page).toContain('label="free_offer · conformance/kernel_v1.json" copy={false} scroll');
  });

  it('prefixes every rule with the story shell so it outranks its element styles', () => {
    const parts = (list: string) => {
      const out: string[] = [];
      let depth = 0;
      let current = '';
      for (const char of list) {
        if (char === '(') depth += 1;
        if (char === ')') depth -= 1;
        if (char === ',' && depth === 0) { out.push(current.trim()); current = ''; } else current += char;
      }
      return [...out, current.trim()];
    };
    const rules = Array.from(cssRules.matchAll(/(^|\})\s*([^{}@]+)\{/g)).map((match) => match[2].trim());
    const loose = rules.filter((selector) => parts(selector).some((part) => !/^(\[data-theme='light'\] )?\.story-main /.test(part)));
    expect(loose).toEqual([]);
  });

  it('keeps the inline-code style out of the terminals that sit inside tool rows', () => {
    // A rule on `.dev-tool code` would outrank the terminal's own reset and draw its command as a chip.
    expect(cssRules).not.toContain('.dev-tool code');
    expect(cssRules).toContain('.story-main .dev-tool p code');
    expect(cssRules).toMatch(/\.story-main \.dev-term code \{[^}]*background: none/);
  });

  it('styles only what the page uses', () => {
    const classes = new Set(Array.from(cssRules.matchAll(/\.(dev-[a-z0-9_-]+)/g)).map((match) => match[1]));
    // The page, the terminal component, and the script that renders the verifier's result.
    const rendered = page + terminal + read('scripts/dev-page.ts');
    expect(Array.from(classes).filter((name) => !rendered.includes(name))).toEqual([]);
  });

  it('stacks the two-column layouts before their code blocks are squeezed, and never hides table cells', () => {
    expect(css).toMatch(/@media \(max-width: 1120px\) \{\s*\.story-main \.dev-split \{ grid-template-columns: minmax\(0, 1fr\)/);
    expect(css).toMatch(/@media \(max-width: 1120px\) \{[^}]*\}\s*\.story-main \.dev-tool \{ grid-template-columns: minmax\(0, 1fr\)/);
    const rules = Array.from(cssRules.matchAll(/([^{}]*dev-table[^{}]*)\{([^}]*)\}/g));
    for (const [, selector, body] of rules) expect(body, selector.trim()).not.toMatch(/display\s*:\s*none/);
  });
});
