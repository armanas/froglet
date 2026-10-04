// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { repo, wasmBuildEnv } from '../../../scripts/wasm-tools.mjs';

const remap = `--remap-path-prefix=${repo}=/froglet`;

describe('browser Rust build flags', () => {
  it('remaps checkout paths without changing the parent environment', () => {
    const original = { PATH: '/example/bin', CARGO_TARGET_DIR: '/example/cache' };
    expect(wasmBuildEnv(original)).toEqual({ ...original, CARGO_ENCODED_RUSTFLAGS: remap });
    expect(original).not.toHaveProperty('CARGO_ENCODED_RUSTFLAGS');
  });

  it('keeps encoded arguments intact and gives them precedence over RUSTFLAGS', () => {
    const original = { CARGO_ENCODED_RUSTFLAGS: '--cfg\x1fexample="two words"', RUSTFLAGS: '--invalid-ignored-flag' };
    expect(wasmBuildEnv(original).CARGO_ENCODED_RUSTFLAGS.split('\x1f')).toEqual([
      '--cfg', 'example="two words"', remap,
    ]);
  });

  it('respects an explicitly empty encoded flag list', () => {
    expect(wasmBuildEnv({ CARGO_ENCODED_RUSTFLAGS: '', RUSTFLAGS: '--invalid-ignored-flag' }).CARGO_ENCODED_RUSTFLAGS).toBe(remap);
  });

  it('keeps space-separated RUSTFLAGS when encoded flags are absent', () => {
    expect(wasmBuildEnv({ RUSTFLAGS: '  -D warnings  --cfg example\n' }).CARGO_ENCODED_RUSTFLAGS.split('\x1f')).toEqual([
      '-D', 'warnings', '--cfg', 'example', remap,
    ]);
  });

  it('matches Cargo 1.91 tokenization rather than interpreting shell quotes or internal tabs', () => {
    expect(wasmBuildEnv({ RUSTFLAGS: '-D\twarnings --cfg "two words"' }).CARGO_ENCODED_RUSTFLAGS.split('\x1f')).toEqual([
      '-D\twarnings', '--cfg', '"two', 'words"', remap,
    ]);
    expect(wasmBuildEnv({ CARGO_ENCODED_RUSTFLAGS: '-D\x1f\x1fwarnings' }).CARGO_ENCODED_RUSTFLAGS.split('\x1f')).toEqual([
      '-D', '', 'warnings', remap,
    ]);
  });
});
