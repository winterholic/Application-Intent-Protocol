// Query planning. A query declares its whole data graph up front, so the
// planner emits one statement per relation edge in the selection: the number
// of round-trips depends on the shape of the query, never on the row count.

import type { Field, Policy, Query, Selection } from './ir.ts';
import { Model, q } from './model.ts';
import { Params, sqlExpr } from './sql.ts';
import { AipError } from './runtime/errors.ts';
import { evalExpr, evalPolicy, needsActor, type Env, type Row } from './runtime/eval.ts';

export interface PlanNode {
  entity: string;
  path: string;
  // Columns fetched for this node (IR fields, including implicit id).
  fields: Field[];
  // Fields returned to the client, in selection order.
  output: string[];
  children: { field: Field; node: PlanNode }[];
}

export function buildTree(m: Model, entity: string, sel: Selection, path: string): PlanNode {
  const fields = new Map<string, Field>();
  fields.set('id', m.field(entity, 'id')!);
  const children: PlanNode['children'] = [];
  for (const n of sel.fields) {
    const f = m.field(entity, n.name)!;
    if (f.kind === 'ref') {
      fields.set(f.name, f);
      children.push({ field: f, node: buildTree(m, f.target, n.sub!, `${path}.${f.name}`) });
    } else if (f.kind === 'many') {
      const node = buildTree(m, f.target, n.sub!, `${path}.${f.name}`);
      // The child must carry its back-reference so rows can be grouped under parents.
      if (!node.fields.some((x) => x.name === f.via)) node.fields.push(m.field(f.target, f.via)!);
      children.push({ field: f, node });
    } else {
      fields.set(f.name, f);
    }
  }
  return { entity, path, fields: [...fields.values()], output: sel.fields.map((n) => n.name), children };
}

function selectList(m: Model, node: PlanNode, alias: string): string {
  return node.fields.map((f) => `${alias}.${q(m.column(f))}`).join(', ');
}

function childSql(m: Model, parent: PlanNode, edge: PlanNode['children'][number]): string {
  const { field, node } = edge;
  const t = q(m.table(node.entity));
  if (field.kind === 'ref') {
    return `SELECT ${selectList(m, node, 't')} FROM ${t} AS t WHERE t."id" = ANY($1::uuid[])`;
  }
  const via = m.field(node.entity, (field as Extract<Field, { kind: 'many' }>).via)!;
  return `SELECT ${selectList(m, node, 't')} FROM ${t} AS t WHERE t.${q(m.column(via))} = ANY($1::uuid[]) ORDER BY t."id"`;
}

// Policy → SQL predicate for list queries, so rows the actor may not see are
// never read. Row-independent terms fold to TRUE/FALSE.
function policySql(p: Policy, alias: string, m: Model, entity: string, actor: Row | null, params: Params): string {
  switch (p.k) {
    case 'public':
      return 'TRUE';
    case 'authenticated':
      return actor ? 'TRUE' : 'FALSE';
    case 'role':
      return actor && actor.role === p.role ? 'TRUE' : 'FALSE';
    case 'owner': {
      if (!actor) return 'FALSE';
      const f = m.field(entity, p.path.fields[0] ?? 'id')!;
      return `(${alias}.${q(m.column(f))} = ${params.add(actor.id)})`;
    }
    case 'or':
      return `(${policySql(p.l, alias, m, entity, actor, params)} OR ${policySql(p.r, alias, m, entity, actor, params)})`;
    case 'and':
      return `(${policySql(p.l, alias, m, entity, actor, params)} AND ${policySql(p.r, alias, m, entity, actor, params)})`;
  }
}

