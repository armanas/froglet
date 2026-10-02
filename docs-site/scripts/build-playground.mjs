import { copyFileSync, mkdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { build } from 'esbuild';
import { bindgen, run, site, target } from './wasm-tools.mjs';

const generator = bindgen();

// The signing half of the kernel (froglet-wasm), with its browser bindings. The playground verifies with the verifier
// build, so this one stays small.
run('cargo', ['build', '--locked', '--release', '-p', 'froglet-wasm', '--lib', '--target', 'wasm32-unknown-unknown', '--target-dir', target]);
const kernel = resolve(site, 'src/generated/kernel');
mkdirSync(kernel, { recursive: true });
run(generator, [resolve(target, 'wasm32-unknown-unknown/release/froglet_wasm.wasm'), '--target', 'web', '--out-dir', kernel]);

// The Rust services that the editor's adder and Fibonacci functions are ports of. The page does not serve these builds: the
// tests run them as the answer that each port must give. examples/wasm-services is its own workspace, built into the same target directory.
run('cargo', ['build', '--locked', '--release', '--manifest-path', 'examples/wasm-services/Cargo.toml', '--target', 'wasm32-unknown-unknown', '--target-dir', target]);
const oracles = resolve(site, 'src/generated/playground');
mkdirSync(oracles, { recursive: true });
for (const name of ['adder', 'fibonacci']) copyFileSync(resolve(target, `wasm32-unknown-unknown/release/${name}.wasm`), resolve(oracles, `${name}.wasm`));

// The AssemblyScript compiler, bundled into one self-contained module. The page loads it on demand, in a Worker, from its own
// origin (see src/scripts/playground/compiler.ts), and compiles the source in the editor with it. The Node built-ins the
// compiler mentions are only reached when it runs under Node, so they stay external. Its licences are in public/licenses/.
const compiler = resolve(site, 'src/generated/assemblyscript');
mkdirSync(compiler, { recursive: true });
await build({
  entryPoints: [resolve(site, 'node_modules/assemblyscript/dist/asc.js')],
  outfile: resolve(compiler, 'asc.js'),
  bundle: true,
  minify: true,
  format: 'esm',
  platform: 'browser',
  external: ['node:*', 'fs', 'module', 'path', 'url'],
  logLevel: 'warning',
});
