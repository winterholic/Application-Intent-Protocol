// Static analysis over the IR. Everything the runtime relies on must be
// established here: names resolve, types line up, every operation has an
// explicit policy, and no data is read implicitly through a relation.

import type { Diagnostic } from './diagnostics.ts';
import type { App, Command, Entity, Expr, Loc, PathExpr, Policy, Query, Selection, Stmt, ValueType } from './ir.ts';
import { fieldType, Model, typeName } from './model.ts';

type SymKind = 'param' | 'binding' | 'actor' | 'iter' | 'alias' | 'field';

interface Sym {
  kind: SymKind;
  type: ValueType;
  // How many `.field` hops are allowed from this root. Anything deeper would
  // need an implicit load, which AIP forbids (no lazy loading → no hidden N+1).
  maxDepth: number;
  lock?: boolean;
  fromLoad?: boolean;
}

type Scope = Map<string, Sym>;

export interface EventSchema {
  name: string;
  fields: { name: string; type: ValueType }[];
  loc: Loc;
}

export interface CommandInfo {
  writes: string[]; // "Entity.field"
  creates: string[];
  errors: string[];
}

export interface Analysis {
  app: App;
  model: Model;
  diagnostics: Diagnostic[];
  events: Map<string, EventSchema>;
  commands: Map<string, CommandInfo>;
}

const INT: ValueType = { k: 'scalar', name: 'Int' };
const BOOL: ValueType = { k: 'scalar', name: 'Bool' };

export function compatible(a: ValueType, b: ValueType): boolean {
  if (a.k === 'null' || b.k === 'null') return a.k !== 'many' && b.k !== 'many';
  if (a.k === 'scalar' && b.k === 'scalar') return a.name === b.name;
  if (a.k === 'enum' && b.k === 'enum') return a.name === b.name;
  if (a.k === 'ref' && b.k === 'ref') return a.entity === b.entity;
  // A reference and a UUID carry the same value (the target's id).
  if (a.k === 'ref' && b.k === 'scalar') return b.name === 'UUID';
  if (a.k === 'scalar' && b.k === 'ref') return a.name === 'UUID';
  return false;
}

function pathText(p: PathExpr): string {
  return [p.root, ...p.fields].join('.');
}

class Analyzer {
  app: App;
  model: Model;
  diags: Diagnostic[] = [];
  events = new Map<string, EventSchema>();
  commands = new Map<string, CommandInfo>();

  constructor(app: App) {
    this.app = app;
    this.model = new Model(app);
  }

  err(code: string, message: string, loc: Loc, help?: string) {
    this.diags.push({ severity: 'error', code, message, loc, help });
  }

  warn(code: string, message: string, loc: Loc, help?: string) {
    this.diags.push({ severity: 'warning', code, message, loc, help });
  }

  run(): Analysis {
    this.checkNames();
    for (const e of this.app.entities) this.resolveFields(e);
    for (const e of this.app.entities) this.checkEntity(e);
    if (this.app.actor && !this.model.entities.has(this.app.actor.entity)) {
      this.err('AIP-E110', `actor entity '${this.app.actor.entity}' is not defined`, this.app.actor.loc);
    }
    for (const q of this.app.queries) this.checkQuery(q);
    for (const c of this.app.commands) this.checkCommand(c);
    this.diags.sort((a, b) => a.loc.line - b.loc.line || a.loc.col - b.loc.col);
    return { app: this.app, model: this.model, diagnostics: this.diags, events: this.events, commands: this.commands };
  }

  checkNames() {
    const types = new Map<string, Loc>();
    for (const d of [...this.app.enums, ...this.app.entities]) {
      if (types.has(d.name)) this.err('AIP-E101', `duplicate type name '${d.name}'`, d.loc);
      types.set(d.name, d.loc);
    }
    const ops = new Map<string, Loc>();
    for (const d of [...this.app.queries, ...this.app.commands]) {
      if (ops.has(d.name)) this.err('AIP-E101', `duplicate operation name '${d.name}'`, d.loc, 'queries and commands share one namespace');
      ops.set(d.name, d.loc);
    }
    for (const en of this.app.enums) {
      const seen = new Set<string>();
      for (const v of en.values) {
        if (seen.has(v)) this.err('AIP-E101', `duplicate value '${v}' in enum ${en.name}`, en.loc);
        seen.add(v);
      }
      if (en.values.length === 0) this.err('AIP-E101', `enum ${en.name} has no values`, en.loc);
    }
  }