function rootSql(m: Model, query: Query, tree: PlanNode, env: Env, actor: Row | null, params: Params): string {
  const alias = 'r';
  const ctx = {
    path: (p: { root: string; fields: string[] }) => {
      if (p.root === query.as) {
        const f = m.field(query.entity, p.fields[0] ?? 'id')!;
        return `${alias}.${q(m.column(f))}`;
      }
      return params.add(evalExpr({ k: 'path', root: p.root, fields: p.fields, loc: { line: 0, col: 0 } }, env));
    },
    value: (v: unknown) => params.add(v),
  };
  const conds: string[] = [];
  if (query.by) conds.push(`${alias}."id" = ${sqlExpr(query.by, ctx)}`);
  if (query.where) conds.push(sqlExpr(query.where, ctx));
  // `by` queries check the policy on the loaded row instead (to tell FORBIDDEN from NOT_FOUND).
  if (!query.by) conds.push(policySql(query.policy!, alias, m, query.entity, actor, params));
  let sql = `SELECT ${selectList(m, tree, alias)} FROM ${q(m.table(query.entity))} AS ${alias}`;
  if (conds.length) sql += ` WHERE ${conds.join(' AND ')}`;
  if (query.sort) {
    const f = m.field(query.entity, query.sort.field)!;
    sql += ` ORDER BY ${alias}.${q(m.column(f))} ${query.sort.dir.toUpperCase()}, ${alias}."id"`;
  } else if (!query.by) {
    sql += ` ORDER BY ${alias}."id"`;
  }
  if (query.limit !== null) sql += ` LIMIT ${query.limit}`;
  return sql;
}

// ---- explain ----

export interface ExplainStep {
  path: string;
  strategy: string;
  sql: string;
}

// The root also fetches every column its policy looks at, so `by` queries can
// evaluate owner(...) without an extra round-trip.
export function queryTree(m: Model, query: Query): PlanNode {
  const tree = buildTree(m, query.entity, query.select, query.entity);
  const walk = (p: Policy) => {
    if (p.k === 'or' || p.k === 'and') {
      walk(p.l);
      walk(p.r);
    } else if (p.k === 'owner' && p.path.root === query.as) {
      const f = m.field(query.entity, p.path.fields[0] ?? 'id')!;
      if (!tree.fields.includes(f)) tree.fields.push(f);
    }
  };
  if (query.policy) walk(query.policy);
  return tree;
}

export function explainQuery(m: Model, query: Query): { query: string; steps: ExplainStep[]; roundTrips: string } {
  const tree = queryTree(m, query);
  const params = new Params();
  const env: Env = new Map();
  const symbolic = new Proxy({}, { get: () => '<actor>' }) as Row;
  for (const p of query.params) env.set(p.name, `<${p.name}>`);
  env.set('actor', symbolic);
  const steps: ExplainStep[] = [{ path: tree.path, strategy: query.by ? 'root: by id' : 'root: filtered list', sql: rootSql(m, query, tree, env, symbolic, params) }];
  const walk = (node: PlanNode) => {
    for (const edge of node.children) {
      steps.push({
        path: edge.node.path,
        strategy: edge.field.kind === 'ref' ? 'batch by id (many-to-one)' : 'batch by foreign key (one-to-many)',
        sql: childSql(m, node, edge),
      });
      walk(edge.node);
    }
  };
  walk(tree);
  return { query: query.name, steps, roundTrips: `${steps.length} (fixed; independent of row count)` };
}

// ---- execution ----

export interface Db {
  query(text: string, values?: unknown[]): Promise<{ rows: Record<string, unknown>[] }>;
}

type Out = Record<string, unknown>;

