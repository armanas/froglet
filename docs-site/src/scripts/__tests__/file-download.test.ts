import { readFileSync } from 'node:fs';
import { createHash, webcrypto } from 'node:crypto';
import { Blob as NodeBlob } from 'node:buffer';
import { JSDOM } from 'jsdom';
import { afterEach, expect, test, vi } from 'vitest';

const script = readFileSync(`${process.cwd()}/public/service-share.js`, 'utf8');
const windows: JSDOM[] = [];
afterEach(() => { windows.splice(0).forEach(dom => dom.window.close()); });

function fixture(bytes = 'file', invite = false) {
  const dom = new JSDOM('<button data-file-download>Download file</button><input id="file-invitation"><p data-download-status></p><script id="file-download-config" type="application/json"></script>', {url:'https://froglet.dev', runScripts:'outside-only'});
  windows.push(dom);
  const w = dom.window;
  const metadata = {url:`https://${'a'.repeat(52)}.relay.froglet.dev/v1/provider/services/test/files/${'b'.repeat(64)}/download`, size_bytes:4, sha256:createHash('sha256').update('file').digest('hex'), expires_at:Math.floor(Date.now()/1000)+60, filename:'test.txt', invite};
  w.document.getElementById('file-download-config')!.textContent = JSON.stringify(metadata);
  const fetcher = vi.fn(async (_url: unknown, _options: RequestInit) => new Response(bytes, {headers:{'content-length':'4'}}));
  Object.defineProperty(w, 'fetch', {value:fetcher});
  Object.defineProperty(w, 'Blob', {value:NodeBlob});
  Object.defineProperty(w.crypto, 'subtle', {value:webcrypto.subtle});
  const create = vi.fn(() => 'blob:verified');
  Object.defineProperty(w.URL, 'createObjectURL', {value:create});
  Object.defineProperty(w.URL, 'revokeObjectURL', {value:vi.fn()});
  vi.spyOn(w.HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
  w.eval(script);
  const status = () => w.document.querySelector('[data-download-status]')!.textContent ?? '';
  const click = async () => {
    (w.document.querySelector('button') as HTMLButtonElement).click();
    await vi.waitFor(() => expect(status()).not.toMatch(/^$|Downloading/));
  };
  return {w, fetcher, create, click, status, metadata};
}

test('does no preview transfer; verifies bytes before making a save link', async () => {
  const f = fixture();
  expect(f.fetcher).not.toHaveBeenCalled();
  await f.click();
  expect(f.status()).toContain('SHA-256 verified');
  expect(f.create).toHaveBeenCalledOnce();
  expect(f.w.document.querySelector('[data-file-save]')?.getAttribute('download')).toBe('test.txt');
  expect(f.fetcher.mock.calls[0][1]).toMatchObject({credentials:'omit', redirect:'error', cache:'no-store', referrerPolicy:'no-referrer'});
});

test.each([['fake','Checksum mismatch'],['longer','exceeded its approved size'],['a','interrupted']])('refuses invalid response %s without saving', async (bytes, error) => {
  const f = fixture(bytes);
  await f.click();
  expect(f.status()).toContain(error);
  expect(f.create).not.toHaveBeenCalled();
  expect(f.w.document.querySelector('[data-file-save]')).toBeNull();
});

test('invitation is sent only in a header then cleared even on denial', async () => {
  const f = fixture('file', true);
  const token = 'invitation-'.repeat(4);
  const input = f.w.document.getElementById('file-invitation') as HTMLInputElement;
  input.value = token;
  f.fetcher.mockResolvedValueOnce(new Response('', {status:403}));
  await f.click();
  expect(f.fetcher.mock.calls[0][1].headers).toEqual({'x-froglet-access-token':token});
  expect(String(f.fetcher.mock.calls[0][0])).not.toContain(token);
  expect(input.value).toBe('');
  expect(f.w.localStorage.length).toBe(0);
  expect(f.create).not.toHaveBeenCalled();
});

test.each(['http://127.0.0.1/file','https://evil.example/download'])('refuses unverified destinations %s', async url => {
  const f = fixture();
  f.metadata.url = url;
  f.w.document.getElementById('file-download-config')!.textContent = JSON.stringify(f.metadata);
  await f.click();
  expect(f.fetcher).not.toHaveBeenCalled();
  expect(f.status()).toContain('metadata is invalid');
});
