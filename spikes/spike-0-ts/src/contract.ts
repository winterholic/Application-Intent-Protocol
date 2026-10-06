// Machine-readable contract served at /aip/describe. It is derived only from
// the analyzed IR, so what clients (and agents) see is exactly what runs.

import type { Analysis } from './analyzer.ts';
import type { Command, Param, Query, Selection, ValueType } from './ir.ts';
import type { Model } from './model.ts';
import { policyText } from './planner.ts';

export type CType =
  | 'UUID'
  | 'String'
  | 'Int'
  | 'Bool'
  | { enum: string }
  | { object: Record<string, CType> }
  | { list: CType }
  | { nullable: CType };

export const CONTRACT_VERSION = '0.1';

function valueType(t: ValueType): CType {
  switch (t.k) {
    case 'scalar':
      return t.name;
    case 'enum':
      return { enum: t.name };
    case 'ref':
      return 'UUID';
    default:
      return 'String';
  }
}

function inputType(params: Param[]): Record<string, CType> {
  return Object.fromEntries(params.map((p) => [p.name, valueType(p.type)]));
}

function selectionType(m: Model, entity: string, sel: Selection): CType {
  const out: Record<string, CType> = {};
  for (const n of sel.fields) {
    const f = m.field(entity, n.name)!;
    let t: CType;
    switch (f.kind) {
      case 'scalar':
        t = f.type;
        break;
      case 'enum':
        t = { enum: f.enum };
        break;
      case 'ref':
        t = selectionType(m, f.target, n.sub!);
        break;
      case 'many':
        t = { list: selectionType(m, f.target, n.sub!) };
        break;
    }
    out[n.name] = 'optional' in f && f.optional ? { nullable: t } : t;
  }
  return { object: out };
}

// A command returns the bound row: its own columns, references as ids.
function rowType(m: Model, entity: string): CType {
  const out: Record<string, CType> = {};
  for (const f of m.columns(entity)) {
    const t: CType = f.kind === 'scalar' ? f.type : f.kind === 'enum' ? { enum: f.enum } : 'UUID';
    out[f.name] = 'optional' in f && f.optional ? { nullable: t } : t;
  }
  return { object: out };
}

function queryErrors(qr: Query): string[] {
  const errs = ['AIP.INPUT.INVALID', 'AIP.AUTH.UNAUTHENTICATED', 'AIP.AUTH.FORBIDDEN'];
  if (qr.by) errs.push('AIP.NOT_FOUND');
  return errs;
}

function commandErrors(a: Analysis, c: Command): { code: string; reasons?: string[] }[] {
  const info = a.commands.get(c.name)!;
  const out: { code: string; reasons?: string[] }[] = [{ code: 'AIP.INPUT.INVALID' }, { code: 'AIP.AUTH.UNAUTHENTICATED' }, { code: 'AIP.AUTH.FORBIDDEN' }];
  if (c.loads.length) out.push({ code: 'AIP.NOT_FOUND', reasons: [...new Set(c.loads.map((l) => `${l.entity.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toUpperCase()}_NOT_FOUND`))] });
  if (c.requires.length) out.push({ code: 'AIP.PRECONDITION.FAILED', reasons: c.requires.map((r) => r.code) });
  const touched = new Set([...info.writes.map((w) => w.split('.')[0]), ...info.creates]);
  const invs = a.app.entities.filter((e) => touched.has(e.name)).flatMap((e) => e.invariants.map((i) => i.name));
  if (invs.length) out.push({ code: 'AIP.INVARIANT.VIOLATED', reasons: invs });
  if (info.creates.length) out.push({ code: 'AIP.INPUT.REFERENCE_NOT_FOUND' });
  if (c.idempotent) out.push({ code: 'AIP.INPUT.IDEMPOTENCY_KEY_REQUIRED' }, { code: 'AIP.IDEMPOTENCY.KEY_REUSED' });
  out.push({ code: 'AIP.CONCURRENCY.CONFLICT' });
  return out;
}

export function describe(a: Analysis) {
  const m = a.model;
  const queries = Object.fromEntries(
    a.app.queries.map((qr) => {
      const shape = selectionType(m, qr.entity, qr.select);
      return [qr.name, {
        kind: 'query',
        input: inputType(qr.params),
        output: qr.by ? shape : { list: shape },
        policy: policyText(qr.policy!),
        errors: queryErrors(qr).map((code) => ({ code })),
      }];
    }),
  );
  const commands = Object.fromEntries(
    a.app.commands.map((c) => {
      const info = a.commands.get(c.name)!;
      const retEntity = c.returns ? (c.loads.find((l) => l.name === c.returns!.name)?.entity ?? findCreated(c, c.returns.name)) : null;
      return [c.name, {
        kind: 'command',
        input: inputType(c.params),
        output: retEntity ? rowType(m, retEntity) : null,
        policy: policyText(c.policy!),
        idempotent: c.idempotent,
        effects: {
          writes: info.writes,
          creates: [...new Set(info.creates)],
          emits: c.emits.map((e) => e.event),
          delivery: c.emits.length ? 'at-least-once via transactional outbox' : null,
        },
        errors: commandErrors(a, c),
      }];
    }),
  );
  const events = Object.fromEntries([...a.events.values()].map((e) => [e.name, { object: Object.fromEntries(e.fields.map((f) => [f.name, valueType(f.type)])) }]));
  const enums = Object.fromEntries(a.app.enums.map((e) => [e.name, e.values]));
  return {
    aip: CONTRACT_VERSION,
    transport: { describe: 'GET /aip/describe', query: 'POST /aip/query/{name}', command: 'POST /aip/command/{name}', headers: ['x-aip-actor (dev only)', 'idempotency-key'] },
    enums,
    queries,
    commands,
    events,
  };
}

function findCreated(c: Command, name: string): string | null {
  const walk = (stmts: Command['tx']): string | null => {
    for (const s of stmts) {
      if (s.k === 'create' && s.as === name) return s.entity;
      if (s.k === 'each') {
        const r = walk(s.body);
        if (r) return r;
      }
    }
    return null;
  };
  return walk(c.tx);
}

export type Contract = ReturnType<typeof describe>;
