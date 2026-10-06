import { readFileSync } from 'node:fs';
import pg from 'pg';
import type { Analysis } from '../src/analyzer.ts';
import { compileSource, hasErrors } from '../src/compile.ts';
import { formatDiagnostic } from '../src/diagnostics.ts';
import { Runtime, type CallRequest } from '../src/runtime/runtime.ts';
import { migrate } from '../src/schema.ts';

export const SHOP = new URL('../examples/shop/app.aip', import.meta.url).pathname;
export const SHOP_SRC = readFileSync(SHOP, 'utf8');

const ADMIN_URL = process.env.AIP_TEST_ADMIN_URL ?? 'postgres://localhost/postgres';
const TEST_DB = process.env.AIP_TEST_DB ?? 'aip_test';

export function compileOk(src: string, file = 'test.aip'): Analysis {
  const { analysis, diagnostics } = compileSource(src, file);
  if (!analysis || hasErrors(diagnostics)) throw new Error(diagnostics.map((d) => formatDiagnostic(d, file)).join('\n'));
  return analysis;
}

export async function freshDb(a: Analysis): Promise<pg.Pool> {
  const admin = new pg.Client({ connectionString: ADMIN_URL });
  await admin.connect();
  const { rows } = await admin.query('SELECT 1 FROM pg_database WHERE datname = $1', [TEST_DB]);
  if (!rows.length) await admin.query(`CREATE DATABASE "${TEST_DB}"`);
  await admin.end();
  const url = new URL(ADMIN_URL);
  url.pathname = `/${TEST_DB}`;
  const pool = new pg.Pool({ connectionString: url.toString(), max: 20 });
  const c = await pool.connect();
  try {
    await migrate(c, a.model, { reset: true });
  } finally {
    c.release();
  }
  return pool;
}

export interface Seed {
  alice: string;
  bob: string;
  admin: string;
  productA: string;
  productB: string;
}

export async function seedShop(pool: pg.Pool): Promise<Seed> {
  const one = async (sql: string, v: unknown[]) => (await pool.query(sql, v)).rows[0].id as string;
  return {
    alice: await one(`INSERT INTO "user" (name, role) VALUES ($1, 'CUSTOMER') RETURNING id`, ['alice']),
    bob: await one(`INSERT INTO "user" (name, role) VALUES ($1, 'CUSTOMER') RETURNING id`, ['bob']),
    admin: await one(`INSERT INTO "user" (name, role) VALUES ($1, 'ADMIN') RETURNING id`, ['root']),
    productA: await one(`INSERT INTO product (name, price, stock) VALUES ('Keyboard', 100, 5) RETURNING id`, []),
    productB: await one(`INSERT INTO product (name, price, stock) VALUES ('Mouse', 50, 3) RETURNING id`, []),
  };
}

// Thin helper: throws on unexpected failures so tests read as a script.
export function caller(rt: Runtime) {
  const call = (kind: CallRequest['kind'], name: string, actorId: string | null, input: unknown = {}, idempotencyKey: string | null = null) =>
    rt.call({ kind, name, input, actorId, idempotencyKey });
  return {
    raw: call,
    async ok(kind: CallRequest['kind'], name: string, actorId: string | null, input: unknown = {}, key: string | null = null) {
      const r = await call(kind, name, actorId, input, key);
      if (!r.ok) throw new Error(`${name} failed: ${JSON.stringify(r.error)}`);
      return r.data as any;
    },
  };
}
