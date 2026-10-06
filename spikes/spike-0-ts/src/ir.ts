// Typed AIP IR. The DSL parser produces this shape, the analyzer validates it
// and fills in `type` on expressions, and the planner/runtime consume it.
// It must stay JSON-serializable so other front-ends (or an LLM) can emit it directly.

export interface Loc {
  line: number;
  col: number;
}

export type ScalarName = 'UUID' | 'String' | 'Int' | 'Bool';
export const SCALARS: readonly ScalarName[] = ['UUID', 'String', 'Int', 'Bool'];

// Value types as seen by expressions. `ref` is the id of a many-to-one target.
export type ValueType =
  | { k: 'scalar'; name: ScalarName }
  | { k: 'enum'; name: string }
  | { k: 'ref'; entity: string }
  | { k: 'many'; entity: string }
  | { k: 'null' };

export type Field =
  | { kind: 'scalar'; name: string; type: ScalarName; optional: boolean; loc: Loc }
  | { kind: 'enum'; name: string; enum: string; optional: boolean; loc: Loc }
  | { kind: 'ref'; name: string; target: string; optional: boolean; loc: Loc }
  | { kind: 'many'; name: string; target: string; via: string; loc: Loc };

export interface Invariant {
  name: string;
  expr: Expr;
  loc: Loc;
}

export interface Entity {
  name: string;
  fields: Field[];
  invariants: Invariant[];
  loc: Loc;
}

export interface EnumDef {
  name: string;
  values: string[];
  loc: Loc;
}

export type BinOp = '+' | '-' | '*' | '==' | '!=' | '<' | '<=' | '>' | '>=' | 'and' | 'or';

export type Expr =
  | { k: 'lit'; value: string | number | boolean | null; loc: Loc; type?: ValueType }
  | { k: 'enum'; enum: string; value: string; loc: Loc; type?: ValueType }
  // `root` is a param, a binding, `actor`, an iterator, or (unresolved) a bare enum value.
  | { k: 'path'; root: string; fields: string[]; loc: Loc; type?: ValueType }
  | { k: 'bin'; op: BinOp; l: Expr; r: Expr; loc: Loc; type?: ValueType }
  | { k: 'not'; e: Expr; loc: Loc; type?: ValueType }
  | { k: 'neg'; e: Expr; loc: Loc; type?: ValueType }
  | { k: 'in'; e: Expr; list: Expr[]; loc: Loc; type?: ValueType };

export type PathExpr = Extract<Expr, { k: 'path' }>;

export type Policy =
  | { k: 'public'; loc: Loc }
  | { k: 'authenticated'; loc: Loc }
  | { k: 'role'; role: string; loc: Loc }
  | { k: 'owner'; path: PathExpr; loc: Loc }
  | { k: 'or'; l: Policy; r: Policy; loc: Loc }
  | { k: 'and'; l: Policy; r: Policy; loc: Loc };

export interface Param {
  name: string;
  type: ValueType; // scalar or enum only
  loc: Loc;
}

export interface Load {
  name: string;
  entity: string;
  by: Expr;
  lock: boolean;
  loc: Loc;
}

export interface Require {
  cond: Expr;
  code: string;
  loc: Loc;
}

export interface FieldValue {
  name: string;
  value: Expr;
  loc: Loc;
}

export type Stmt =
  | { k: 'set'; target: PathExpr; value: Expr; loc: Loc }
  | { k: 'inc'; target: PathExpr; by: Expr; sign: 1 | -1; loc: Loc }
  | { k: 'create'; entity: string; fields: FieldValue[]; as: string | null; loc: Loc }
  | { k: 'each'; source: PathExpr; as: string; body: Stmt[]; loc: Loc };

export interface Emit {
  event: string;
  fields: FieldValue[];
  loc: Loc;
}

export interface Command {
  name: string;
  params: Param[];
  idempotent: boolean;
  loads: Load[];
  policy: Policy | null;
  requires: Require[];
  tx: Stmt[];
  emits: Emit[];
  returns: { name: string; loc: Loc } | null;
  loc: Loc;
}

export interface Selection {
  fields: SelNode[];
}

export interface SelNode {
  name: string;
  sub: Selection | null;
  loc: Loc;
}

export interface Query {
  name: string;
  params: Param[];
  entity: string;
  as: string;
  by: Expr | null;
  where: Expr | null;
  sort: { field: string; dir: 'asc' | 'desc'; loc: Loc } | null;
  limit: number | null;
  policy: Policy | null;
  select: Selection;
  loc: Loc;
}

export interface App {
  file: string;
  actor: { entity: string; loc: Loc } | null;
  enums: EnumDef[];
  entities: Entity[];
  queries: Query[];
  commands: Command[];
}
