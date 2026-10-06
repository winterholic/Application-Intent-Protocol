import { createServer, type IncomingMessage, type Server } from 'node:http';
import type { Runtime } from './runtime.ts';

const MAX_BODY = 1024 * 1024;

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    let size = 0;
    const chunks: Buffer[] = [];
    req.on('data', (c: Buffer) => {
      size += c.length;
      if (size > MAX_BODY) {
        reject(new Error('body too large'));
        req.destroy();
        return;
      }
      chunks.push(c);
    });
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
    req.on('error', reject);
  });
}

function header(req: IncomingMessage, name: string): string | null {
  const v = req.headers[name];
  return typeof v === 'string' && v.length > 0 ? v : null;
}

export function createAipServer(rt: Runtime, opts: { log?: boolean } = {}): Server {
  return createServer(async (req, res) => {
    const started = Date.now();
    const send = (status: number, body: unknown) => {
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(body));
    };
    const url = new URL(req.url ?? '/', 'http://localhost');
    if (req.method === 'GET' && url.pathname === '/aip/describe') return send(200, rt.describe());

    const m = /^\/aip\/(query|command)\/([A-Za-z_][A-Za-z0-9_]*)$/.exec(url.pathname);
    if (!m || req.method !== 'POST') {
      return send(404, { error: { code: 'AIP.REQUEST.UNKNOWN_OPERATION', operation: url.pathname, message: 'use POST /aip/query/{name} or /aip/command/{name}', retryable: false } });
    }
    let input: unknown;
    try {
      const raw = await readBody(req);
      input = raw.trim() ? JSON.parse(raw) : {};
    } catch {
      return send(400, { error: { code: 'AIP.REQUEST.MALFORMED', operation: m[2], message: 'body must be JSON (max 1MB)', retryable: false } });
    }
    // DEV ONLY: the actor is taken from a header. A real deployment plugs a verified identity in here.
    const r = await rt.call({ kind: m[1] as 'query' | 'command', name: m[2], input, actorId: header(req, 'x-aip-actor'), idempotencyKey: header(req, 'idempotency-key') });
    if (opts.log) {
      console.log(JSON.stringify({ op: `${m[1]}:${m[2]}`, status: r.status, code: r.ok ? null : r.error.code, reason: r.ok ? null : r.error.reason ?? null, statements: r.trace.length, ms: Date.now() - started }));
    }
    return r.ok ? send(200, { data: r.data }) : send(r.status, { error: r.error });
  });
}
