import type { HttpRequest, HttpResponse, Transport } from './types';

export interface WireEntry {
  n: number;
  method: string;
  path: string;
  status: number;
  ms: number;
  request?: unknown;
  response: unknown;
}

export interface Wire {
  transport: Transport;
  entries: WireEntry[];
  clear(): void;
}

/**
 * The network between two parties in one page. Nothing leaves the browser: each message crosses as JSON text, so neither
 * side can see or change the other's objects, and every crossing is recorded for the page to show.
 */
export function createWire(handler: (request: HttpRequest) => Promise<HttpResponse>, onEntry?: (entry: WireEntry) => void): Wire {
  const entries: WireEntry[] = [];
  let count = 0;
  return {
    entries,
    clear: () => {
      entries.length = 0;
      count = 0;
    },
    transport: async (request) => {
      const sent: HttpRequest = JSON.parse(JSON.stringify(request));
      const started = performance.now();
      const answered = await handler(sent);
      const received: HttpResponse = JSON.parse(JSON.stringify(answered));
      const entry: WireEntry = {
        n: (count += 1),
        method: sent.method,
        path: sent.path,
        status: received.status,
        ms: performance.now() - started,
        ...(sent.body !== undefined ? { request: sent.body } : {}),
        response: received.body,
      };
      entries.push(entry);
      onEntry?.(entry);
      return received;
    },
  };
}
