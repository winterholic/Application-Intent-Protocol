// Command execution. A command runs as ONE database transaction covering
// loads (with row locks), policy, preconditions, mutations, outbox writes and
// the idempotency record, so a check can never be separated from its effect.

import { createHash } from 'node:crypto';
import type { Command, Expr, PathExpr, Stmt } from '../ir.ts';
import type { Model } from '../model.ts';
import { q } from '../model.ts';
import { IDEMPOTENCY, OUTBOX } from '../schema.ts';
import { Params, sqlExpr } from '../sql.ts';
import { AipError } from './errors.ts';
import { evalExpr, evalPolicy, fromDb, needsActor, type Env, type Row } from './eval.ts';
import { policyText, type Db } from '../planner.ts';

export interface CommandRequest {
  input: Record<string, unknown>;
  actor: Row | null;
  idempotencyKey: string | null;
}

export async function executeCommand(db: Db, m: Model, c: Command, req: CommandRequest, trace: string[] = []): Promise<unknown> {
  const run = async (sql: string, values: unknown[] = []) => {
    trace.push(sql);
    return (await db.query(sql, values)).rows;
  };
  const op = c.name;
  const { actor } = req;

  if (!actor && c.policy && needsActor(c.policy)) throw new AipError('AIP.AUTH.UNAUTHENTICATED', op, 'this command requires an authenticated actor');
  if (c.idempotent && !req.idempotencyKey) {
    throw new AipError('AIP.INPUT.IDEMPOTENCY_KEY_REQUIRED', op, `${op} is idempotent; send an Idempotency-Key header`);
  }
  if (!c.idempotent && req.idempotencyKey) {
    throw new AipError('AIP.INPUT.IDEMPOTENCY_KEY_UNSUPPORTED', op, `${op} is not declared idempotent; an Idempotency-Key would give a false guarantee`);
  }

  if (c.idempotent) {
    const hash = createHash('sha256').update(JSON.stringify(req.input)).digest('hex');
    const actorKey = actor?.id ?? '';
    // A concurrent request with the same key blocks on this insert until the
    // first one commits (then replays its response) or rolls back (then runs).
    const ins = await run(
      `INSERT INTO ${q(IDEMPOTENCY)} ("operation", "actor", "key", "request_hash") VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING RETURNING 1`,
      [op, actorKey, req.idempotencyKey, hash],
    );
    if (!ins.length) {
      const prev = await run(`SELECT "request_hash", "response" FROM ${q(IDEMPOTENCY)} WHERE "operation" = $1 AND "actor" = $2 AND "key" = $3`, [op, actorKey, req.idempotencyKey]);
      if (prev[0].request_hash !== hash) {
        throw new AipError('AIP.IDEMPOTENCY.KEY_REUSED', op, 'this Idempotency-Key was already used with a different input');
      }
      return prev[0].response;
    }
  }

  const env: Env = new Map(Object.entries(req.input));
  if (actor) env.set('actor', actor);
  const entityOf = new Map<string, string>();

  for (const l of c.loads) {
    const id = evalExpr(l.by, env);
    const rows = await run(`SELECT * FROM ${q(m.table(l.entity))} WHERE "id" = $1${l.lock ? ' FOR UPDATE' : ''}`, [id]);
    if (!rows.length) throw new AipError('AIP.NOT_FOUND', op, `${l.entity} not found`, { reason: `${upperSnake(l.entity)}_NOT_FOUND`, path: l.name });
    env.set(l.name, fromDb(m, l.entity, rows[0]));
    entityOf.set(l.name, l.entity);
  }

  if (!evalPolicy(c.policy!, env, actor)) {
    throw new AipError(actor ? 'AIP.AUTH.FORBIDDEN' : 'AIP.AUTH.UNAUTHENTICATED', op, `policy denied: ${policyText(c.policy!)}`);
  }

  for (const r of c.requires) {
    if (!evalExpr(r.cond, env)) throw new AipError('AIP.PRECONDITION.FAILED', op, `precondition failed: ${r.code}`, { reason: r.code });
  }

  await execStmts(c.tx, env, entityOf, m, run);

  for (const em of c.emits) {
    const payload: Record<string, unknown> = {};
    for (const f of em.fields) payload[f.name] = evalExpr(f.value, env);
    await run(`INSERT INTO ${q(OUTBOX)} ("event", "operation", "payload") VALUES ($1, $2, $3)`, [em.event, op, JSON.stringify(payload)]);
  }

  const result = c.returns ? env.get(c.returns.name) : null;

  if (c.idempotent) {
    await run(`UPDATE ${q(IDEMPOTENCY)} SET "response" = $4 WHERE "operation" = $1 AND "actor" = $2 AND "key" = $3`, [op, actor?.id ?? '', req.idempotencyKey, JSON.stringify(result)]);
  }
  return result;
}

type Run = (sql: string, values?: unknown[]) => Promise<Record<string, unknown>[]>;