async function fill(db: Db, m: Model, node: PlanNode, rows: Record<string, unknown>[], trace: string[]): Promise<Out[]> {
  const outs: Out[] = rows.map(() => ({}));
  for (const edge of node.children) {
    const { field, node: child } = edge;
    if (field.kind === 'ref') {
      const col = m.column(field);
      const ids = [...new Set(rows.map((r) => r[col]).filter((v) => v !== null))];
      const childRows = ids.length ? (await run(db, childSql(m, node, edge), [ids], trace)) : [];
      const childOuts = await fill(db, m, child, childRows, trace);
      const byId = new Map(childRows.map((r, i) => [r.id, childOuts[i]]));
      rows.forEach((r, i) => (outs[i][field.name] = r[col] === null ? null : byId.get(r[col]) ?? null));
    } else if (field.kind === 'many') {
      const via = m.column(m.field(child.entity, field.via)!);
      const ids = rows.map((r) => r.id);
      const childRows = ids.length ? await run(db, childSql(m, node, edge), [ids], trace) : [];
      const childOuts = await fill(db, m, child, childRows, trace);
      const groups = new Map<unknown, Out[]>();
      childRows.forEach((r, i) => {
        const g = groups.get(r[via]) ?? [];
        g.push(childOuts[i]);
        groups.set(r[via], g);
      });
      rows.forEach((r, i) => (outs[i][field.name] = groups.get(r.id) ?? []));
    }
  }
  const scalars = node.output.filter((n) => !node.children.some((c) => c.field.name === n));
  rows.forEach((r, i) => {
    const o: Out = {};
    for (const name of node.output) {
      if (scalars.includes(name)) o[name] = r[m.column(m.field(node.entity, name)!)];
      else o[name] = outs[i][name];
    }
    outs[i] = o;
  });
  return outs;
}

async function run(db: Db, sql: string, values: unknown[], trace: string[]) {
  trace.push(sql);
  return (await db.query(sql, values)).rows;
}

export async function executeQuery(db: Db, m: Model, query: Query, input: Record<string, unknown>, actor: Row | null, trace: string[] = []): Promise<unknown> {
  const policy = query.policy!;
  if (!actor && needsActor(policy)) throw new AipError('AIP.AUTH.UNAUTHENTICATED', query.name, 'this query requires an authenticated actor');
  const env: Env = new Map(Object.entries(input));
  if (actor) env.set('actor', actor);
  const tree = queryTree(m, query);
  const params = new Params();
  const sql = rootSql(m, query, tree, env, actor, params);

  if (query.by) {
    const rows = await run(db, sql, params.values, trace);
    if (!rows.length) throw new AipError('AIP.NOT_FOUND', query.name, `${query.entity} not found`, { reason: `${query.entity.toUpperCase()}_NOT_FOUND`, path: query.as });
    const row: Record<string, unknown> = {};
    for (const f of tree.fields) row[f.name] = rows[0][m.column(f)];
    const penv = new Map(env);
    penv.set(query.as, row);
    if (!evalPolicy(policy, penv, actor)) {
      throw new AipError(actor ? 'AIP.AUTH.FORBIDDEN' : 'AIP.AUTH.UNAUTHENTICATED', query.name, `policy denied: ${policyText(policy)}`, { path: query.as });
    }
    return (await fill(db, m, tree, rows, trace))[0];
  }
  if (!needsActor(policy) || actor) {
    // A list policy that folds to FALSE for this actor is a denial, not an empty result.
    const probe = new Params();
    const folded = policySql(policy, 'r', m, query.entity, actor, probe);
    if (folded === 'FALSE') throw new AipError('AIP.AUTH.FORBIDDEN', query.name, `policy denied: ${policyText(policy)}`);
  }
  const rows = await run(db, sql, params.values, trace);
  return fill(db, m, tree, rows, trace);
}

export function policyText(p: Policy): string {
  switch (p.k) {
    case 'public':
    case 'authenticated':
      return p.k;
    case 'role':
      return `role(${p.role})`;
    case 'owner':
      return `owner(${[p.path.root, ...p.path.fields].join('.')})`;
    case 'or':
      return `${policyText(p.l)} | ${policyText(p.r)}`;
    case 'and':
      return `${wrap(p.l)} & ${wrap(p.r)}`;
  }
}

function wrap(p: Policy): string {
  return p.k === 'or' ? `(${policyText(p)})` : policyText(p);
}