  resolveFields(e: Entity) {
    e.fields = e.fields.map((f) => {
      if (f.kind === 'ref' && this.model.enums.has(f.target)) {
        return { kind: 'enum', name: f.name, enum: f.target, optional: f.optional, loc: f.loc };
      }
      return f;
    });
  }

  checkEntity(e: Entity) {
    const seen = new Set<string>();
    for (const f of e.fields) {
      if (f.name === 'id') {
        this.err('AIP-E105', `field 'id' is implicit on every entity`, f.loc, `remove the 'id' declaration from ${e.name}`);
        continue;
      }
      if (seen.has(f.name)) this.err('AIP-E101', `duplicate field '${f.name}' in ${e.name}`, f.loc);
      seen.add(f.name);
      if (f.kind === 'ref' && !this.model.entities.has(f.target)) {
        this.err('AIP-E102', `unknown type '${f.target}' for field ${e.name}.${f.name}`, f.loc);
      }
      if (f.kind === 'many') {
        const back = this.model.entities.has(f.target) ? this.model.field(f.target, f.via) : undefined;
        if (!this.model.entities.has(f.target)) {
          this.err('AIP-E102', `unknown type '${f.target}' for field ${e.name}.${f.name}`, f.loc);
        } else if (!back || back.kind !== 'ref' || back.target !== e.name) {
          this.err('AIP-E106', `${e.name}.${f.name} is 'via ${f.via}', but ${f.target}.${f.via} is not a reference to ${e.name}`, f.loc,
            `declare '${f.via}: ${e.name}' on ${f.target}`);
        }
      }
    }
    // Invariants compile to CHECK constraints, so they may only see this row's own columns.
    const scope: Scope = new Map();
    for (const f of e.fields) {
      if (f.kind === 'scalar' || f.kind === 'enum') scope.set(f.name, { kind: 'field', type: fieldType(f), maxDepth: 0 });
    }
    const names = new Set<string>();
    for (const inv of e.invariants) {
      if (names.has(inv.name)) this.err('AIP-E101', `duplicate invariant '${inv.name}'`, inv.loc);
      names.add(inv.name);
      this.expectType(inv.expr, scope, BOOL, `invariant ${inv.name}`);
    }
  }

  // ---- expressions ----

  isBareUnknown(e: Expr, scope: Scope): boolean {
    return e.k === 'path' && e.fields.length === 0 && !scope.has(e.root);
  }

  expectType(e: Expr, scope: Scope, want: ValueType, what: string): ValueType | undefined {
    const t = this.typeExpr(e, scope, want);
    if (t && !compatible(t, want)) this.err('AIP-E201', `${what} must be ${typeName(want)}, got ${typeName(t)}`, e.loc);
    return t;
  }

  typeExpr(e: Expr, scope: Scope, expected?: ValueType): ValueType | undefined {
    const t = this.typeExprInner(e, scope, expected);
    if (t) e.type = t;
    return t;
  }

