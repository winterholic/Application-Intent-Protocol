import type { Expr, PathExpr } from './ir.ts';

export interface SqlCtx {
  // Returns a SQL fragment for a path (a column reference or a bound parameter).
  path(p: PathExpr): string;
  // Returns a SQL fragment for a literal value.
  value(v: unknown): string;
}

const OPS: Record<string, string> = { '==': '=', '!=': '<>', and: 'AND', or: 'OR' };

export function sqlExpr(e: Expr, ctx: SqlCtx): string {
  switch (e.k) {
    case 'lit':
      return e.value === null ? 'NULL' : ctx.value(e.value);
    case 'enum':
      return ctx.value(e.value);
    case 'path':
      return ctx.path(e);
    case 'not':
      return `(NOT ${sqlExpr(e.e, ctx)})`;
    case 'neg':
      return `(-${sqlExpr(e.e, ctx)})`;
    case 'in':
      return `(${sqlExpr(e.e, ctx)} IN (${e.list.map((x) => sqlExpr(x, ctx)).join(', ')}))`;
    case 'bin': {
      const isNull = (x: Expr) => x.k === 'lit' && x.value === null;
      if ((e.op === '==' || e.op === '!=') && (isNull(e.l) || isNull(e.r))) {
        const other = isNull(e.l) ? e.r : e.l;
        return `(${sqlExpr(other, ctx)} IS ${e.op === '==' ? '' : 'NOT '}NULL)`;
      }
      return `(${sqlExpr(e.l, ctx)} ${OPS[e.op] ?? e.op} ${sqlExpr(e.r, ctx)})`;
    }
  }
}

export function inlineLiteral(v: unknown): string {
  if (typeof v === 'number') return String(Math.trunc(v));
  if (typeof v === 'boolean') return v ? 'TRUE' : 'FALSE';
  return `'${String(v).replace(/'/g, "''")}'`;
}

// Collects bound parameters in order for a single statement.
export class Params {
  values: unknown[] = [];
  add(v: unknown): string {
    this.values.push(v);
    return `$${this.values.length}`;
  }
}
