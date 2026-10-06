import pg from 'pg';
import type { Analysis } from '../analyzer.ts';
import { describe } from '../contract.ts';
import { q } from '../model.ts';
import { executeQuery } from '../planner.ts';
import { invariantConstraint } from '../schema.ts';
import { executeCommand } from './command.ts';
import { AipError, type AipErrorBody } from './errors.ts';
import { fromDb, isUuid, validateInput, type Row } from './eval.ts';

export interface CallRequest {
  kind: 'query' | 'command';
  name: string;
  input: unknown;
  actorId: string | null;
  idempotencyKey: string | null;
}

export type CallResult = { ok: true; status: 200; data: unknown; trace: string[] } | { ok: false; status: number; error: AipErrorBody; trace: string[] };

export class Runtime {
  a: Analysis;
  pool: pg.Pool;
  invariants = new Map<string, { entity: string; name: string }>();

  constructor(a: Analysis, pool: pg.Pool) {
    this.a = a;
    this.pool = pool;
    for (const e of a.app.entities) {
      for (const inv of e.invariants) this.invariants.set(invariantConstraint(a.model, e.name, inv.name), { entity: e.name, name: inv.name });
    }
  }

  describe() {
    return describe(this.a);
  }

  async call(req: CallRequest): Promise<CallResult> {
    const trace: string[] = [];
    const op = req.name;
    const client = await this.pool.connect();
    let began = false;
    try {
      const query = req.kind === 'query' ? this.a.app.queries.find((x) => x.name === op) : undefined;
      const command = req.kind === 'command' ? this.a.app.commands.find((x) => x.name === op) : undefined;
      if (!query && !command) throw new AipError('AIP.REQUEST.UNKNOWN_OPERATION', op, `no ${req.kind} named '${op}'`);
      const input = validateInput(this.a.model, op, (query ?? command)!.params, req.input);

      // Queries read several statements (one per relation edge) from one snapshot.
      await client.query(query ? 'BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY' : 'BEGIN');
      began = true;
      const actor = await this.loadActor(client, op, req.actorId);
      const data = query
        ? await executeQuery(client, this.a.model, query, input, actor, trace)
        : await executeCommand(client, this.a.model, command!, { input, actor, idempotencyKey: req.idempotencyKey }, trace);
      await client.query('COMMIT');
      return { ok: true, status: 200, data, trace };
    } catch (e) {
      if (began) await client.query('ROLLBACK').catch(() => {});
      const err = this.toAipError(e, op);
      return { ok: false, status: err.status, error: err.body, trace };
    } finally {
      client.release();
    }
  }

  async loadActor(client: pg.PoolClient, op: string, actorId: string | null): Promise<Row | null> {
    if (!actorId) return null;
    const actorEntity = this.a.app.actor?.entity;
    if (!actorEntity || !isUuid(actorId)) throw new AipError('AIP.AUTH.UNAUTHENTICATED', op, 'invalid actor credentials');
    const m = this.a.model;
    const { rows } = await client.query(`SELECT * FROM ${q(m.table(actorEntity))} WHERE "id" = $1`, [actorId.toLowerCase()]);
    if (!rows.length) throw new AipError('AIP.AUTH.UNAUTHENTICATED', op, 'invalid actor credentials');
    return fromDb(m, actorEntity, rows[0]);
  }

  toAipError(e: unknown, op: string): AipError {
    if (e instanceof AipError) return e;
    const pgErr = e as { code?: string; constraint?: string; message?: string };
    switch (pgErr.code) {
      case '23514': {
        const inv = pgErr.constraint ? this.invariants.get(pgErr.constraint) : undefined;
        if (inv) return new AipError('AIP.INVARIANT.VIOLATED', op, `invariant ${inv.entity}.${inv.name} would be violated`, { reason: inv.name, path: inv.entity });
        break;
      }
      case '23503':
        return new AipError('AIP.INPUT.REFERENCE_NOT_FOUND', op, 'a referenced row does not exist');
      case '22003':
        return new AipError('AIP.INPUT.OUT_OF_RANGE', op, 'a numeric value is out of range');
      case '40001':
      case '40P01':
        return new AipError('AIP.CONCURRENCY.CONFLICT', op, 'concurrent update conflict; retry the request', { retryable: true });
    }
    console.error(`[aip] internal error in ${op}:`, e);
    return new AipError('AIP.INTERNAL', op, 'internal error', { retryable: false });
  }
}
