#!/usr/bin/env node
// Integration check: run the real website Wasm generators in three checkout roots, including a fresh target cache.
// It writes only a new output directory, uses existing dependencies/tools, and never deploys.
import { createHash } from 'node:crypto';
import { execFileSync, spawn } from 'node:child_process';
import { chmodSync, createWriteStream, copyFileSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, realpathSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const requiredArtifacts = [
  'assemblyscript/asc.js',
  'kernel/froglet_wasm.d.ts', 'kernel/froglet_wasm.js', 'kernel/froglet_wasm_bg.wasm', 'kernel/froglet_wasm_bg.wasm.d.ts',
  'playground/adder.wasm', 'playground/fibonacci.wasm',
  'verifier/free-chain.json', 'verifier/froglet_verify.d.ts', 'verifier/froglet_verify.js', 'verifier/froglet_verify_bg.wasm', 'verifier/froglet_verify_bg.wasm.d.ts',
];
const options = new Map();
const allowed = new Set(['--source', '--source-manifest', '--output', '--node-modules', '--wasm-bindgen', '--timeout-ms']);
for (let i = 2; i < process.argv.length; i += 2) {
  const key = process.argv[i];
  if (!allowed.has(key) || options.has(key) || !process.argv[i + 1]) throw new Error(`Invalid option ${key}`);
  options.set(key, process.argv[i + 1]);
}
if (!options.has('--output') || !isAbsolute(options.get('--output'))) throw new Error('--output must name a new absolute directory');
if (process.platform === 'win32') throw new Error('This opt-in check currently requires POSIX process groups and directory symlinks');
const source = realpathSync(options.get('--source') || resolve(dirname(fileURLToPath(import.meta.url)), '../..'));
// Resolve the existing parent so a symlink cannot send evidence back into the source checkout.
const requestedOutput = resolve(options.get('--output'));
const output = join(realpathSync(dirname(requestedOutput)), basename(requestedOutput));
if (output === source || output.startsWith(source + sep)) throw new Error('Output must be outside the source checkout');
if (existsSync(output)) throw new Error('Refusing to reuse or overwrite an existing output directory');
const deps = realpathSync(options.get('--node-modules') || join(source, 'docs-site/node_modules'));
const generator = realpathSync(options.get('--wasm-bindgen') || process.env.WASM_BINDGEN || join(source, 'target/verifier-tools/bin/wasm-bindgen'));
const timeoutMs = Number(options.get('--timeout-ms') || 600000);
if (!Number.isSafeInteger(timeoutMs) || timeoutMs < 1000 || timeoutMs > 1800000) throw new Error('Build timeout must be an integer from1000 to1800000 milliseconds');
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const hashFile = path => digest(readFileSync(path));
const smallCommand = (cmd, args, cwd = source, env = process.env) => execFileSync(cmd, args, { cwd, env, encoding: 'utf8', timeout: 10000 }).trim();
const fileRecord = (root, path) => ({ path, bytes: statSync(join(root, path)).size, mode: statSync(join(root, path)).mode & 0o777, sha256: hashFile(join(root, path)) });
const writeJson = (path, value) => writeFileSync(path, JSON.stringify(value, null, 2) + '\n');
let input;
if (options.has('--source-manifest')) {
  input = JSON.parse(readFileSync(options.get('--source-manifest'), 'utf8'));
  if (!Array.isArray(input.files) || !input.files.length) throw new Error('Source manifest has no files');
} else {
  const paths = execFileSync('git', ['ls-files', '-z'], { cwd: source, encoding: 'utf8', timeout: 10000 }).split('\0').filter(Boolean);
  input = { source, head: smallCommand('git', ['rev-parse', 'HEAD']), tree: smallCommand('git', ['rev-parse', 'HEAD^{tree}']), git_status_short: smallCommand('git', ['status', '--short']), files: paths.map(path => fileRecord(source, path)) };
}
if (options.has('--source-manifest')) {
  const actualPaths = collect(source).map(file => file.path).sort();
  if (JSON.stringify(actualPaths) !== JSON.stringify(input.files.map(file => file.path).sort())) throw new Error('Frozen source export contains missing or unmanifested files');
}
const seen = new Set();
for (const file of input.files) {
  if (typeof file.path !== 'string' || isAbsolute(file.path) || file.path.split(/[\\/]/).includes('..') || file.path.includes('\0') || seen.has(file.path)) throw new Error('Unsafe or duplicate manifest path');
  seen.add(file.path);
  const original = join(source, file.path);
  if (!lstatSync(original).isFile() || lstatSync(original).isSymbolicLink()) throw new Error(`Source is not a regular file: ${file.path}`);
  const actual = fileRecord(source, file.path);
  if (actual.sha256 !== file.sha256 || actual.bytes !== file.bytes || actual.mode !== file.mode) throw new Error(`Source manifest mismatch: ${file.path}`);
}
for (const path of ['Cargo.lock', 'docs-site/package-lock.json', 'docs-site/scripts/build-verifier.mjs', 'docs-site/scripts/build-playground.mjs', 'docs-site/scripts/wasm-tools.mjs']) {
  if (!seen.has(path)) throw new Error(`Required source input missing: ${path}`);
}
const version = readFileSync(join(source, 'Cargo.lock'), 'utf8').match(/name = "wasm-bindgen"\nversion = "([^"]+)"/)?.[1];
if (!version || smallCommand(generator, ['--version']) !== `wasm-bindgen ${version}`) throw new Error('Existing wasm-bindgen does not match Cargo.lock');
const pinnedToolchain = smallCommand('rustup', ['show', 'active-toolchain']).split(/\s/)[0];
for (const key of ['RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER']) if (process.env[key]) throw new Error(`Custom ${key} is outside this pinned-toolchain check`);
mkdirSync(output, { recursive: true });
const report = {
  status: 'running', harness: { path: fileURLToPath(import.meta.url), sha256: hashFile(fileURLToPath(import.meta.url)) }, started_at: new Date().toISOString(), source, output,
  source_manifest: { path: join(output, 'source-manifest.json'), sha256: null, file_count: input.files.length, head: input.head, tree: input.tree, git_status_short: input.git_status_short },
  toolchain: { node: process.version, node_executable: process.execPath, rustc: smallCommand('rustc', ['--version', '--verbose']), cargo: smallCommand('cargo', ['--version']), wasm_bindgen: { path: generator, version, sha256: hashFile(generator) }, rustup_toolchain: pinnedToolchain, deps: { path: deps, assemblyscript: JSON.parse(readFileSync(join(deps, 'assemblyscript/package.json'))).version, esbuild: JSON.parse(readFileSync(join(deps, 'esbuild/package.json'))).version } },
  environment: { CARGO_TARGET_DIR: join(output, 'cargo-target'), WASM_BINDGEN: generator, CARGO_NET_OFFLINE: 'true', CARGO_INCREMENTAL: '0', CARGO_BUILD_JOBS: '2', CARGO_TERM_COLOR: 'never', RUSTUP_TOOLCHAIN: pinnedToolchain, RUSTFLAGS: process.env.RUSTFLAGS ?? null, CARGO_ENCODED_RUSTFLAGS: process.env.CARGO_ENCODED_RUSTFLAGS ?? null },
  per_build_timeout_ms: timeoutMs, roots: [], builds: [], artifacts: {}, differences: [], embedded_checkout_paths: {}, cancellation: null,
  limits: ['Same local POSIX host, Rust/Node/toolchain/dependencies only; no cross-OS/toolchain guarantee.', 'Byte equality, including path-sensitive metadata, is required. No byte normalization.', 'Only generated website inputs are compared; production deployment and full Astro output are outside this check.'],
};
const manifestPath = join(output, 'source-manifest.json');
writeJson(manifestPath, input); report.source_manifest.sha256 = hashFile(manifestPath);
const environment = { ...process.env, ...Object.fromEntries(Object.entries(report.environment).filter(([key]) => !['RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS'].includes(key))) };
let activeBuild;
function stopActive(signal) {
  report.cancellation = signal;
  if (activeBuild) activeBuild.cancel();
}
const onInterrupt = () => stopActive('SIGINT');
const onTerminate = () => stopActive('SIGTERM');
process.on('SIGINT', onInterrupt); process.on('SIGTERM', onTerminate);
async function runBuild(root, script, label, target) {
  const logPath = join(output, `${label}-${script.replace('.mjs', '')}.log`);
  const record = { root, cargo_target: target, command: [process.execPath, `docs-site/scripts/${script}`], log_path: logPath, started_at: new Date().toISOString(), timed_out: false };
  report.builds.push(record); writeJson(join(output, 'verification.json'), report);
  console.log(`${label}: ${script}`);
  const log = createWriteStream(logPath, { flags: 'wx' });
  const child = spawn(process.execPath, [join(root, 'docs-site/scripts', script)], { cwd: root, env: { ...environment, CARGO_TARGET_DIR: target }, detached: true, stdio: ['ignore', 'pipe', 'pipe'] });
  child.stdout.pipe(log, { end: false }); child.stderr.pipe(log, { end: false });
  let forceKill;
  let stopRequested = false;
  const terminate = signal => { try { process.kill(-child.pid, signal); } catch (error) { if (error.code !== 'ESRCH') throw error; } };
  const stop = () => { stopRequested = true; terminate('SIGTERM'); if (!forceKill) forceKill = setTimeout(() => terminate('SIGKILL'), 2000); };
  activeBuild = { cancel: stop };
  const timer = setTimeout(() => { record.timed_out = true; stop(); }, timeoutMs);
  let result;
  try {
    result = await new Promise((resolve, reject) => { child.once('error', reject); child.once('close', (code, signal) => resolve({ code, signal })); });
  } finally { activeBuild = undefined; clearTimeout(timer); if (stopRequested) terminate('SIGKILL'); if (forceKill) clearTimeout(forceKill); await new Promise(resolve => log.end(resolve)); }
  Object.assign(record, result, { finished_at: new Date().toISOString(), log_sha256: hashFile(logPath) });
  if (report.cancellation) throw new Error(`Interrupted by ${report.cancellation}`);
  if (record.timed_out || result.code !== 0) throw new Error(`${label} ${script} ${record.timed_out ? 'timed out' : `exited ${result.code}`}; inspect ${logPath}`);
}
function collect(root, base = root) {
  const records = [];
  for (const entry of readdirSync(root, { withFileTypes: true }).sort((a,b) => a.name.localeCompare(b.name))) {
    const full = join(root, entry.name);
    if (entry.isDirectory()) records.push(...collect(full, base));
    else if (entry.isFile()) records.push(fileRecord(base, relative(base, full).split(sep).join('/')));
    else throw new Error(`Unexpected generated symlink or special file: ${full}`);
  }
  return records.sort((a,b) => a.path.localeCompare(b.path));
}
try {
  // The third root uses an entirely fresh Cargo target as an independent stale-cache control.
  for (const [label, name] of [['short', 'a'], ['long', 'checkout with a deliberately much longer root name'], ['cold', 'fresh-target checkout control with a longer root']]) {
    const root = join(output, name); mkdirSync(root);
    for (const file of input.files) { const dest = join(root, file.path); mkdirSync(dirname(dest), { recursive: true }); copyFileSync(join(source, file.path), dest); chmodSync(dest, file.mode); }
    symlinkSync(deps, join(root, 'docs-site/node_modules'), 'dir');
    const actualToolchain = { rustc: smallCommand('rustc', ['--version', '--verbose'], root, environment), cargo: smallCommand('cargo', ['--version'], root, environment) };
    if (actualToolchain.rustc !== report.toolchain.rustc || actualToolchain.cargo !== report.toolchain.cargo) throw new Error(`Toolchain differs in ${label} root`);
    const cargoTarget = join(output, label === 'cold' ? 'cold-cargo-target' : 'cargo-target');
    report.roots.push({ label, path: root, realpath: realpathSync(root), path_bytes: Buffer.byteLength(root), source_file_count: input.files.length, cargo_target: cargoTarget, actual_toolchain: actualToolchain });
    for (const script of ['build-verifier.mjs', 'build-playground.mjs']) await runBuild(root, script, label, cargoTarget);
    // Full contents, length, type and mode are checked, including the input snapshot itself.
    for (const location of [source, root]) for (const file of input.files) {
      const record = fileRecord(location, file.path);
      if (!lstatSync(join(location, file.path)).isFile() || record.sha256 !== file.sha256 || record.bytes !== file.bytes || record.mode !== file.mode) throw new Error(`Build changed source: ${location}/${file.path}`);
    }
    // Cargo's own compile notices must bind rebuilt workspace crates to this root.
    const log = report.builds.filter(build => build.root === root).map(build => readFileSync(build.log_path, 'utf8')).join('\n');
    const expectedCrates = ['froglet-protocol', 'froglet-verify', 'froglet-wasm', 'adder', 'fibonacci'];
    const rebuilt = expectedCrates.filter(name => log.split('\n').some(line => line.includes(`Compiling ${name} v`) && line.includes(`(${root}/`)));
    report.roots.at(-1).local_crates_recompiled = rebuilt;
    if (rebuilt.length !== expectedCrates.length) throw new Error(`Local crates were not proven rebuilt at ${label}: ${expectedCrates.filter(name => !rebuilt.includes(name)).join(', ')}`);
    const generated = join(root, 'docs-site/src/generated');
    const files = collect(generated);
    const expectedPaths = [...requiredArtifacts].sort();
    if (JSON.stringify(files.map(file => file.path).sort()) !== JSON.stringify(expectedPaths)) throw new Error(`Generated artifact set differs from exact reviewed twelve files in ${label}`);
    report.embedded_checkout_paths[label] = [];
    for (const file of files) {
      const bytes = readFileSync(join(generated, file.path));
      for (const physicalRoot of report.roots.map(value => value.realpath)) if (bytes.includes(Buffer.from(physicalRoot))) report.embedded_checkout_paths[label].push({ artifact: file.path, root: physicalRoot });
    }
    report.artifacts[label] = files;
    writeJson(join(output, `${label}-generated-manifest.json`), files);
  }
  if (report.roots[0].path_bytes === report.roots[1].path_bytes) throw new Error('Regression roots must have different path lengths');
  const left = new Map(report.artifacts.short.map(file => [file.path, file]));
  for (const label of ['long', 'cold']) {
  const right = new Map(report.artifacts[label].map(file => [file.path, file]));
  for (const path of [...new Set([...left.keys(), ...right.keys()])].sort()) {
    const a = left.get(path); const b = right.get(path);
    if (!a || !b || a.sha256 !== b.sha256 || a.bytes !== b.bytes) report.differences.push({ comparison: `short_vs_${label}`, path, short: a || null, other: b || null });
    else if (!readFileSync(join(report.roots[0].path, 'docs-site/src/generated', path)).equals(readFileSync(join(report.roots.find(value => value.label === label).path, 'docs-site/src/generated', path)))) throw new Error(`Digest collision or comparison failure: ${path}`);
  }
  }
  const embeddedCount = Object.values(report.embedded_checkout_paths).reduce((count, rows) => count + rows.length, 0);
  report.status = report.differences.length || embeddedCount ? 'fail' : 'pass';
  report.finished_at = new Date().toISOString();
  writeJson(join(output, 'verification.json'), report);
  console.log(`${report.status}: three roots, twelve files each; ${report.differences.length} byte differences, ${embeddedCount} embedded checkout paths`);
  process.exitCode = report.status === 'pass' ? 0 : 1;
} catch (error) {
  report.status = 'incomplete'; report.error = String(error); report.finished_at = new Date().toISOString();
  writeJson(join(output, 'verification.json'), report); console.error(error); process.exitCode = report.cancellation === 'SIGINT' ? 130 : report.cancellation === 'SIGTERM' ? 143 : 2;
} finally {
  process.removeListener('SIGINT', onInterrupt); process.removeListener('SIGTERM', onTerminate);
}