  typeExprInner(e: Expr, scope: Scope, expected?: ValueType): ValueType | undefined {
    switch (e.k) {
      case 'lit':
        if (e.value === null) return { k: 'null' };
        if (typeof e.value === 'number') return INT;
        if (typeof e.value === 'boolean') return BOOL;
        return { k: 'scalar', name: 'String' };
      case 'enum':
        return { k: 'enum', name: e.enum };
      case 'path':
        return this.typePath(e, scope, expected);
      case 'neg':
        this.expectType(e.e, scope, INT, 'operand of unary -');
        return INT;
      case 'not':
        this.expectType(e.e, scope, BOOL, "operand of 'not'");
        return BOOL;
      case 'in': {
        const te = this.typeExpr(e.e, scope);
        if (e.list.length === 0) this.err('AIP-E201', "'in' needs at least one value", e.loc);
        for (const item of e.list) {
          const ti = this.typeExpr(item, scope, te);
          if (te && ti && !compatible(te, ti)) this.err('AIP-E201', `'in' list item is ${typeName(ti)}, expected ${typeName(te)}`, item.loc);
        }
        return BOOL;
      }
      case 'bin': {
        switch (e.op) {
          case '+':
          case '-':
          case '*':
            this.expectType(e.l, scope, INT, `left side of '${e.op}'`);
            this.expectType(e.r, scope, INT, `right side of '${e.op}'`);
            return INT;
          case 'and':
          case 'or':
            this.expectType(e.l, scope, BOOL, `left side of '${e.op}'`);
            this.expectType(e.r, scope, BOOL, `right side of '${e.op}'`);
            return BOOL;
          case '<':
          case '<=':
          case '>':
          case '>=':
            this.expectType(e.l, scope, INT, `left side of '${e.op}'`);
            this.expectType(e.r, scope, INT, `right side of '${e.op}'`);
            return BOOL;
          case '==':
          case '!=': {
            let tl: ValueType | undefined;
            let tr: ValueType | undefined;
            if (this.isBareUnknown(e.l, scope) && !this.isBareUnknown(e.r, scope)) {
              tr = this.typeExpr(e.r, scope);
              tl = this.typeExpr(e.l, scope, tr);
            } else {
              tl = this.typeExpr(e.l, scope);
              tr = this.typeExpr(e.r, scope, tl);
            }
            if (tl && tr && !compatible(tl, tr)) this.err('AIP-E201', `cannot compare ${typeName(tl)} with ${typeName(tr)}`, e.loc);
            return BOOL;
          }
        }
      }
    }
  }

  typePath(e: PathExpr, scope: Scope, expected?: ValueType): ValueType | undefined {
    const sym = scope.get(e.root);
    if (!sym) {
      if (e.fields.length === 0) {
        const en = expected?.k === 'enum' ? this.model.enums.get(expected.name) : undefined;
        if (en && en.values.includes(e.root)) {
          const lit = e as unknown as Record<string, unknown>;
          const value = e.root;
          delete lit.root;
          delete lit.fields;
          Object.assign(lit, { k: 'enum', enum: en.name, value });
          return expected;
        }
        if (en) {
          this.err('AIP-E202', `'${e.root}' is not a value of enum ${en.name}`, e.loc, `values: ${en.values.join(', ')}`);
          return undefined;
        }
      }
      if (e.root === 'actor') {
        this.err('AIP-E110', "'actor' used but no actor entity is declared", e.loc, "add 'actor <Entity>' at top level");
        return undefined;
      }
      this.err('AIP-E104', `unknown name '${e.root}'`, e.loc, `in scope: ${[...scope.keys()].join(', ') || '(nothing)'}`);
      return undefined;
    }
    if (e.fields.length > sym.maxDepth) {
      this.err('AIP-E213', `'${pathText(e)}' reads through a relation; AIP never loads data implicitly`, e.loc,
        sym.kind === 'param' ? 'parameters are plain values and have no fields'
          : `load the related row explicitly (e.g. 'load x: <Entity> by ${[e.root, ...e.fields.slice(0, sym.maxDepth)].join('.')}')`);
      return undefined;
    }
    let t: ValueType = sym.type;
    for (const name of e.fields) {
      if (t.k !== 'ref') {
        this.err('AIP-E103', `cannot access '.${name}' on ${typeName(t)}`, e.loc);
        return undefined;
      }
      const f = this.model.field(t.entity, name);
      if (!f) {
        this.err('AIP-E103', `${t.entity} has no field '${name}'`, e.loc, `fields: id, ${this.model.entity(t.entity).fields.map((x) => x.name).join(', ')}`);
        return undefined;
      }
      t = fieldType(f);
    }
    return t;
  }

