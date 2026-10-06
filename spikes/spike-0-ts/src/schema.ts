// Entities → PostgreSQL DDL. Invariants become CHECK constraints so they hold
// even under concurrent writers, and every reference column gets an index
// because the planner batch-loads relations by those columns.

import type pg from 'pg';
import type { Field } from './ir.ts';
import { Model, q } from './model.ts';
import { inlineLiteral, sqlExpr } from './sql.ts';

export const OUTBOX = '_aip_outbox';
export const IDEMPOTENCY = '_aip_idempotency';

function columnType(m: Model, f: Field): string {
  switch (f.kind) {
    case 'scalar':
      return { UUID: 'uuid', String: 'text', Int: 'integer', Bool: 'boolean' }[f.type];
    case 'enum':
      return 'text';
    case 'ref':
      return 'uuid';
    case 'many':
      throw new Error('many has no column');
  }
}

export function invariantConstraint(m: Model, entity: string, name: string): string {
  return `${m.table(entity)}__${name}`;
}

export function generateDDL(m: Model): string[] {
  const out: string[] = [];
  const later: string[] = [];
  for (const e of m.app.entities) {
    const t = m.table(e.name);
    const cols: string[] = ['"id" uuid PRIMARY KEY DEFAULT gen_random_uuid()'];
    const checks: string[] = [];
    for (const f of e.fields) {
      if (f.kind === 'many') continue;
      const col = m.column(f);
      cols.push(`${q(col)} ${columnType(m, f)}${f.optional ? '' : ' NOT NULL'}`);
      if (f.kind === 'enum') {
        const values = m.enums.get(f.enum)!.values.map(inlineLiteral).join(', ');
        checks.push(`CONSTRAINT ${q(`${t}__${col}_enum`)} CHECK (${q(col)} IN (${values}))`);
      }
      if (f.kind === 'ref') {
        later.push(`ALTER TABLE ${q(t)} ADD CONSTRAINT ${q(`${t}__${col}_fk`)} FOREIGN KEY (${q(col)}) REFERENCES ${q(m.table(f.target))}("id")`);
        later.push(`CREATE INDEX ${q(`${t}__${col}_idx`)} ON ${q(t)} (${q(col)})`);
      }
    }
    for (const inv of e.invariants) {
      const cond = sqlExpr(inv.expr, {
        path: (p) => q(m.column(m.field(e.name, p.root)!)),
        value: inlineLiteral,
      });
      checks.push(`CONSTRAINT ${q(invariantConstraint(m, e.name, inv.name))} CHECK ${cond}`);
    }
    out.push(`CREATE TABLE ${q(t)} (\n  ${[...cols, ...checks].join(',\n  ')}\n)`);
  }
  out.push(...later);
  out.push(`CREATE TABLE ${q(OUTBOX)} (
  "id" bigserial PRIMARY KEY,
  "event" text NOT NULL,
  "operation" text NOT NULL,
  "payload" jsonb NOT NULL,
  "created_at" timestamptz NOT NULL DEFAULT now(),
  "delivered_at" timestamptz
)`);
  out.push(`CREATE TABLE ${q(IDEMPOTENCY)} (
  "operation" text NOT NULL,
  "actor" text NOT NULL,
  "key" text NOT NULL,
  "request_hash" text NOT NULL,
  "response" jsonb,
  "created_at" timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY ("operation", "actor", "key")
)`);
  return out;
}

export async function migrate(client: pg.Client | pg.PoolClient, m: Model, opts: { reset: boolean }): Promise<string[]> {
  const ddl = generateDDL(m);
  await client.query('BEGIN');
  try {
    if (opts.reset) {
      const tables = [...m.app.entities.map((e) => m.table(e.name)), OUTBOX, IDEMPOTENCY];
      await client.query(`DROP TABLE IF EXISTS ${tables.map(q).join(', ')} CASCADE`);
    } else {
      const { rows } = await client.query(`SELECT 1 FROM information_schema.tables WHERE table_name = $1`, [OUTBOX]);
      if (rows.length) throw new Error('schema already exists; PoC migrations only support --reset');
    }
    for (const stmt of ddl) await client.query(stmt);
    await client.query('COMMIT');
  } catch (e) {
    await client.query('ROLLBACK');
    throw e;
  }
  return ddl;
}
