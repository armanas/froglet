import { spawn, type ChildProcess } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync } from 'node:fs';
import net from 'node:net';
import { tmpdir } from 'node:os';
import { resolve } from 'node:path';
import { repoRoot } from './route-helpers';

// Shared by the opt-in tests that run a real froglet-node: where the binary is, how to start one on free ports with a data
// directory of its own, and how to stop it so a test never leaves one running. Build the node first:
//   cargo build -p froglet --bin froglet-node
// FROGLET_NODE_BIN points at a different binary.

export const nodeBinary = process.env.FROGLET_NODE_BIN ?? resolve(repoRoot, 'target/debug/froglet-node');

export const wait = (ms: number) => new Promise((done) => setTimeout(done, ms));

export const freePort = () =>
  new Promise<number>((resolvePort, reject) => {
    const probe = net.createServer();
    probe.once('error', reject);
    probe.listen(0, '127.0.0.1', () => {
      const { port } = probe.address() as net.AddressInfo;
      probe.close(() => resolvePort(port));
    });
  });

export interface RunningNode {
  /** What the node has printed so far. */
  log(): string;
  /** The identity seed the node made for itself, which is the key its provider signs with. */
  seed: string;
  /** A bearer token the node wrote under its data directory: `auth` for the runtime API, `froglet-control` for the provider-control API. */
  token(name: 'auth' | 'froglet-control'): string;
  stop(): Promise<void>;
}

/**
 * Starts the node with `env` added to the environment, and waits until it answers on `healthUrl` and has written its
 * identity and runtime token. `label` names the data directory.
 */
export async function startNode(label: string, env: Record<string, string>, healthUrl: string): Promise<RunningNode> {
  if (!existsSync(nodeBinary)) throw new Error(`no node binary at ${nodeBinary}: run cargo build -p froglet --bin froglet-node`);
  const dataRoot = mkdtempSync(resolve(tmpdir(), `froglet-playground-${label}-`));
  const child: ChildProcess = spawn(nodeBinary, [], {
    env: { ...process.env, FROGLET_DATA_ROOT: dataRoot, ...env },
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let log = '';
  for (const stream of [child.stdout!, child.stderr!]) stream.on('data', (chunk) => (log += chunk));
  const seedFile = resolve(dataRoot, 'identity/secp256k1.seed');
  const tokenFile = resolve(dataRoot, 'runtime/auth.token');
  const controlFile = resolve(dataRoot, 'runtime/froglet-control.token');

  const stop = async () => {
    if (child.exitCode === null) {
      const exited = new Promise<void>((done) => child.once('exit', () => done()));
      child.kill('SIGTERM');
      await Promise.race([exited, wait(5000)]);
      if (child.exitCode === null) child.kill('SIGKILL');
    }
    rmSync(dataRoot, { recursive: true, force: true });
  };

  for (let attempt = 0; ; attempt++) {
    const up = await fetch(`${healthUrl}/health`).then((response) => response.ok, () => false);
    if (up && existsSync(seedFile) && existsSync(tokenFile)) break;
    if (child.exitCode !== null || attempt >= 300) {
      await stop();
      throw new Error(`the node ${child.exitCode !== null ? 'exited early' : 'did not come up'}:\n${log}`);
    }
    await wait(100);
  }

  return {
    log: () => log,
    seed: readFileSync(seedFile, 'utf8').trim(),
    token: (name) => readFileSync(name === 'auth' ? tokenFile : controlFile, 'utf8').trim(),
    stop,
  };
}