  // ---- policy ----

  checkPolicy(p: Policy | null, scope: Scope, owner: string, loc: Loc, isCommand: boolean) {
    if (!p) {
      this.err('AIP-E301', `${owner} has no policy`, loc, "nothing is exposed implicitly; add 'policy: public | authenticated | owner(<path>) | role(<ROLE>)'");
      return;
    }
    const actor = this.app.actor ? this.model.entities.get(this.app.actor.entity) : undefined;
    const walk = (x: Policy) => {
      switch (x.k) {
        case 'or':
        case 'and':
          walk(x.l);
          walk(x.r);
          return;
        case 'public':
          if (isCommand) this.warn('AIP-W302', `${owner} is callable without authentication`, x.loc, 'make sure an anonymous caller may perform this change');
          return;
        case 'authenticated':
          if (!this.app.actor) this.err('AIP-E110', "'authenticated' used but no actor entity is declared", x.loc, "add 'actor <Entity>' at top level");
          return;
        case 'role': {
          if (!actor) {
            this.err('AIP-E110', "'role' used but no actor entity is declared", x.loc);
            return;
          }
          const rf = this.model.field(actor.name, 'role');
          if (!rf || rf.kind !== 'enum') {
            this.err('AIP-E304', `role(${x.role}) needs an enum field 'role' on ${actor.name}`, x.loc);
            return;
          }
          if (!this.model.enums.get(rf.enum)?.values.includes(x.role)) {
            this.err('AIP-E304', `'${x.role}' is not a value of ${rf.enum}`, x.loc, `values: ${this.model.enums.get(rf.enum)?.values.join(', ')}`);
          }
          return;
        }
        case 'owner': {
          if (!actor) {
            this.err('AIP-E110', "'owner' used but no actor entity is declared", x.loc);
            return;
          }
          const sym = scope.get(x.path.root);
          if (sym && sym.kind === 'binding' && !sym.fromLoad) {
            this.err('AIP-E305', `owner(${pathText(x.path)}) refers to '${x.path.root}', which does not exist yet when the policy is checked`, x.loc);
            return;
          }
          const t = this.typeExpr(x.path, scope);
          if (t && !(t.k === 'ref' && t.entity === actor.name)) {
            this.err('AIP-E303', `owner(${pathText(x.path)}) must point to a ${actor.name}, got ${typeName(t)}`, x.loc);
          }
          return;
        }
      }
    };
    walk(p);
  }

  // ---- queries ----

  baseScope(params: { name: string; type: ValueType; loc: Loc }[]): Scope {
    const scope: Scope = new Map();
    if (this.app.actor && this.model.entities.has(this.app.actor.entity)) {
      scope.set('actor', { kind: 'actor', type: { k: 'ref', entity: this.app.actor.entity }, maxDepth: 1 });
    }
    for (const p of params) {
      if (scope.has(p.name)) this.err('AIP-E101', `'${p.name}' is reserved or already defined`, p.loc);
      if (p.type.k === 'enum' && !this.model.enums.has(p.type.name)) {
        this.err('AIP-E102', `unknown parameter type '${p.type.name}'`, p.loc, 'parameters are scalars (UUID, String, Int, Bool) or enums; pass entity ids as UUID');
      }
      scope.set(p.name, { kind: 'param', type: p.type, maxDepth: 0 });
    }
    return scope;
  }

