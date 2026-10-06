import type { Expr, Param, Policy } from '../ir.ts';
import type { Model } from '../model.ts';
import { AipError } from './errors.ts';

// A loaded row keyed by IR field names; references hold the target id.
export type Row = Record<string, unknown> & { id: string };

export type Env = Map<string, unknown>;

export function isRow(v: unknown): v is Row {
  return typeof v === 'object' && v !== null && 'id' in v;
}

export function fromDb(m: Model, entity: string, dbRow: Record<string, unknown>): Row {
  const out: Record<string, unknown> = {};
  for (const f of m.columns(entity)) out[f.name] = dbRow[m.column(f)];
  return out as Row;
}

export function evalExpr(e: Expr, env: Env): unknown {
  switch (e.k) {
    case 'lit':
      return e.value;
    case 'enum':
      return e.value;
    case 'path': {
      let v = env.get(e.root);
      if (e.fields.length === 0) return isRow(v) ? v.id : v;
      for (const f of e.fields) v = isRow(v) ? v[f] : undefined;
      return v ?? null;
    }
    case 'not':
      return !evalExpr(e.e, env);
    case 'neg':
      return -(evalExpr(e.e, env) as number);
    case 'in': {
      const v = evalExpr(e.e, env);
      return e.list.some((x) => evalExpr(x, env) === v);
    }
    case 'bin': {
      if (e.op === 'and') return Boolean(evalExpr(e.l, env)) && Boolean(evalExpr(e.r, env));
      if (e.op === 'or') return Boolean(evalExpr(e.l, env)) || Boolean(evalExpr(e.r, env));
      const l = evalExpr(e.l, env) as number;
      const r = evalExpr(e.r, env) as number;
      switch (e.op) {
        case '+':
          return l + r;
        case '-':
          return l - r;
        case '*':
          return l * r;
        case '==':
          return l === r;
        case '!=':
          return l !== r;
        case '<':
          return l < r;
        case '<=':
          return l <= r;
        case '>':
          return l > r;
        case '>=':
          return l >= r;
      }
    }
  }
}

export function needsActor(p: Policy): boolean {
  switch (p.k) {
    case 'public':
      return false;
    case 'or':
      return needsActor(p.l) && needsActor(p.r);
    case 'and':
      return needsActor(p.l) || needsActor(p.r);
    default:
      return true;
  }
}

export function evalPolicy(p: Policy, env: Env, actor: Row | null): boolean {
  switch (p.k) {
    case 'public':
      return true;
    case 'authenticated':
      return actor !== null;
    case 'role':
      return actor !== null && actor.role === p.role;
    case 'owner':
      return actor !== null && evalExpr(p.path, env) === actor.id;
    case 'or':
      return evalPolicy(p.l, env, actor) || evalPolicy(p.r, env, actor);
    case 'and':
      return evalPolicy(p.l, env, actor) && evalPolicy(p.r, env, actor);
  }
}

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const INT4_MIN = -2147483648;
const INT4_MAX = 2147483647;

export function isUuid(v: unknown): v is string {
  return typeof v === 'string' && UUID_RE.test(v);
}

// Inputs are validated strictly: unknown keys and missing keys are errors.
export function validateInput(m: Model, op: string, params: Param[], raw: unknown): Record<string, unknown> {
  const bad = (path: string, message: string): never => {
    throw new AipError('AIP.INPUT.INVALID', op, message, { path });
  };
  if (raw === undefined || raw === null) raw = {};
  if (typeof raw !== 'object' || Array.isArray(raw)) bad('', 'input must be a JSON object');
  const obj = raw as Record<string, unknown>;
  const out: Record<string, unknown> = {};
  for (const k of Object.keys(obj)) {
    if (!params.some((p) => p.name === k)) bad(k, `unknown input field '${k}'`);
  }
  for (const p of params) {
    const v = obj[p.name];
    if (v === undefined || v === null) bad(p.name, `'${p.name}' is required`);
    const t = p.type;
    if (t.k === 'enum') {
      const values = m.enums.get(t.name)!.values;
      if (typeof v !== 'string' || !values.includes(v)) bad(p.name, `'${p.name}' must be one of ${values.join(', ')}`);
      out[p.name] = v;
      continue;
    }
    if (t.k !== 'scalar') continue;
    switch (t.name) {
      case 'UUID':
        if (!isUuid(v)) bad(p.name, `'${p.name}' must be a UUID`);
        out[p.name] = (v as string).toLowerCase();
        break;
      case 'Int':
        if (typeof v !== 'number' || !Number.isInteger(v) || v < INT4_MIN || v > INT4_MAX) bad(p.name, `'${p.name}' must be a 32-bit integer`);
        out[p.name] = v;
        break;
      case 'String':
        if (typeof v !== 'string') bad(p.name, `'${p.name}' must be a string`);
        out[p.name] = v;
        break;
      case 'Bool':
        if (typeof v !== 'boolean') bad(p.name, `'${p.name}' must be a boolean`);
        out[p.name] = v;
        break;
    }
  }
  return out;
}
