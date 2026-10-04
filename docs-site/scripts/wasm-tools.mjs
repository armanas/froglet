// Shared by the scripts that compile Rust to WebAssembly for the site: the repository paths, and the wasm-bindgen CLI
// pinned to the version in Cargo.lock.
import { readFileSync, existsSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

export const site = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const repo = resolve(site, '..');
export const target = resolve(repo, process.env.CARGO_TARGET_DIR || 'target');
export const run = (cmd, args) => execFileSync(cmd, args, { cwd: repo, stdio: 'inherit' });

/** Browser builds use explicit compiler flags: preserve Cargo's environment precedence, then remap the checkout root.
 * The encoded form keeps a checkout path containing spaces in one argument. Cargo config rustflags are intentionally
 * not merged; set custom browser flags through CARGO_ENCODED_RUSTFLAGS or RUSTFLAGS (see the site README).
 */
export function wasmBuildEnv(env = process.env) {
  const flags = env.CARGO_ENCODED_RUSTFLAGS !== undefined
    ? (env.CARGO_ENCODED_RUSTFLAGS === '' ? [] : env.CARGO_ENCODED_RUSTFLAGS.split('\x1f'))
    : (env.RUSTFLAGS || '').split(' ').map(flag => flag.trim()).filter(Boolean);
  return { ...env, CARGO_ENCODED_RUSTFLAGS: [...flags, `--remap-path-prefix=${repo}=/froglet`].join('\x1f') };
}

export const buildWasm = (args) => execFileSync('cargo', ['build', ...args], {
  cwd: repo, stdio: 'inherit', env: wasmBuildEnv(),
});

/** The wasm-bindgen CLI whose version matches Cargo.lock, installed into the Cargo target directory on first use. */
export function bindgen() {
  const version = readFileSync(resolve(repo, 'Cargo.lock'), 'utf8').match(/name = "wasm-bindgen"\nversion = "([^"]+)"/)[1];
  const toolRoot = resolve(target, 'verifier-tools');
  const path = process.env.WASM_BINDGEN || resolve(toolRoot, 'bin/wasm-bindgen');
  if (!existsSync(path)) run('cargo', ['install', 'wasm-bindgen-cli', '--version', version, '--locked', '--root', toolRoot]);
  if (execFileSync(path, ['--version'], { encoding: 'utf8' }).trim() !== `wasm-bindgen ${version}`) {
    throw new Error(`wasm-bindgen must match Cargo.lock (${version})`);
  }
  return path;
}