  checkQuery(q: Query) {
    const scope = this.baseScope(q.params);
    if (!this.model.entities.has(q.entity)) {
      this.err('AIP-E102', `unknown entity '${q.entity}'`, q.loc);
      return;
    }
    if (scope.has(q.as)) this.err('AIP-E101', `alias '${q.as}' shadows another name`, q.loc);
    scope.set(q.as, { kind: 'alias', type: { k: 'ref', entity: q.entity }, maxDepth: 1 });
    if (q.by) {
      const t = this.typeExpr(q.by, scope);
      if (t && !compatible(t, { k: 'ref', entity: q.entity })) this.err('AIP-E212', `'by' must be a UUID, got ${typeName(t)}`, q.by.loc);
    }
    if (q.where) this.expectType(q.where, scope, BOOL, "'where'");
    if (q.sort) {
      const f = this.model.field(q.entity, q.sort.field);
      if (!f) this.err('AIP-E103', `${q.entity} has no field '${q.sort.field}'`, q.sort.loc);
      else if (f.kind === 'many' || f.kind === 'ref') this.err('AIP-E201', `cannot sort by relation '${q.sort.field}'`, q.sort.loc);
    }
    if (q.limit !== null && (q.limit < 1 || q.limit > 1000)) this.err('AIP-E201', 'limit must be between 1 and 1000', q.loc);
    if (!q.by && q.limit === null) {
      this.warn('AIP-W403', `query ${q.name} returns an unbounded list`, q.loc, "add 'limit <n>'");
    }
    this.checkPolicy(q.policy, scope, `query ${q.name}`, q.loc, false);
    this.checkSelection(q.entity, q.select, q.name);
  }

  checkSelection(entity: string, sel: Selection, owner: string) {
    const seen = new Set<string>();
    if (sel.fields.length === 0) this.err('AIP-E220', `empty selection on ${entity} in ${owner}`, { line: 0, col: 0 });
    for (const n of sel.fields) {
      if (seen.has(n.name)) this.err('AIP-E101', `'${n.name}' selected twice`, n.loc);
      seen.add(n.name);
      const f = this.model.field(entity, n.name);
      if (!f) {
        this.err('AIP-E103', `${entity} has no field '${n.name}'`, n.loc);
        continue;
      }
      const isRel = f.kind === 'ref' || f.kind === 'many';
      if (isRel && !n.sub) {
        this.err('AIP-E220', `relation '${entity}.${n.name}' needs a sub-selection`, n.loc, `write '${n.name} { id ... }'`);
      } else if (!isRel && n.sub) {
        this.err('AIP-E220', `'${entity}.${n.name}' is not a relation and cannot have a sub-selection`, n.loc);
      } else if (isRel && n.sub) {
        this.checkSelection(f.target, n.sub, owner);
      }
    }
  }

  // ---- commands ----

  checkCommand(c: Command) {
    const scope = this.baseScope(c.params);
    const info: CommandInfo = { writes: [], creates: [], errors: c.requires.map((r) => r.code) };
    this.commands.set(c.name, info);

    for (const l of c.loads) {
      if (!this.model.entities.has(l.entity)) {
        this.err('AIP-E102', `unknown entity '${l.entity}'`, l.loc);
        continue;
      }
      const t = this.typeExpr(l.by, scope);
      if (t && !compatible(t, { k: 'ref', entity: l.entity })) this.err('AIP-E212', `'load ${l.name}' must be by a UUID, got ${typeName(t)}`, l.by.loc);
      if (scope.has(l.name)) this.err('AIP-E101', `'${l.name}' is already defined`, l.loc);
      scope.set(l.name, { kind: 'binding', type: { k: 'ref', entity: l.entity }, maxDepth: 1, lock: l.lock, fromLoad: true });
    }

    this.checkPolicy(c.policy, scope, `command ${c.name}`, c.loc, true);

    for (const r of c.requires) this.expectType(r.cond, scope, BOOL, 'require condition');

    const writes: { root: string; field: string; entity: string; loc: Loc }[] = [];
    this.checkStmts(c.tx, scope, writes, null, info);
    info.writes = [...new Set(writes.map((w) => `${w.entity}.${w.field}`))];

    // Check-then-act: a condition verified in `require` and then changed in the
    // transaction is only safe if the row is locked for the whole command.
    for (const r of c.requires) {
      for (const p of paths(r.cond)) {
        if (p.fields.length !== 1) continue;
        const sym = scope.get(p.root);
        if (!sym || sym.kind !== 'binding' || !sym.fromLoad || sym.lock) continue;
        const w = writes.find((x) => x.root === p.root && x.field === p.fields[0]);
        if (w) {
          this.err('AIP-E401', `check-then-act race on '${p.root}.${p.fields[0]}': it is checked in 'require' and modified in 'transaction', but '${p.root}' is not locked`,
            w.loc, `write 'load ${p.root}: ${typeName(sym.type)} by ... lock'`);
        }
      }
    }

    if (info.creates.length > 0 && !c.idempotent) {
      this.warn('AIP-W501', `command ${c.name} creates ${info.creates.join(', ')} but is not idempotent; a client retry creates duplicates`, c.loc,
        "add 'idempotent' so callers can send an Idempotency-Key");
    }

    for (const em of c.emits) this.checkEmit(em.event, em.fields, scope, em.loc);

    if (c.returns) {
      const sym = scope.get(c.returns.name);
      if (!sym || sym.kind !== 'binding') this.err('AIP-E211', `'returns ${c.returns.name}' does not name a loaded or created row`, c.returns.loc);
    }
  }

