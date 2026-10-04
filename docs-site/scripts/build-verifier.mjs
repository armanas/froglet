import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { bindgen, buildWasm, repo, run, site, target } from './wasm-tools.mjs';

const generator = bindgen();
buildWasm(['--locked', '--release', '-p', 'froglet-verify', '--lib', '--target', 'wasm32-unknown-unknown', '--target-dir', target]);
const output = resolve(site, 'src/generated/verifier');
mkdirSync(output, { recursive: true });
run(generator, [resolve(target, 'wasm32-unknown-unknown/release/froglet_verify.wasm'), '--target', 'web', '--out-dir', output]);
const fixture = JSON.parse(readFileSync(resolve(repo, 'conformance/kernel_v1.json'), 'utf8'));
const artifacts = ['descriptor', 'free_offer', 'free_quote', 'free_deal', 'free_receipt'].map(name => fixture.artifacts[name].artifact);
writeFileSync(resolve(output, 'free-chain.json'), JSON.stringify({ artifacts }));
