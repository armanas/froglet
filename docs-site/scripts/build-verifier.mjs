import { readFileSync, mkdirSync, writeFileSync, existsSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

const site = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const repo = resolve(site, '..');
const target = resolve(repo, process.env.CARGO_TARGET_DIR || 'target');
const version = readFileSync(resolve(repo, 'Cargo.lock'), 'utf8').match(/name = "wasm-bindgen"\nversion = "([^"]+)"/)[1];
const toolRoot = resolve(target, 'verifier-tools');
const bindgen = process.env.WASM_BINDGEN || resolve(toolRoot, 'bin/wasm-bindgen');
const run = (cmd, args) => execFileSync(cmd, args, { cwd: repo, stdio: 'inherit' });
if (!existsSync(bindgen)) run('cargo', ['install', 'wasm-bindgen-cli', '--version', version, '--locked', '--root', toolRoot]);
if (execFileSync(bindgen, ['--version'], { encoding: 'utf8' }).trim() !== `wasm-bindgen ${version}`) {
  throw new Error(`wasm-bindgen must match Cargo.lock (${version})`);
}
run('cargo', ['build', '--locked', '--release', '-p', 'froglet-verify', '--lib', '--target', 'wasm32-unknown-unknown', '--target-dir', target]);
const output = resolve(site, 'src/generated/verifier');
mkdirSync(output, { recursive: true });
run(bindgen, [resolve(target, 'wasm32-unknown-unknown/release/froglet_verify.wasm'), '--target', 'web', '--out-dir', output]);
const fixture = JSON.parse(readFileSync(resolve(repo, 'conformance/kernel_v1.json'), 'utf8'));
const artifacts = ['descriptor', 'free_offer', 'free_quote', 'free_deal', 'free_receipt'].map(name => fixture.artifacts[name].artifact);
writeFileSync(resolve(output, 'free-chain.json'), JSON.stringify({ artifacts }));