  checkStmts(stmts: Stmt[], scope: Scope, writes: { root: string; field: string; entity: string; loc: Loc }[], iter: string | null, info: CommandInfo) {
    for (const s of stmts) {
      switch (s.k) {
        case 'set':
        case 'inc': {
          const target = this.mutationTarget(s.target, scope, iter, s.k === 'set');
          if (!target) break;
          const f = this.model.field(target.entity, target.field);
          if (!f) {
            this.err('AIP-E103', `${target.entity} has no field '${target.field}'`, s.target.loc);
            break;
          }
          if (f.name === 'id' || f.kind === 'many') {
            this.err('AIP-E203', `'${pathText(s.target)}' cannot be assigned`, s.target.loc);
            break;
          }
          if (s.k === 'set') {
            this.expectType(s.value, scope, fieldType(f), `value for ${target.entity}.${f.name}`);
          } else {
            if (!(f.kind === 'scalar' && f.type === 'Int')) this.err('AIP-E204', `${s.sign > 0 ? 'increment' : 'decrement'} needs an Int field, '${pathText(s.target)}' is ${typeName(fieldType(f))}`, s.loc);
            this.expectType(s.by, scope, INT, 'increment amount');
          }
          writes.push({ root: s.target.root, field: target.field, entity: target.entity, loc: s.loc });
          break;
        }
        case 'create': {
          if (iter) {
            this.err('AIP-E215', "'create' is not allowed inside 'each'", s.loc);
            break;
          }
          if (!this.model.entities.has(s.entity)) {
            this.err('AIP-E102', `unknown entity '${s.entity}'`, s.loc);
            break;
          }
          const given = new Set<string>();
          for (const fv of s.fields) {
            const f = this.model.field(s.entity, fv.name);
            if (!f) {
              this.err('AIP-E103', `${s.entity} has no field '${fv.name}'`, fv.loc);
              continue;
            }
            if (f.name === 'id' || f.kind === 'many') {
              this.err('AIP-E203', `'${fv.name}' cannot be assigned`, fv.loc);
              continue;
            }
            if (given.has(fv.name)) this.err('AIP-E101', `'${fv.name}' given twice`, fv.loc);
            given.add(fv.name);
            this.expectType(fv.value, scope, fieldType(f), `value for ${s.entity}.${f.name}`);
          }
          const missing = this.model.entity(s.entity).fields.filter((f) => f.kind !== 'many' && !f.optional && !given.has(f.name));
          if (missing.length) this.err('AIP-E205', `create ${s.entity} is missing ${missing.map((f) => f.name).join(', ')}`, s.loc);
          if (s.as) {
            if (scope.has(s.as)) this.err('AIP-E101', `'${s.as}' is already defined`, s.loc);
            scope.set(s.as, { kind: 'binding', type: { k: 'ref', entity: s.entity }, maxDepth: 1 });
          }
          info.creates.push(s.entity);
          break;
        }
        case 'each': {
          if (iter) {
            this.err('AIP-E214', "nested 'each' is not supported", s.loc);
            break;
          }
          const sym = scope.get(s.source.root);
          if (!sym || sym.kind !== 'binding' || s.source.fields.length !== 1) {
            this.err('AIP-E206', `'each' must iterate over a list field of a loaded row, like 'each order.items as item'`, s.source.loc);
            break;
          }
          const t = sym.type.k === 'ref' ? this.model.field(sym.type.entity, s.source.fields[0]) : undefined;
          if (!t || t.kind !== 'many') {
            this.err('AIP-E206', `'${pathText(s.source)}' is not a list relation`, s.source.loc);
            break;
          }
          if (scope.has(s.as)) this.err('AIP-E101', `'${s.as}' is already defined`, s.loc);
          const inner: Scope = new Map(scope);
          inner.set(s.as, { kind: 'iter', type: { k: 'ref', entity: t.target }, maxDepth: 1 });
          this.checkStmts(s.body, inner, writes, s.as, info);
          break;
        }
      }
    }
  }