async function execStmts(stmts: Stmt[], env: Env, entityOf: Map<string, string>, m: Model, run: Run) {
  for (const s of stmts) {
    switch (s.k) {
      case 'set':
      case 'inc': {
        const entity = entityOf.get(s.target.root)!;
        const row = env.get(s.target.root) as Row;
        const col = q(m.column(m.field(entity, s.target.fields[0])!));
        const value = s.k === 'set' ? evalExpr(s.value, env) : s.sign * (evalExpr(s.by, env) as number);
        // Increments are applied in SQL (col = col + n) so they are atomic even without a lock.
        const setSql = s.k === 'set' ? `${col} = $1` : `${col} = ${col} + $1`;
        const rows = await run(`UPDATE ${q(m.table(entity))} SET ${setSql} WHERE "id" = $2 RETURNING *`, [value, row.id]);
        env.set(s.target.root, fromDb(m, entity, rows[0]));
        break;
      }
      case 'create': {
        const cols: string[] = [];
        const params = new Params();
        const vals: string[] = [];
        for (const fv of s.fields) {
          cols.push(q(m.column(m.field(s.entity, fv.name)!)));
          vals.push(params.add(evalExpr(fv.value, env)));
        }
        const rows = await run(`INSERT INTO ${q(m.table(s.entity))} (${cols.join(', ')}) VALUES (${vals.join(', ')}) RETURNING *`, params.values);
        if (s.as) {
          env.set(s.as, fromDb(m, s.entity, rows[0]));
          entityOf.set(s.as, s.entity);
        }
        break;
      }
      case 'each': {
        const touched = await execEach(s, env, entityOf, m, run);
        // Bindings of entities written in bulk are re-read so later statements see fresh values.
        for (const [name, entity] of entityOf) {
          if (!touched.has(entity)) continue;
          const row = env.get(name) as Row;
          const rows = await run(`SELECT * FROM ${q(m.table(entity))} WHERE "id" = $1`, [row.id]);
          env.set(name, fromDb(m, entity, rows[0]));
        }
        break;
      }
    }
  }
}

// `each parent.rel as it { ... }` compiles every body statement to ONE
// set-based UPDATE; there is never a per-row round-trip.
async function execEach(s: Extract<Stmt, { k: 'each' }>, env: Env, entityOf: Map<string, string>, m: Model, run: Run): Promise<Set<string>> {
  const parentEntity = entityOf.get(s.source.root)!;
  const parent = env.get(s.source.root) as Row;
  const rel = m.field(parentEntity, s.source.fields[0]) as Extract<ReturnType<Model['field']>, { kind: 'many' }>;
  const child = rel.target;
  const viaCol = q(m.column(m.field(child, rel.via)!));
  const touched = new Set<string>();

  for (const st of s.body) {
    if (st.k !== 'set' && st.k !== 'inc') continue;
    const params = new Params();
    const parentParam = params.add(parent.id);
    const ctx = (alias: string) => ({
      path: (p: PathExpr) => {
        if (p.root === s.as) return `${alias}.${q(m.column(m.field(child, p.fields[0] ?? 'id')!))}`;
        return params.add(evalExpr(p, env));
      },
      value: (v: unknown) => params.add(v),
    });
    const t = st.target;
    if (t.fields.length === 1) {
      const col = q(m.column(m.field(child, t.fields[0])!));
      const expr: Expr = st.k === 'set' ? st.value : st.by;
      const rhs = sqlExpr(expr, ctx('c'));
      const setSql = st.k === 'set' ? `${col} = ${rhs}` : `${col} = c.${col} ${st.sign > 0 ? '+' : '-'} ${rhs}`;
      await run(`UPDATE ${q(m.table(child))} AS c SET ${setSql} WHERE c.${viaCol} = ${parentParam}`, params.values);
      touched.add(child);
    } else if (st.k === 'inc') {
      // Several children may point to the same target row (e.g. two lines of
      // the same product); UPDATE ... FROM would apply only one of them, so
      // amounts are summed per target first.
      const ref = m.field(child, t.fields[0]) as Extract<ReturnType<Model['field']>, { kind: 'ref' }>;
      const target = ref.target;
      const refCol = q(m.column(ref));
      const col = q(m.column(m.field(target, t.fields[1])!));
      const amount = sqlExpr(st.by, ctx('c'));
      await run(
        `UPDATE ${q(m.table(target))} AS t SET ${col} = t.${col} ${st.sign > 0 ? '+' : '-'} agg.v ` +
          `FROM (SELECT c.${refCol} AS id, SUM(${amount}) AS v FROM ${q(m.table(child))} AS c WHERE c.${viaCol} = ${parentParam} GROUP BY c.${refCol}) AS agg ` +
          `WHERE t."id" = agg.id`,
        params.values,
      );
      touched.add(target);
    }
  }
  return touched;
}

export function upperSnake(s: string): string {
  return s.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toUpperCase();
}