  // Resolves `x.f` (or `iter.rel.f` inside each) to the entity/field written.
  mutationTarget(p: PathExpr, scope: Scope, iter: string | null, isSet: boolean): { entity: string; field: string } | undefined {
    const sym = scope.get(p.root);
    if (!sym || (sym.kind !== 'binding' && sym.kind !== 'iter')) {
      this.err('AIP-E209', `'${pathText(p)}' is not a field of a loaded or created row`, p.loc,
        sym?.kind === 'actor' ? "load the actor's row explicitly to modify it" : undefined);
      return undefined;
    }
    if (iter && p.root !== iter) {
      this.err('AIP-E208', `inside 'each', only '${iter}' and rows reached from it can be modified`, p.loc);
      return undefined;
    }
    if (sym.type.k !== 'ref') return undefined;
    if (p.fields.length === 1) return { entity: sym.type.entity, field: p.fields[0] };
    if (sym.kind === 'iter' && p.fields.length === 2) {
      const rel = this.model.field(sym.type.entity, p.fields[0]);
      if (!rel || rel.kind !== 'ref') {
        this.err('AIP-E103', `'${p.root}.${p.fields[0]}' is not a reference`, p.loc);
        return undefined;
      }
      if (isSet) {
        this.err('AIP-E207', `'set ${pathText(p)}' is ambiguous when several ${sym.type.entity} rows point to the same ${rel.target}`, p.loc,
          'only increment/decrement (which add up) may write through a reference inside each');
        return undefined;
      }
      return { entity: rel.target, field: p.fields[1] };
    }
    this.err('AIP-E213', `'${pathText(p)}' writes through a relation`, p.loc, 'load the row explicitly and modify it directly');
    return undefined;
  }

  checkEmit(event: string, fields: { name: string; value: Expr; loc: Loc }[], scope: Scope, loc: Loc) {
    const schema: EventSchema = { name: event, fields: [], loc };
    for (const fv of fields) {
      const t = this.typeExpr(fv.value, scope);
      if (t) schema.fields.push({ name: fv.name, type: t });
    }
    const prev = this.events.get(event);
    if (!prev) {
      this.events.set(event, schema);
      return;
    }
    const sig = (s: EventSchema) => s.fields.map((f) => `${f.name}:${typeName(f.type)}`).sort().join(',');
    if (sig(prev) !== sig(schema)) {
      this.err('AIP-E216', `event ${event} is emitted with a different shape than at line ${prev.loc.line}`, loc, `expected { ${sig(prev)} }`);
    }
  }
}

export function paths(e: Expr): PathExpr[] {
  switch (e.k) {
    case 'path':
      return [e];
    case 'bin':
      return [...paths(e.l), ...paths(e.r)];
    case 'not':
    case 'neg':
      return paths(e.e);
    case 'in':
      return [...paths(e.e), ...e.list.flatMap(paths)];
    default:
      return [];
  }
}

export function analyze(app: App): Analysis {
  return new Analyzer(app).run();
}
